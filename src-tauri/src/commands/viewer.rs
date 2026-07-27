use serde::{Deserialize, Serialize};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

use crate::db::highlight_repo::{self, Highlight, HighlightPatch, SaveHighlightInput};
use crate::db::page_repo::{self, ExtractedPage, PageInfo, Sentence};
use crate::db::paper_repo::{self, ImportStatus, Paper};
use crate::error::{AppError, CommandResult};
use crate::jobs::{self, JobType};
use crate::state::AppState;

pub const PAPER_CHANGED: &str = "paper://changed";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerDocument {
    pub paper: Paper,
    pub pages: Vec<PageInfo>,
    pub has_text_layer: bool,
}

#[tauri::command]
pub fn get_viewer_document(
    state: State<'_, AppState>,
    paper_id: String,
) -> CommandResult<ViewerDocument> {
    let conn = state.db.conn();
    paper_repo::touch_opened(&conn, &paper_id)?;

    let paper = paper_repo::get(&conn, &paper_id)?;
    let page_count = paper.page_count.unwrap_or(0);
    let pages = (1..=page_count)
        .filter_map(|number| page_repo::page_info(&conn, &paper_id, number).ok().flatten())
        .collect();

    Ok(ViewerDocument {
        has_text_layer: page_repo::has_text_layer(&conn, &paper_id)?,
        paper,
        pages,
    })
}

/// Streams the managed PDF as raw bytes. Returning `Response` keeps them out of
/// JSON — a 20MB paper would otherwise be serialized as an array of numbers.
#[tauri::command]
pub fn read_paper_bytes(state: State<'_, AppState>, paper_id: String) -> CommandResult<Response> {
    let path = {
        let conn = state.db.conn();
        paper_repo::managed_path(&conn, &paper_id)?
    };

    // The path comes from our own row, but re-check it lives inside managed
    // storage: a tampered DB must not turn into an arbitrary file read.
    let path = std::path::PathBuf::from(path);
    if !path.starts_with(state.paths.papers_dir()) {
        return Err(AppError::InvalidInput("managed path escapes app storage".into()).into());
    }

    let bytes = std::fs::read(&path)
        .map_err(|_| AppError::Rejected(crate::import::pdf_file::RejectReason::Unreadable))?;

    Ok(Response::new(bytes))
}

#[tauri::command]
pub fn get_page_sentences(
    state: State<'_, AppState>,
    paper_id: String,
    page_number: i64,
) -> CommandResult<Vec<Sentence>> {
    Ok(page_repo::page_sentences(&state.db.conn(), &paper_id, page_number)?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitExtractionInput {
    pub job_id: String,
    pub paper_id: String,
    pub pages: Vec<ExtractedPage>,
    /// Title found inside the PDF (metadata or page-1 layout), when one could
    /// be detected with confidence. Absent → the current title stands.
    #[serde(default)]
    pub detected_title: Option<String>,
}

/// The webview's PDF.js finished extracting. Persist it, then let the runner's
/// waiting job complete and the pipeline move on.
#[tauri::command]
pub fn submit_extraction(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SubmitExtractionInput,
) -> CommandResult<()> {
    let page_count = input.pages.len() as i64;

    let result = (|| -> crate::error::Result<()> {
        let mut conn = state.db.conn();
        page_repo::replace_extraction(&mut conn, &input.paper_id, &input.pages)?;
        paper_repo::set_page_count(&conn, &input.paper_id, page_count)?;

        // The filename is only a placeholder — the paper's own title replaces
        // it (never a title the user typed; see `apply_detected_title`).
        if let Some(title) = input.detected_title.as_deref() {
            paper_repo::apply_detected_title(&conn, &input.paper_id, title)?;
        }

        // A scan with no text layer can still be read, but translation, analysis
        // and RAG have nothing to work on — mark it partial rather than failed.
        if page_repo::has_text_layer(&conn, &input.paper_id)? {
            paper_repo::set_status(&conn, &input.paper_id, ImportStatus::Indexing)?;
            jobs::queue::enqueue(
                &conn,
                Some(&input.paper_id),
                JobType::Embed,
                jobs::EMBED_VERSION,
            )?;

            // Re-extraction of an already-analysed paper (e.g. after an
            // extraction fix) refreshes the text and embeddings but keeps the
            // existing analysis — re-analysing every paper would spend the
            // provider budget without the user asking. A fresh import has no
            // analysis yet, so it still runs. The embed step returns the paper
            // to `ready` when no analysis is pending.
            let has_analysis: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM analyses WHERE paper_id = ?1)",
                [&input.paper_id],
                |row| row.get::<_, i64>(0),
            )? == 1;

            if !has_analysis {
                jobs::queue::enqueue(
                    &conn,
                    Some(&input.paper_id),
                    JobType::Analyze,
                    jobs::ANALYZE_VERSION,
                )?;
            }
        } else {
            paper_repo::set_status(&conn, &input.paper_id, ImportStatus::Partial)?;
            tracing::info!(paper = %input.paper_id, "no text layer; skipping ai and rag steps");
        }
        Ok(())
    })();

    let outcome = result.map_err(|error| {
        tracing::warn!(code = ?error.code(), "could not persist extraction");
        error
    });
    let failed = outcome.is_err();

    state.jobs.resolve(&input.job_id, outcome.map_err(Into::into));
    state.jobs.notify();

    if failed {
        return Err(AppError::Storage(rusqlite::Error::InvalidQuery).into());
    }

    let _ = app.emit(PAPER_CHANGED, &input.paper_id);
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitThumbnailInput {
    pub job_id: String,
    pub paper_id: String,
    /// PNG bytes. WKWebView cannot encode WebP — `toBlob('image/webp')` silently
    /// returns PNG there — so the webview always sends PNG and Bbrain stores PNG,
    /// keeping macOS and Windows byte-identical.
    pub png: Vec<u8>,
}

#[tauri::command]
pub fn submit_thumbnail(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SubmitThumbnailInput,
) -> CommandResult<()> {
    let path = state.paths.paper_dir(&input.paper_id).join("thumbnail.png");

    let result = std::fs::create_dir_all(state.paths.paper_dir(&input.paper_id))
        .and_then(|_| std::fs::write(&path, &input.png))
        .map_err(|e| AppError::Internal(format!("write thumbnail: {e}")));

    let failed = result.is_err();
    state.jobs.resolve(&input.job_id, result.map_err(Into::into));
    state.jobs.notify();

    if failed {
        return Err(AppError::Internal("thumbnail write failed".into()).into());
    }

    let _ = app.emit(PAPER_CHANGED, &input.paper_id);
    Ok(())
}

#[tauri::command]
pub fn read_thumbnail(state: State<'_, AppState>, paper_id: String) -> CommandResult<Response> {
    let path = state.paths.paper_dir(&paper_id).join("thumbnail.png");
    let bytes = std::fs::read(&path).unwrap_or_default();
    Ok(Response::new(bytes))
}

/// The webview could not process this paper (corrupt, encrypted, or PDF.js threw).
/// `reason` is the webview's own error text — it names the failure in the log,
/// which is the only place a frontend-side fault would otherwise be visible.
#[tauri::command]
pub fn report_extraction_failure(
    state: State<'_, AppState>,
    job_id: String,
    paper_id: String,
    reason: Option<String>,
) -> CommandResult<()> {
    tracing::warn!(
        paper = %paper_id,
        reason = reason.as_deref().unwrap_or("unknown"),
        "the webview could not process this pdf"
    );

    {
        let conn = state.db.conn();
        paper_repo::set_status(&conn, &paper_id, ImportStatus::Failed)?;
    }

    state.jobs.resolve(
        &job_id,
        Err(AppError::Rejected(
            crate::import::pdf_file::RejectReason::Corrupt,
        )),
    );
    state.jobs.notify();
    Ok(())
}

// --- highlights -------------------------------------------------------------

#[tauri::command]
pub fn list_highlights(
    state: State<'_, AppState>,
    paper_id: String,
) -> CommandResult<Vec<Highlight>> {
    Ok(highlight_repo::list(&state.db.conn(), &paper_id)?)
}

#[tauri::command]
pub fn save_highlight(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SaveHighlightInput,
) -> CommandResult<Vec<String>> {
    let paper_id = input.paper_id.clone();
    let ids = highlight_repo::save(&mut state.db.conn(), &input)?;
    let _ = app.emit(PAPER_CHANGED, &paper_id);
    Ok(ids)
}

#[tauri::command]
pub fn update_highlight(
    state: State<'_, AppState>,
    highlight_id: String,
    patch: HighlightPatch,
) -> CommandResult<()> {
    highlight_repo::update(&state.db.conn(), &highlight_id, &patch)?;
    Ok(())
}

#[tauri::command]
pub fn delete_highlight(state: State<'_, AppState>, highlight_id: String) -> CommandResult<()> {
    highlight_repo::delete(&state.db.conn(), &highlight_id)?;
    Ok(())
}
