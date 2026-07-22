use serde::Serialize;
use tauri::{AppHandle, State};

use crate::analysis::schema::PaperAnalysisV1;
use crate::db::settings_repo;
use crate::error::CommandResult;
use crate::jobs::{self, JobType};
use crate::state::AppState;
use crate::translation::{self, PageTranslation};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAnalysis {
    pub analysis: PaperAnalysisV1,
    pub markdown: String,
    pub provider: String,
    pub model: String,
    pub created_at: String,
}

#[tauri::command]
pub fn get_analysis(
    state: State<'_, AppState>,
    paper_id: String,
) -> CommandResult<Option<StoredAnalysis>> {
    let conn = state.db.conn();

    let row = conn
        .query_row(
            "SELECT structured_json, markdown, provider, model, created_at
             FROM analyses WHERE paper_id = ?1",
            [&paper_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .ok();

    Ok(row.and_then(|(json, markdown, provider, model, created_at)| {
        serde_json::from_str(&json).ok().map(|analysis| StoredAnalysis {
            analysis,
            markdown,
            provider,
            model,
            created_at,
        })
    }))
}

/// Re-runs the analysis at the user's request. A new task version bypasses the
/// idempotency key so the work actually re-runs (DEVELOPMENT.md §10.2).
#[tauri::command]
pub fn reanalyze_paper(state: State<'_, AppState>, paper_id: String) -> CommandResult<()> {
    let conn = state.db.conn();
    let version = format!("{}-manual-{}", jobs::ANALYZE_VERSION, crate::time::now_iso8601());
    jobs::queue::enqueue(&conn, Some(&paper_id), JobType::Analyze, &version)?;
    drop(conn);

    state.jobs.notify();
    Ok(())
}

#[tauri::command]
pub async fn translate_page(
    app: AppHandle,
    state: State<'_, AppState>,
    paper_id: String,
    page_number: i64,
    target_language: Option<String>,
) -> CommandResult<PageTranslation> {
    let language = match target_language {
        Some(language) => language,
        None => {
            let conn = state.db.conn();
            settings_repo::get(&conn)?.translation_language
        }
    };

    Ok(translation::translate_page(&app, &paper_id, page_number, &language).await?)
}

/// Translates a free-form selection the reader dragged over the page — the
/// "이 부분만 번역" action in the selection toolbar. Ad-hoc and uncached.
#[tauri::command]
pub async fn translate_selection(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    target_language: Option<String>,
) -> CommandResult<String> {
    let language = match target_language {
        Some(language) => language,
        None => {
            let conn = state.db.conn();
            settings_repo::get(&conn)?.translation_language
        }
    };

    Ok(translation::translate_selection(&app, &text, &language).await?)
}

/// Returns a saved translation for a page, or `None` if it was never translated.
/// Read-only: never contacts the provider, so the viewer can restore a prior
/// translation automatically when a page comes into view.
#[tauri::command]
pub fn get_cached_translation(
    app: AppHandle,
    state: State<'_, AppState>,
    paper_id: String,
    page_number: i64,
    target_language: Option<String>,
) -> CommandResult<Option<PageTranslation>> {
    let language = match target_language {
        Some(language) => language,
        None => {
            let conn = state.db.conn();
            settings_repo::get(&conn)?.translation_language
        }
    };

    Ok(translation::load_cached_page(&app, &paper_id, page_number, &language)?)
}
