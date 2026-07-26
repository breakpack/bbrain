use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::db::{paper_repo, settings_repo};
use crate::error::{AppError, CommandResult, Result};
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

/// One paper's contribution to a concept's accumulated note (§ tag insights).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNoteEntry {
    pub paper_id: String,
    pub paper_title: String,
    pub insight: String,
    pub evidence_pages: Vec<i64>,
    pub updated_at: String,
}

/// The concept note for one tag: every paper's explanation of it, newest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNote {
    pub label: String,
    pub entries: Vec<TagNoteEntry>,
}

/// The "second brain" note for a single concept, keyed by its tag label.
#[tauri::command]
pub fn get_tag_note(state: State<'_, AppState>, label: String) -> CommandResult<Option<TagNote>> {
    Ok(find_tag_note(&state.db.conn(), &label)?)
}

/// Connection-facing core of `get_tag_note`, so the lookup is testable without
/// a running Tauri app. `label` matches `tags.display_name` case-insensitively.
fn find_tag_note(conn: &rusqlite::Connection, label: &str) -> Result<Option<TagNote>> {
    let tag = conn
        .query_row(
            "SELECT id, display_name FROM tags WHERE display_name = ?1 COLLATE NOCASE",
            params![label],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;

    let Some((tag_id, display_name)) = tag else {
        return Ok(None);
    };

    let mut statement = conn.prepare(
        "SELECT e.paper_id, p.title, e.insight_md, e.evidence_pages, e.updated_at
         FROM tag_note_entries e JOIN papers p ON p.id = e.paper_id
         WHERE e.tag_id = ?1
         ORDER BY e.updated_at DESC",
    )?;
    let entries = statement
        .query_map(params![tag_id], |row| {
            let evidence_pages: String = row.get(3)?;
            Ok(TagNoteEntry {
                paper_id: row.get(0)?,
                paper_title: row.get(1)?,
                insight: row.get(2)?,
                evidence_pages: serde_json::from_str(&evidence_pages).unwrap_or_default(),
                updated_at: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Some(TagNote { label: display_name, entries }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn seed_paper(conn: &rusqlite::Connection, id: &str, title: &str) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", title, paper_repo::ImportStatus::Ready)
            .unwrap();
    }

    fn seed_entry(
        conn: &rusqlite::Connection,
        tag_id: &str,
        paper_id: &str,
        insight: &str,
        pages: &str,
        updated_at: &str,
    ) {
        conn.execute(
            "INSERT INTO tag_note_entries (tag_id, paper_id, insight_md, evidence_pages, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![tag_id, paper_id, insight, pages, updated_at],
        )
        .unwrap();
    }

    #[test]
    fn no_matching_tag_returns_none() {
        let db = Database::open_in_memory().unwrap();
        assert!(find_tag_note(&db.conn(), "nonexistent").unwrap().is_none());
    }

    /// label 매칭은 대소문자 무시이고, entries는 updated_at 내림차순으로
    /// paper 제목과 함께 나와야 한다 — IPC 계약의 정렬·조인 부분.
    #[test]
    fn a_tag_note_joins_paper_titles_and_sorts_newest_first() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        seed_paper(&conn, "paper-a", "Paper A");
        seed_paper(&conn, "paper-b", "Paper B");
        let tag_id = paper_repo::upsert_tag(&conn, "RAG", "ai").unwrap();

        seed_entry(&conn, &tag_id, "paper-a", "첫 논문의 설명", "[1,2]", "2024-01-01T00:00:00Z");
        seed_entry(&conn, &tag_id, "paper-b", "둘째 논문의 설명", "[3]", "2024-06-01T00:00:00Z");

        let note = find_tag_note(&conn, "rag").unwrap().expect("tag exists");

        assert_eq!(note.label, "RAG");
        assert_eq!(note.entries.len(), 2);
        assert_eq!(note.entries[0].paper_id, "paper-b");
        assert_eq!(note.entries[0].paper_title, "Paper B");
        assert_eq!(note.entries[0].evidence_pages, vec![3]);
        assert_eq!(note.entries[1].paper_id, "paper-a");
        assert_eq!(note.entries[1].evidence_pages, vec![1, 2]);
    }
}
