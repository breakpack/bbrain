use serde::Deserialize;
use tauri::{AppHandle, Manager, State};

use crate::chat::{self, ChatSession, StartChatRequest, StoredMessage};
use crate::db::settings_repo;
use crate::error::CommandResult;
use crate::rag::{self, Scope, SearchHit};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub scope: Scope,
}

#[tauri::command]
pub async fn search_library(
    state: State<'_, AppState>,
    request: SearchRequest,
) -> CommandResult<Vec<SearchHit>> {
    let db = state.db.clone();
    let embedder = state.embedder.clone();

    // Embedding the query is CPU work; keep it off the async runtime.
    let hits = tokio::task::spawn_blocking(move || -> crate::error::Result<Vec<SearchHit>> {
        let conn = db.conn();
        let generation = settings_repo::get(&conn)?.index_generation;
        let candidates = rag::retrieve(&conn, &embedder, &request.query, &request.scope, generation)?;
        rag::to_hits(&conn, &candidates)
    })
    .await
    .map_err(|e| crate::error::AppError::Internal(format!("search task: {e}")))??;

    Ok(hits)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    pub scope: Scope,
    pub title: String,
}

#[tauri::command]
pub fn create_chat_session(app: AppHandle, input: CreateSessionInput) -> CommandResult<String> {
    Ok(chat::create_session(&app, &input.scope, &input.title)?)
}

#[tauri::command]
pub fn list_chat_sessions(state: State<'_, AppState>) -> CommandResult<Vec<ChatSession>> {
    let conn = state.db.conn();
    let mut statement = conn.prepare(
        "SELECT id, title, scope_type, scope_id, updated_at
         FROM chat_sessions ORDER BY updated_at DESC",
    )?;

    let rows = statement.query_map([], |row| {
        Ok(ChatSession {
            id: row.get(0)?,
            title: row.get(1)?,
            scope_type: row.get(2)?,
            scope_id: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[tauri::command]
pub fn list_chat_messages(
    app: AppHandle,
    session_id: String,
) -> CommandResult<Vec<StoredMessage>> {
    Ok(chat::load_messages(&app, &session_id)?)
}

/// Starts the answer and returns immediately. Progress arrives as `chat://delta`
/// events and ends with `chat://completed` or `chat://failed`, so the UI is not
/// blocked on a long generation and `cancel_chat` can stop it mid-stream.
#[tauri::command]
pub fn start_chat(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartChatRequest,
) -> CommandResult<()> {
    let request_id = request.request_id.clone();

    let handle = tauri::async_runtime::spawn({
        let app = app.clone();
        let request_id = request_id.clone();
        async move {
            // Errors are reported to the UI through chat://failed inside
            // start_chat; nothing else can act on them here.
            let _ = chat::start_chat(&app, request).await;

            let state = app.state::<AppState>();
            state.chats.forget(&request_id);
        }
    });

    state.chats.register(&request_id, handle);
    Ok(())
}

#[tauri::command]
pub fn cancel_chat(state: State<'_, AppState>, request_id: String) -> CommandResult<()> {
    state.chats.cancel(&request_id);
    Ok(())
}

#[tauri::command]
pub fn delete_chat_session(state: State<'_, AppState>, session_id: String) -> CommandResult<()> {
    state
        .db
        .conn()
        .execute("DELETE FROM chat_sessions WHERE id = ?1", [&session_id])?;
    Ok(())
}
