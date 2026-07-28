//! Paper discovery ("논문 찾기") — searches Semantic Scholar for papers by topic
//! and imports an open-access PDF through the normal import pipeline. The frontend
//! (Track V) already speaks this contract; see docs/discover-ipc-spec.md.
//!
//! Semantic Scholar's Academic Graph API is free and public; a key is optional and,
//! if ever added, lives only in the OS credential store — never in logs or errors,
//! and never crosses to the frontend (CLAUDE.md).

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

use crate::commands::library::LIBRARY_CHANGED;
use crate::db::paper_repo::{self, PaperPatch};
use crate::error::{AppError, CommandResult, Result};
use crate::ids::new_id;
use crate::import::{self, pdf_file::RejectReason, ImportOutcome};
use crate::providers;
use crate::state::AppState;

const API: &str = "https://api.semanticscholar.org/graph/v1";
const FIELDS: &str =
    "title,authors,year,venue,abstract,externalIds,citationCount,openAccessPdf,url";
const MAX_ATTEMPTS: usize = 3;
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;
const REQUEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

static RATE_GATE: tokio::sync::Mutex<Option<tokio::time::Instant>> =
    tokio::sync::Mutex::const_new(None);

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
    pub local_paper_id: Option<String>,
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
        local_paper_id: None,
    })
}

// --- search ------------------------------------------------------------------

async fn wait_for_rate_slot() {
    let mut next = RATE_GATE.lock().await;
    if let Some(deadline) = *next {
        tokio::time::sleep_until(deadline).await;
    }
    *next = Some(tokio::time::Instant::now() + REQUEST_INTERVAL);
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        // OA links may redirect between publisher/CDN hosts, but never permit a
        // downgrade to clear-text HTTP.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.stop()
            } else if attempt.url().scheme() == "https" {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|error| AppError::Internal(format!("discover http client: {error}")))
}

async fn send_s2(
    client: &reqwest::Client,
    url: &str,
    params: &[(&str, String)],
    api_key: Option<&str>,
) -> Result<reqwest::Response> {
    for attempt in 0..MAX_ATTEMPTS {
        wait_for_rate_slot().await;
        let mut request = client.get(url).query(params);
        if let Some(key) = api_key {
            request = request.header("x-api-key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| providers::map_transport(&error))?;

        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let retry_after = providers::retry_after_seconds(response.headers());
        let retryable =
            status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        if retryable && attempt + 1 < MAX_ATTEMPTS {
            let delay = retry_after.unwrap_or(1u64 << attempt).clamp(1, 30);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            continue;
        }
        return Err(providers::map_status(status, retry_after));
    }
    Err(AppError::ProviderUnavailable)
}

async fn fetch(
    query: &str,
    offset: i64,
    limit: i64,
    year_from: Option<i64>,
    open_access_only: bool,
    api_key: Option<&str>,
) -> Result<(Vec<DiscoveredPaper>, i64)> {
    let client = client()?;

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
    if open_access_only {
        // This flag intentionally has an empty value per the API contract.
        // Filtering at the source keeps totals and pagination truthful.
        params.push(("openAccessPdf", String::new()));
    }

    let response = send_s2(&client, &format!("{API}/paper/search"), &params, api_key).await?;

    // The body is parsed but never logged (§16.1).
    let body: S2Response = response
        .json()
        .await
        .map_err(|_| AppError::ProviderResponse("검색 응답을 해석하지 못했습니다.".into()))?;

    let hits: Vec<DiscoveredPaper> = body
        .data
        .into_iter()
        .filter_map(map_paper)
        // Keep this defensive check in case an upstream record violates its
        // own openAccessPdf filter.
        .filter(|paper| !open_access_only || paper.pdf_url.is_some())
        .collect();

    Ok((hits, body.total))
}

async fn fetch_paper(paper_id: &str, api_key: Option<&str>) -> Result<DiscoveredPaper> {
    let source_id = source_paper_id(paper_id)?;

    let client = client()?;
    let params = vec![("fields", FIELDS.to_string())];
    let response = send_s2(
        &client,
        &format!("{API}/paper/{source_id}"),
        &params,
        api_key,
    )
    .await?;
    let body: S2Paper = response
        .json()
        .await
        .map_err(|_| AppError::ProviderResponse("논문 응답을 해석하지 못했습니다.".into()))?;
    map_paper(body).ok_or_else(|| AppError::ProviderResponse("논문 ID가 없습니다.".into()))
}

fn source_paper_id(paper_id: &str) -> Result<&str> {
    let source_id = paper_id
        .strip_prefix("semantic-scholar:")
        .ok_or_else(|| AppError::InvalidInput("unsupported discover source".into()))?;
    if source_id.is_empty()
        || !source_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(AppError::InvalidInput(
            "invalid semantic scholar paper id".into(),
        ));
    }
    Ok(source_id)
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

    let api_key = crate::secrets::get_semantic_scholar_key()?;
    let (mut hits, total) = fetch(
        &term,
        offset,
        limit,
        query.year_from,
        open_access_only,
        api_key.as_deref(),
    )
    .await?;

    // Mark papers already held, matched by DOI (case-insensitive). Done after the
    // await so no DB guard is held across it.
    {
        let conn = state.db.conn();
        for hit in hits.iter_mut() {
            if let Some(doi) = &hit.doi {
                hit.local_paper_id = conn
                    .query_row(
                        "SELECT paper_id FROM paper_metadata
                         WHERE doi IS NOT NULL AND lower(doi) = lower(?1)
                         LIMIT 1",
                        [doi],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();
                hit.already_in_library = hit.local_paper_id.is_some();
            }
        }
    }

    // Relevance search exposes at most the first 1,000 matches even when its
    // reported total is larger.
    let reachable_total = total.min(1_000);
    let next_offset = if offset + limit < reachable_total {
        Some(offset + limit)
    } else {
        None
    };
    Ok(DiscoverResults {
        hits,
        total,
        next_offset,
    })
}

// --- import ------------------------------------------------------------------

#[tauri::command]
pub async fn import_discovered_paper(
    app: AppHandle,
    state: State<'_, AppState>,
    paper_id: String,
    target_group_id: Option<String>,
) -> CommandResult<ImportOutcome> {
    // Never trust a download URL supplied by the webview. Re-resolve the stable
    // source ID in the Rust core and use only Semantic Scholar's current OA URL.
    let api_key = crate::secrets::get_semantic_scholar_key()?;
    let paper = fetch_paper(&paper_id, api_key.as_deref()).await?;
    let Some(pdf_url) = paper.pdf_url.clone() else {
        return Ok(ImportOutcome::Rejected {
            file_name: paper.title.clone(),
            reason: RejectReason::Unreadable,
            message: "무료로 내려받을 수 있는 PDF가 없습니다.".into(),
        });
    };

    let temp = std::env::temp_dir().join(format!("bbrain-discover-{}.pdf", new_id()));
    download_pdf(&pdf_url, &temp).await?;

    // Reuse the normal import: validate, hash, dedupe, copy, queue extraction.
    let import_result = {
        let db = state.db.clone();
        let paths = state.paths.clone();
        let group = target_group_id.clone();
        let temp = temp.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.conn();
            import::import_one(&mut conn, &paths, &temp, group.as_deref())
        })
        .await
    };
    let _ = std::fs::remove_file(&temp);
    let outcome =
        import_result.map_err(|error| AppError::Internal(format!("import task: {error}")))?;

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

async fn download_pdf(url: &str, destination: &std::path::Path) -> Result<()> {
    let result = download_pdf_inner(url, destination).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(destination).await;
    }
    result
}

async fn download_pdf_inner(url: &str, destination: &std::path::Path) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| AppError::ProviderResponse("PDF URL이 올바르지 않습니다.".into()))?;
    if parsed.scheme() != "https" {
        return Err(AppError::ProviderResponse(
            "안전하지 않은 PDF URL을 거부했습니다.".into(),
        ));
    }

    let response = client()?
        .get(parsed)
        .send()
        .await
        .map_err(|error| providers::map_transport(&error))?;
    if !response.status().is_success() {
        let retry_after = providers::retry_after_seconds(response.headers());
        return Err(providers::map_status(response.status(), retry_after));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_PDF_BYTES)
    {
        return Err(AppError::InvalidInput("PDF download exceeds 100 MB".into()));
    }

    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|error| AppError::Internal(format!("download temp file: {error}")))?;
    let mut stream = response.bytes_stream();
    let mut written = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::Network)?;
        written += chunk.len() as u64;
        if written > MAX_PDF_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(destination).await;
            return Err(AppError::InvalidInput("PDF download exceeds 100 MB".into()));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| AppError::Internal(format!("download write: {error}")))?;
    }
    file.flush()
        .await
        .map_err(|error| AppError::Internal(format!("download flush: {error}")))?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureSemanticScholarInput {
    pub api_key: String,
}

#[tauri::command]
pub async fn configure_semantic_scholar(
    state: State<'_, AppState>,
    input: ConfigureSemanticScholarInput,
) -> CommandResult<()> {
    let key = input.api_key.trim();
    if key.is_empty() {
        return Err(AppError::InvalidInput("empty semantic scholar key".into()).into());
    }

    // Validate before persisting so settings never claim a broken key works.
    fetch("test", 0, 1, None, false, Some(key)).await?;
    let reference = crate::secrets::set_semantic_scholar_key(key)?;
    if let Err(error) = crate::db::settings_repo::set_semantic_scholar_credential_ref(
        &state.db.conn(),
        Some(&reference),
    ) {
        let _ = crate::secrets::delete_semantic_scholar_key();
        return Err(error.into());
    }
    Ok(())
}

#[tauri::command]
pub fn remove_semantic_scholar(state: State<'_, AppState>) -> CommandResult<()> {
    crate::secrets::delete_semantic_scholar_key()?;
    crate::db::settings_repo::set_semantic_scholar_credential_ref(&state.db.conn(), None)?;
    Ok(())
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
        assert_eq!(
            paper.pdf_url.as_deref(),
            Some("https://example.org/attention.pdf")
        );
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

    #[test]
    fn only_semantic_scholar_result_ids_can_be_re_resolved() {
        assert_eq!(
            source_paper_id("semantic-scholar:abc123").unwrap(),
            "abc123"
        );
        assert!(source_paper_id("https://internal.example/pdf").is_err());
        assert!(source_paper_id("semantic-scholar:../secret").is_err());
    }
}
