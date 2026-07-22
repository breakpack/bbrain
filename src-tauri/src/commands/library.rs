use std::path::PathBuf;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::db::paper_repo::{
    self, Group, GroupPatch, LibraryQuery, Paper, PaperPatch, Tag,
};
use crate::error::CommandResult;
use crate::import::{self, ImportOutcome};
use crate::state::AppState;

/// Emitted after any change to the library so open views refetch. The event
/// carries no data — the DB stays the source of truth (DEVELOPMENT.md §14.5).
pub const LIBRARY_CHANGED: &str = "library://changed";

#[tauri::command]
pub async fn import_papers(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<PathBuf>,
    target_group_id: Option<String>,
) -> CommandResult<Vec<ImportOutcome>> {
    let db = state.db.clone();
    let app_paths = state.paths.clone();
    let group = target_group_id.clone();

    // Hashing and copying are blocking file work; keep them off the async
    // runtime's cooperative threads.
    let outcomes = tokio::task::spawn_blocking(move || {
        let mut conn = db.conn();
        paths
            .iter()
            .map(|path| import::import_one(&mut conn, &app_paths, path, group.as_deref()))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| crate::error::AppError::Internal(format!("import task: {e}")))?;

    let imported: Vec<String> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ImportOutcome::Imported { paper_id, .. } => Some(paper_id.clone()),
            _ => None,
        })
        .collect();

    if !imported.is_empty() {
        let _ = app.emit(LIBRARY_CHANGED, ());
        crate::jobs::enqueue_import_pipeline(&state.db.conn(), &imported)?;
        state.jobs.notify();
    }

    Ok(outcomes)
}

#[tauri::command]
pub fn list_papers(state: State<'_, AppState>, query: LibraryQuery) -> CommandResult<Vec<Paper>> {
    Ok(paper_repo::list(&state.db.conn(), &query)?)
}

#[tauri::command]
pub fn get_paper(state: State<'_, AppState>, paper_id: String) -> CommandResult<Paper> {
    Ok(paper_repo::get(&state.db.conn(), &paper_id)?)
}

#[tauri::command]
pub fn update_paper(
    app: AppHandle,
    state: State<'_, AppState>,
    paper_id: String,
    patch: PaperPatch,
) -> CommandResult<Paper> {
    let paper = paper_repo::update(&mut state.db.conn(), &paper_id, &patch)?;
    let _ = app.emit(LIBRARY_CHANGED, ());
    Ok(paper)
}

#[tauri::command]
pub fn delete_paper(
    app: AppHandle,
    state: State<'_, AppState>,
    paper_id: String,
    delete_managed_file: bool,
) -> CommandResult<()> {
    let managed_path = paper_repo::delete(&state.db.conn(), &paper_id)?;

    if delete_managed_file {
        // The row is already gone; a failed unlink leaves recoverable bytes, not
        // a broken library, so it is logged rather than surfaced.
        let dir = state.paths.paper_dir(&paper_id);
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(?dir, %error, "could not remove managed paper directory");
        }
        let _ = managed_path;
    }

    let _ = app.emit(LIBRARY_CHANGED, ());
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInput {
    pub name: String,
    pub color: Option<String>,
}

#[tauri::command]
pub fn create_group(
    app: AppHandle,
    state: State<'_, AppState>,
    input: CreateGroupInput,
) -> CommandResult<String> {
    let id = paper_repo::create_group(&state.db.conn(), &input.name, input.color.as_deref())?;
    let _ = app.emit(LIBRARY_CHANGED, ());
    Ok(id)
}

#[tauri::command]
pub fn list_groups(state: State<'_, AppState>) -> CommandResult<Vec<Group>> {
    Ok(paper_repo::list_groups(&state.db.conn())?)
}

#[tauri::command]
pub fn update_group(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    patch: GroupPatch,
) -> CommandResult<()> {
    paper_repo::update_group(&state.db.conn(), &group_id, &patch)?;
    let _ = app.emit(LIBRARY_CHANGED, ());
    Ok(())
}

#[tauri::command]
pub fn delete_group(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: String,
) -> CommandResult<()> {
    paper_repo::delete_group(&state.db.conn(), &group_id)?;
    let _ = app.emit(LIBRARY_CHANGED, ());
    Ok(())
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> CommandResult<Vec<Tag>> {
    Ok(paper_repo::all_tags(&state.db.conn())?)
}
