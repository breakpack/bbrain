use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::db::{paper_repo, settings_repo};
use crate::error::{AppError, CommandResult};
use crate::jobs::{self, JobType};
use crate::neighborhood::{self, PaperNeighborhood};
use crate::relations::{self, Graph};
use crate::state::AppState;
use crate::topics::{self, TopicGraph};

#[tauri::command]
pub fn get_graph(state: State<'_, AppState>) -> CommandResult<Graph> {
    Ok(relations::load_graph(&state.db.conn())?)
}

/// The ConnectedPapers-style focus graph for a single paper: its nearest
/// neighbours by embedding similarity, the similarity/citation edges among them,
/// and each paper's year so the UI can lay precedent below and derivative above.
#[tauri::command]
pub fn get_paper_neighborhood(
    state: State<'_, AppState>,
    paper_id: String,
) -> CommandResult<PaperNeighborhood> {
    Ok(neighborhood::load(&state.db.conn(), &paper_id)?)
}

/// The topic graph (the "second brain"). Rebuilt from the AI analyses when they
/// have changed since the last build, or when `rebuild` forces it.
#[tauri::command]
pub async fn get_topic_graph(app: AppHandle, rebuild: Option<bool>) -> CommandResult<TopicGraph> {
    Ok(topics::ensure_and_load(&app, rebuild.unwrap_or(false)).await?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualRelationInput {
    pub source_paper_id: String,
    pub target_paper_id: String,
}

#[tauri::command]
pub fn add_manual_relation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ManualRelationInput,
) -> CommandResult<()> {
    if input.source_paper_id == input.target_paper_id {
        return Err(AppError::InvalidInput("a paper cannot link to itself".into()).into());
    }

    relations::add_manual_relation(
        &state.db.conn(),
        &input.source_paper_id,
        &input.target_paper_id,
    )?;
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

#[tauri::command]
pub fn remove_manual_relation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ManualRelationInput,
) -> CommandResult<()> {
    relations::remove_manual_relation(
        &state.db.conn(),
        &input.source_paper_id,
        &input.target_paper_id,
    )?;
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureObsidianInput {
    /// Absolute path to an existing vault, chosen by the user through the native
    /// picker — the webview never gets filesystem scope of its own.
    pub vault_path: String,
}

#[tauri::command]
pub fn configure_obsidian(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ConfigureObsidianInput,
) -> CommandResult<()> {
    let vault = std::path::PathBuf::from(&input.vault_path);
    if !vault.is_dir() {
        return Err(AppError::VaultUnavailable("경로가 폴더가 아닙니다".into()).into());
    }

    {
        let conn = state.db.conn();
        settings_repo::update(
            &conn,
            &settings_repo::SettingsPatch {
                obsidian_vault_path: Some(input.vault_path.clone()),
                ..Default::default()
            },
        )?;

        // Every analysed paper gets a note; papers still processing pick one up
        // when their analysis lands.
        let papers = paper_repo::list(&conn, &paper_repo::LibraryQuery::default())?;
        for paper in papers {
            jobs::queue::enqueue(
                &conn,
                Some(&paper.id),
                JobType::ObsidianSync,
                jobs::OBSIDIAN_VERSION,
            )?;
        }
    }

    crate::obsidian::watch::spawn(app.clone(), vault);
    state.jobs.notify();
    Ok(())
}

/// Exports the topic graph to the configured Obsidian vault as linked notes so
/// its Graph View shows the concept map. Returns the number of topic notes.
#[tauri::command]
pub async fn export_graph_to_obsidian(app: AppHandle) -> CommandResult<usize> {
    Ok(crate::obsidian::export_topic_graph(&app).await?)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureObsidianRestInput {
    /// Local REST API endpoint, e.g. `https://127.0.0.1:27124`. An empty
    /// string disconnects and removes the stored key.
    pub url: String,
    /// Sent once, written straight to the OS credential store (§16.1). Omitted
    /// when only re-testing the connection with the already-stored key.
    pub api_key: Option<String>,
}

/// Connects Bbrain to Obsidian's Local REST API (the channel MCP servers use).
/// Stores the key, saves the URL, and reports the live connection state.
#[tauri::command]
pub async fn configure_obsidian_rest(
    state: State<'_, AppState>,
    input: ConfigureObsidianRestInput,
) -> CommandResult<crate::obsidian::rest::RestHealth> {
    use crate::obsidian::rest;

    let url = input.url.trim().to_string();

    if url.is_empty() {
        let conn = state.db.conn();
        crate::secrets::delete_obsidian_rest_key()?;
        settings_repo::update(
            &conn,
            &settings_repo::SettingsPatch {
                obsidian_rest_url: Some(String::new()),
                ..Default::default()
            },
        )?;
        settings_repo::set_obsidian_rest_credential_ref(&conn, None)?;
        return Ok(rest::RestHealth::Unreachable);
    }

    {
        let conn = state.db.conn();
        settings_repo::update(
            &conn,
            &settings_repo::SettingsPatch {
                obsidian_rest_url: Some(url.clone()),
                ..Default::default()
            },
        )?;
        if let Some(api_key) = &input.api_key {
            let reference = crate::secrets::set_obsidian_rest_key(api_key)?;
            settings_repo::set_obsidian_rest_credential_ref(&conn, Some(&reference))?;
        }
    }

    let Some(config) = ({
        let conn = state.db.conn();
        rest::load(&conn)?
    }) else {
        // URL saved but no key stored yet.
        return Ok(rest::RestHealth::Unauthorized);
    };

    Ok(rest::health(&config).await)
}

/// Live connection state of the configured endpoint, for the settings page.
#[tauri::command]
pub async fn obsidian_rest_status(
    state: State<'_, AppState>,
) -> CommandResult<Option<crate::obsidian::rest::RestHealth>> {
    use crate::obsidian::rest;

    let config = {
        let conn = state.db.conn();
        rest::load(&conn)?
    };
    match config {
        Some(config) => Ok(Some(rest::health(&config).await)),
        None => Ok(None),
    }
}

/// Re-syncs everything, or one paper.
#[tauri::command]
pub fn sync_obsidian(state: State<'_, AppState>, paper_id: Option<String>) -> CommandResult<()> {
    let conn = state.db.conn();

    let version = format!("{}-manual-{}", jobs::OBSIDIAN_VERSION, crate::time::now_iso8601());
    match paper_id {
        Some(paper_id) => {
            jobs::queue::enqueue(&conn, Some(&paper_id), JobType::ObsidianSync, &version)?;
        }
        None => {
            for paper in paper_repo::list(&conn, &paper_repo::LibraryQuery::default())? {
                jobs::queue::enqueue(&conn, Some(&paper.id), JobType::ObsidianSync, &version)?;
            }
        }
    }
    drop(conn);

    state.jobs.notify();
    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    pub paper_id: String,
    pub paper_title: String,
    pub vault_path: String,
    pub status: String,
    pub updated_at: String,
}

#[tauri::command]
pub fn list_sync_records(state: State<'_, AppState>) -> CommandResult<Vec<SyncRecord>> {
    let conn = state.db.conn();
    let mut statement = conn.prepare(
        "SELECT s.paper_id, p.title, s.vault_path, s.status, s.updated_at
         FROM sync_records s JOIN papers p ON p.id = s.paper_id
         ORDER BY s.updated_at DESC",
    )?;

    let rows = statement.query_map([], |row| {
        Ok(SyncRecord {
            paper_id: row.get(0)?,
            paper_title: row.get(1)?,
            vault_path: row.get(2)?,
            status: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
