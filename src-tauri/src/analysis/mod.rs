pub mod markdown;
pub mod schema;

use rusqlite::params;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{page_repo, paper_repo, settings_repo};
use crate::error::{AppError, Result};
use crate::providers::request::StructuredRequest;
use crate::providers::{AnyProvider, Provider};
use crate::state::AppState;
use crate::time::now_iso8601;
use schema::PaperAnalysisV1;

/// Conservative character budget per request. A paper longer than this is
/// summarized section by section first (DEVELOPMENT.md §10.4).
const DIRECT_CHARACTER_BUDGET: usize = 120_000;
const SECTION_CHARACTER_BUDGET: usize = 40_000;

const SYSTEM: &str = "\
당신은 연구 논문을 정리하는 도우미입니다. \
제공된 논문 텍스트에 실제로 있는 내용만 사용하고 추측하지 마세요. \
기여점과 결과에는 근거가 있는 페이지 번호를 함께 제시하세요. \
근거를 찾을 수 없으면 페이지 목록을 비워 두세요. \
모든 서술형 값은 한국어로 작성하고, 키워드와 태그는 논문에서 쓰인 표기를 유지하세요.";

/// The active provider plus its model, or a clear reason why AI work cannot run.
pub struct ActiveProvider {
    pub provider: Provider,
    pub model: String,
    pub client: AnyProvider,
}

pub fn resolve_provider(app: &AppHandle) -> Result<ActiveProvider> {
    let state = app.state::<AppState>();

    let settings = {
        let conn = state.db.conn();
        settings_repo::get(&conn)?
    };

    let provider = settings
        .active_provider
        .ok_or_else(|| AppError::NotFound("no active provider".into()))?;

    let model = match provider {
        Provider::OpenAi => settings.openai_model,
        Provider::Anthropic => settings.anthropic_model,
        Provider::DeepSeek => settings.deepseek_model,
    }
    .ok_or_else(|| AppError::ModelUnsupported("no model selected".into()))?;

    let api_key = state
        .credentials
        .get(provider)?
        // A configured provider with no key in the keychain means the user
        // removed it outside the app; treat it as an auth failure so the job
        // waits for a key instead of retrying.
        .ok_or(AppError::ProviderAuth)?;

    Ok(ActiveProvider {
        client: AnyProvider::new(provider, &api_key),
        provider,
        model,
    })
}

/// Runs the analysis job for one paper.
pub async fn analyze_paper(app: &AppHandle, paper_id: &str) -> Result<()> {
    let (paper, pages, content_hash) = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();

        let paper = paper_repo::get(&conn, paper_id)?;
        let pages = page_repo::document_text(&conn, paper_id)?;
        if pages.is_empty() {
            return Err(AppError::InvalidInput("paper has no extracted text".into()));
        }

        let content_hash = page_repo::text_hash(
            &pages
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        (paper, pages, content_hash)
    };

    let active = match resolve_provider(app) {
        Ok(active) => active,
        Err(error) => {
            // No key yet: the paper is readable, the analysis simply waits.
            let state = app.state::<AppState>();
            paper_repo::set_status(
                &state.db.conn(),
                paper_id,
                paper_repo::ImportStatus::WaitingForAi,
            )?;
            return Err(error);
        }
    };

    {
        let state = app.state::<AppState>();
        paper_repo::set_status(
            &state.db.conn(),
            paper_id,
            paper_repo::ImportStatus::Analyzing,
        )?;
    }

    let page_count = pages.len() as i64;
    let source = build_source(&pages);

    let analysis = generate_with_repair(&active, &source, page_count).await?;
    let markdown = markdown::render(&paper, &analysis, &[]);

    {
        let state = app.state::<AppState>();
        let conn = state.db.conn();

        conn.execute(
            "INSERT INTO analyses
               (paper_id, schema_version, provider, model, content_hash, structured_json,
                markdown, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(paper_id) DO UPDATE SET
               schema_version = excluded.schema_version,
               provider = excluded.provider,
               model = excluded.model,
               content_hash = excluded.content_hash,
               structured_json = excluded.structured_json,
               markdown = excluded.markdown,
               created_at = excluded.created_at",
            params![
                paper_id,
                schema::SCHEMA_VERSION,
                active.provider.as_str(),
                active.model,
                content_hash,
                serde_json::to_string(&analysis).unwrap_or_default(),
                markdown,
                now_iso8601(),
            ],
        )?;

        // AI may suggest tags, but never reassigns the user's groups (§3).
        for tag in &analysis.suggested_tags {
            if let Ok(tag_id) = paper_repo::upsert_tag(&conn, tag, "ai") {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id) VALUES (?1, ?2)",
                    params![paper_id, tag_id],
                );
            }
        }

        if let Some(abstract_text) = analysis.short_summary.get(..).map(str::to_string) {
            conn.execute(
                "UPDATE paper_metadata SET abstract = COALESCE(abstract, ?1) WHERE paper_id = ?2",
                params![abstract_text, paper_id],
            )?;
        }

        paper_repo::reindex_fts(&conn, paper_id)?;
        paper_repo::set_status(&conn, paper_id, paper_repo::ImportStatus::Ready)?;

        // Relations depend on the analysis, so they are enqueued once it lands.
        crate::jobs::queue::enqueue(
            &conn,
            Some(paper_id),
            crate::jobs::JobType::Relations,
            crate::jobs::RELATIONS_VERSION,
        )?;
        if settings_repo::get(&conn)?.obsidian_vault_path.is_some() {
            crate::jobs::queue::enqueue(
                &conn,
                Some(paper_id),
                crate::jobs::JobType::ObsidianSync,
                crate::jobs::OBSIDIAN_VERSION,
            )?;
        }
    }

    let _ = app.emit(crate::commands::viewer::PAPER_CHANGED, paper_id);
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

/// A structured response that fails validation gets exactly one repair attempt,
/// then the job fails (DEVELOPMENT.md §10.2).
async fn generate_with_repair(
    active: &ActiveProvider,
    source: &str,
    page_count: i64,
) -> Result<PaperAnalysisV1> {
    let request = |instructions: String| StructuredRequest {
        model: active.model.clone(),
        system: SYSTEM.to_string(),
        instructions,
        source_material: source.to_string(),
        schema: schema::json_schema(),
        schema_name: "paper_analysis_v1".into(),
        max_output_tokens: 8_000,
    };

    let first = active
        .client
        .generate_structured(request(instructions(page_count)))
        .await?;

    match schema::validate(first, page_count) {
        Ok(analysis) => Ok(analysis),
        Err(error) => {
            tracing::warn!(code = ?error.code(), "analysis failed validation; repairing once");

            let repaired = active
                .client
                .generate_structured(request(format!(
                    "{}\n\n이전 응답이 스키마 검증에 실패했습니다. 모든 필수 필드를 채우고 \
                     evidencePages는 1부터 {page_count} 사이의 정수만 사용하세요.",
                    instructions(page_count)
                )))
                .await?;

            schema::validate(repaired, page_count)
        }
    }
}

fn instructions(page_count: i64) -> String {
    format!(
        "다음 {page_count}쪽 논문을 분석해 스키마에 맞는 JSON으로만 답하세요. \
         evidencePages에는 해당 주장을 실제로 확인할 수 있는 페이지 번호만 넣으세요."
    )
}

/// Page markers let the model cite pages it can actually point at. Long papers
/// are trimmed section by section rather than truncated mid-sentence.
fn build_source(pages: &[(i64, String)]) -> String {
    let full: String = pages
        .iter()
        .map(|(number, text)| format!("[page {number}]\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n");

    if full.len() <= DIRECT_CHARACTER_BUDGET {
        return full;
    }

    // Keep the front matter (abstract, intro) and the tail (conclusion), which
    // carry the claims an analysis needs, and sample the middle.
    let head_budget = SECTION_CHARACTER_BUDGET;
    let tail_budget = SECTION_CHARACTER_BUDGET;

    let mut head = String::new();
    for (number, text) in pages {
        if head.len() + text.len() > head_budget {
            break;
        }
        head.push_str(&format!("[page {number}]\n{text}\n\n"));
    }

    let mut tail = String::new();
    for (number, text) in pages.iter().rev() {
        if tail.len() + text.len() > tail_budget {
            break;
        }
        tail.insert_str(0, &format!("[page {number}]\n{text}\n\n"));
    }

    format!("{head}\n[중략]\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(count: usize, size: usize) -> Vec<(i64, String)> {
        (1..=count)
            .map(|number| (number as i64, "가".repeat(size)))
            .collect()
    }

    #[test]
    fn a_short_paper_is_sent_whole_with_page_markers() {
        let source = build_source(&pages(3, 100));

        assert!(source.contains("[page 1]"));
        assert!(source.contains("[page 3]"));
        assert!(!source.contains("[중략]"));
    }

    #[test]
    fn a_long_paper_keeps_its_opening_and_closing_pages() {
        let source = build_source(&pages(200, 5_000));

        assert!(source.contains("[page 1]"));
        assert!(source.contains("[page 200]"));
        assert!(source.contains("[중략]"), "the middle must be marked as elided");
        assert!(source.len() < DIRECT_CHARACTER_BUDGET * 2);
    }

    #[test]
    fn the_system_prompt_forbids_invented_evidence() {
        assert!(SYSTEM.contains("추측하지"));
        assert!(SYSTEM.contains("근거를 찾을 수 없으면"));
    }
}
