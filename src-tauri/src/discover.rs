//! Paper discovery ("논문 찾기") — searches Semantic Scholar for papers by topic
//! and imports an open-access PDF through the normal import pipeline. The frontend
//! (Track V) already speaks this contract; see docs/discover-ipc-spec.md.
//!
//! Semantic Scholar's Academic Graph API is free and public; a key is optional and,
//! if ever added, lives only in the OS credential store — never in logs or errors,
//! and never crosses to the frontend (CLAUDE.md).

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::commands::library::LIBRARY_CHANGED;
use crate::db::paper_repo::{self, PaperPatch};
use crate::error::{AppError, CommandResult, Result};
use crate::ids::new_id;
use crate::import::{self, pdf_file::RejectReason, ImportOutcome};
use crate::providers;
use crate::state::AppState;

const API: &str = "https://api.semanticscholar.org/graph/v1";
const FIELDS: &str = "title,authors,year,venue,abstract,externalIds,citationCount,openAccessPdf,url";

// --- wire contract (mirrors src/lib/types.ts) --------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverQuery {
    pub query: String,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub year_from: Option<i64>,
    pub open_access_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPaper {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i64>,
    pub venue: Option<String>,
    #[serde(rename = "abstract")]
    pub abstract_text: Option<String>,
    pub pdf_url: Option<String>,
    pub url: String,
    pub doi: Option<String>,
    pub citation_count: Option<i64>,
    pub already_in_library: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResults {
    pub hits: Vec<DiscoveredPaper>,
    pub total: i64,
    pub next_offset: Option<i64>,
}

// --- Semantic Scholar response DTOs ------------------------------------------

#[derive(Debug, Deserialize)]
struct S2Response {
    #[serde(default)]
    total: i64,
    #[serde(default)]
    data: Vec<S2Paper>,
}

#[derive(Debug, Deserialize)]
struct S2Paper {
    #[serde(rename = "paperId")]
    paper_id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    authors: Vec<S2Author>,
    year: Option<i64>,
    venue: Option<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    #[serde(rename = "externalIds")]
    external_ids: Option<S2ExternalIds>,
    #[serde(rename = "citationCount")]
    citation_count: Option<i64>,
    #[serde(rename = "openAccessPdf")]
    open_access_pdf: Option<S2Pdf>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct S2Author {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct S2ExternalIds {
    #[serde(rename = "DOI")]
    doi: Option<String>,
}

#[derive(Debug, Deserialize)]
struct S2Pdf {
    url: Option<String>,
}

/// Maps one Semantic Scholar record to a `DiscoveredPaper`, or `None` when it has
/// no stable id. `already_in_library` is filled in later against the local DB.
/// Pure, so the brittle field mapping is unit-tested.
fn map_paper(paper: S2Paper) -> Option<DiscoveredPaper> {
    let paper_id = paper.paper_id?;
    Some(DiscoveredPaper {
        id: format!("semantic-scholar:{paper_id}"),
        title: paper
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "제목 없음".into()),
        authors: paper.authors.into_iter().filter_map(|a| a.name).collect(),
        year: paper.year,
        venue: paper.venue,
        abstract_text: paper.abstract_text,
        pdf_url: paper.open_access_pdf.and_then(|pdf| pdf.url),
        url: paper.url.unwrap_or_default(),
        doi: paper.external_ids.and_then(|ids| ids.doi),
        citation_count: paper.citation_count,
        already_in_library: false,
    })
}

// --- search ------------------------------------------------------------------

async fn fetch(
    query: &str,
    offset: i64,
    limit: i64,
    year_from: Option<i64>,
    open_access_only: bool,
) -> Result<(Vec<DiscoveredPaper>, i64)> {
    let client = providers::client()?;

    let mut params: Vec<(&str, String)> = vec![
        ("query", query.to_string()),
        ("offset", offset.to_string()),
        ("limit", limit.to_string()),
        ("fields", FIELDS.to_string()),
    ];
    if let Some(year) = year_from {
        // Semantic Scholar reads "2020-" as "2020 onwards".
        params.push(("year", format!("{year}-")));
    }

    let response = client
        .get(format!("{API}/paper/search"))
        .query(&params)
        .send()
        .await
        .map_err(|error| providers::map_transport(&error))?;

    if !response.status().is_success() {
        let retry_after = providers::retry_after_seconds(response.headers());
        return Err(providers::map_status(response.status(), retry_after));
    }

    // The body is parsed but never logged (§16.1).
    let body: S2Response = response
        .json()
        .await
        .map_err(|_| AppError::ProviderResponse("검색 응답을 해석하지 못했습니다.".into()))?;

    let hits: Vec<DiscoveredPaper> = body
        .data
        .into_iter()
        .filter_map(map_paper)
        .filter(|paper| !open_access_only || paper.pdf_url.is_some())
        .collect();

    Ok((hits, body.total))
}

#[tauri::command]
pub async fn search_papers(
    state: State<'_, AppState>,
    query: DiscoverQuery,
) -> CommandResult<DiscoverResults> {
    let term = query.query.trim().to_string();
    if term.is_empty() {
        return Err(AppError::InvalidInput("검색어를 입력하세요.".into()).into());
    }

    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);
    let open_access_only = query.open_access_only.unwrap_or(false);

    let (mut hits, total) = fetch(&term, offset, limit, query.year_from, open_access_only).await?;

    // Mark papers already held, matched by DOI (case-insensitive). Done after the
    // await so no DB guard is held across it.
    {
        let conn = state.db.conn();
        for hit in hits.iter_mut() {
            if let Some(doi) = &hit.doi {
                hit.already_in_library = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM paper_metadata
                         WHERE doi IS NOT NULL AND lower(doi) = lower(?1))",
                        [doi],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap_or(false);
            }
        }
    }

    let next_offset = if offset + limit < total { Some(offset + limit) } else { None };
    Ok(DiscoverResults { hits, total, next_offset })
}

// --- import ------------------------------------------------------------------

#[tauri::command]
pub async fn import_discovered_paper(
    app: AppHandle,
    state: State<'_, AppState>,
    paper: DiscoveredPaper,
    target_group_id: Option<String>,
) -> CommandResult<ImportOutcome> {
    let Some(pdf_url) = paper.pdf_url.clone() else {
        return Ok(ImportOutcome::Rejected {
            file_name: paper.title.clone(),
            reason: RejectReason::Unreadable,
            message: "무료로 내려받을 수 있는 PDF가 없습니다.".into(),
        });
    };

    // Download the open-access PDF the source pointed us to.
    let client = providers::client()?;
    let response = client
        .get(&pdf_url)
        .send()
        .await
        .map_err(|error| providers::map_transport(&error))?;
    if !response.status().is_success() {
        let retry_after = providers::retry_after_seconds(response.headers());
        return Err(providers::map_status(response.status(), retry_after).into());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| AppError::Network)?;

    let temp = std::env::temp_dir().join(format!("bbrain-discover-{}.pdf", new_id()));
    std::fs::write(&temp, &bytes)
        .map_err(|e| AppError::Internal(format!("다운로드 저장 실패: {e}")))?;

    // Reuse the normal import: validate, hash, dedupe, copy, queue extraction.
    let outcome = {
        let db = state.db.clone();
        let paths = state.paths.clone();
        let group = target_group_id.clone();
        let temp = temp.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.conn();
            import::import_one(&mut conn, &paths, &temp, group.as_deref())
        })
        .await
        .map_err(|e| AppError::Internal(format!("import task: {e}")))?
    };
    let _ = std::fs::remove_file(&temp);

    // Search metadata is richer than what a bare PDF yields — fill it in.
    if let ImportOutcome::Imported { paper_id, .. } = &outcome {
        {
            let mut conn = state.db.conn();
            let _ = paper_repo::update(
                &mut conn,
                paper_id,
                &PaperPatch {
                    title: Some(paper.title.clone()),
                    is_favorite: None,
                    year: paper.year,
                    venue: paper.venue.clone(),
                    doi: paper.doi.clone(),
                    authors: Some(paper.authors.clone()),
                    group_ids: None,
                    tags: None,
                },
            );
        }
        crate::jobs::enqueue_import_pipeline(&state.db.conn(), std::slice::from_ref(paper_id))?;
        let _ = app.emit(LIBRARY_CHANGED, ());
        state.jobs.notify();
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> Option<DiscoveredPaper> {
        map_paper(serde_json::from_value(value).unwrap())
    }

    #[test]
    fn maps_a_full_record() {
        let paper = parse(json!({
            "paperId": "abc123",
            "title": "Attention Is All You Need",
            "authors": [{ "name": "Ashish Vaswani" }, { "name": "Noam Shazeer" }],
            "year": 2017,
            "venue": "NeurIPS",
            "abstract": "The dominant sequence transduction models...",
            "externalIds": { "DOI": "10.5555/3295222.3295349" },
            "citationCount": 90000,
            "openAccessPdf": { "url": "https://example.org/attention.pdf" },
            "url": "https://www.semanticscholar.org/paper/abc123"
        }))
        .expect("a record with an id maps");

        assert_eq!(paper.id, "semantic-scholar:abc123");
        assert_eq!(paper.authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!(paper.doi.as_deref(), Some("10.5555/3295222.3295349"));
        assert_eq!(paper.pdf_url.as_deref(), Some("https://example.org/attention.pdf"));
        assert!(!paper.already_in_library);
    }

    #[test]
    fn a_record_without_an_id_is_dropped() {
        assert!(parse(json!({ "title": "no id" })).is_none());
    }

    #[test]
    fn missing_optional_fields_degrade_gracefully() {
        let paper = parse(json!({ "paperId": "x", "url": "https://s2/x" })).unwrap();
        assert_eq!(paper.title, "제목 없음");
        assert!(paper.authors.is_empty());
        assert!(paper.pdf_url.is_none());
        assert!(paper.doi.is_none());
    }
}
