use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use rusqlite::params;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::paper_repo;
use crate::error::Result;
use crate::state::AppState;

use super::{note, SYNC_STATUS};

/// External edits arrive in bursts (Obsidian writes, then re-writes). Wait for
/// quiet before reading (DEVELOPMENT.md §13.4).
const DEBOUNCE: Duration = Duration::from_millis(750);

/// Watches only the `Bbrain/Papers` folder of the configured vault — never the
/// whole vault (§13.4).
pub fn spawn(app: AppHandle, vault: PathBuf) {
    std::thread::spawn(move || {
        let notes_dir = vault.join("Bbrain").join("Papers");
        if !notes_dir.is_dir() {
            return;
        }

        let (sender, receiver) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match notify::recommended_watcher(sender) {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::warn!(%error, "could not start the vault watcher");
                return;
            }
        };

        if let Err(error) = watcher.watch(&notes_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(%error, "could not watch the vault notes directory");
            return;
        }

        tracing::info!(?notes_dir, "watching the vault");

        let mut pending: Vec<PathBuf> = Vec::new();
        loop {
            match receiver.recv_timeout(DEBOUNCE) {
                Ok(Ok(event)) => {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        for path in event.paths {
                            if path.extension().is_some_and(|ext| ext == "md")
                                && !pending.contains(&path)
                            {
                                pending.push(path);
                            }
                        }
                    }
                }
                Ok(Err(error)) => tracing::warn!(%error, "vault watch error"),

                // Quiet period: apply whatever accumulated.
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    for path in pending.drain(..) {
                        if let Err(error) = pull_note(&app, &path) {
                            tracing::warn!(code = ?error.code(), ?path, "could not read vault note");
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

/// Pulls the bidirectional fields (groups, tags, user block) back into the app.
/// A note is tracked by `bbrain_id`, not by file name, so moves and renames in
/// the vault are followed rather than treated as new papers (§13.4).
pub fn pull_note(app: &AppHandle, path: &Path) -> Result<()> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(());
    };

    let parsed = match note::parse(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            // Broken markers: stop touching this note and tell the user (§13.4).
            mark_conflict(app, &content, path)?;
            return Err(error);
        }
    };

    let Some(paper_id) = parsed.frontmatter.get("bbrain_id").cloned() else {
        return Ok(());
    };

    let state = app.state::<AppState>();

    // Deleting a note in Obsidian must never delete the paper (§13.4).
    let exists: bool = state
        .db
        .conn()
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM papers WHERE id = ?1)",
            params![paper_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
        .unwrap_or(false);
    if !exists {
        return Ok(());
    }

    let tags = note::list_field(&parsed.frontmatter_lines, "tags");
    let group_names = note::list_field(&parsed.frontmatter_lines, "groups");

    {
        let mut conn = state.db.conn();

        // Groups are user-owned in both directions; a group named in the vault
        // that the app does not have yet is created rather than dropped.
        let mut group_ids = Vec::new();
        for name in &group_names {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM groups WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .ok();

            let id = match existing {
                Some(id) => id,
                None => paper_repo::create_group(&conn, name, None)?,
            };
            group_ids.push(id);
        }

        paper_repo::update(
            &mut conn,
            &paper_id,
            &paper_repo::PaperPatch {
                tags: Some(tags),
                group_ids: Some(group_ids),
                ..Default::default()
            },
        )?;

        conn.execute(
            "UPDATE sync_records SET vault_revision = ?1, status = 'synced', updated_at = ?2
             WHERE paper_id = ?3",
            params![
                super::content_hash(&content),
                crate::time::now_iso8601(),
                paper_id
            ],
        )?;
    }

    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    let _ = app.emit(SYNC_STATUS, &paper_id);
    Ok(())
}

fn mark_conflict(app: &AppHandle, content: &str, path: &Path) -> Result<()> {
    // The id may still be readable even when the markers are broken.
    let id = content
        .lines()
        .find_map(|line| line.strip_prefix("bbrain_id:"))
        .map(|value| value.trim().trim_matches('"').to_string());

    if let Some(paper_id) = id {
        let state = app.state::<AppState>();
        state.db.conn().execute(
            "UPDATE sync_records SET status = 'conflict', updated_at = ?1 WHERE paper_id = ?2",
            params![crate::time::now_iso8601(), paper_id],
        )?;
        let _ = app.emit(SYNC_STATUS, &paper_id);
    }

    tracing::warn!(?path, "vault note has damaged bbrain markers; sync paused for it");
    Ok(())
}
