pub mod analysis;
pub mod chat;
pub mod commands;
pub mod db;
pub mod discover;
pub mod error;
pub mod ids;
pub mod import;
pub mod insight;
pub mod jobs;
pub mod logging;
pub mod neighborhood;
pub mod obsidian;
pub mod paths;
pub mod providers;
pub mod rag;
pub mod relations;
pub mod secrets;
pub mod state;
pub mod time;
pub mod topics;
pub mod translation;

use std::sync::Arc;

use tauri::Manager;

use crate::db::Database;
use crate::paths::AppPaths;
use crate::secrets::OsCredentialStore;
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle();
            let paths = AppPaths::resolve(handle)?;

            // The guard must outlive setup or the appender stops flushing.
            if let Some(guard) = logging::init(&paths.logs_dir()) {
                app.manage(guard);
            }

            let db = Database::open(&paths.database())?;

            // A previous run may have died mid-job. Its result was never
            // committed, so the work is safe to run again (DEVELOPMENT.md §10.1).
            jobs::queue::recover_running(&db.conn())?;

            // Keychain denials, network outages and since-fixed storage bugs
            // are gone after a relaunch — give those failures one fresh try
            // per launch instead of leaving papers stuck in waiting_for_ai.
            jobs::queue::recover_transient_failures(&db.conn())?;
            jobs::queue::release_waiting_for_key(&db.conn())?;

            // After an extraction-format change, re-extract existing papers once
            // so a fix (e.g. two-column reading order) reaches papers imported
            // before it. Idempotency-gated, so this is a no-op on later launches.
            jobs::reprocess_existing_papers(&db.conn())?;

            let vault_path = db::settings_repo::get(&db.conn())?.obsidian_vault_path;

            app.manage(AppState::new(
                Arc::new(db),
                paths,
                Arc::new(OsCredentialStore::new()),
            ));

            // Resume watching a vault configured in a previous session.
            if let Some(vault) = vault_path {
                obsidian::watch::spawn(handle.clone(), std::path::PathBuf::from(vault));
            }

            jobs::runner::spawn(handle.clone());

            // Warm the embedding model off the main thread so the first paper's
            // indexing is not blocked by the one-time model load.
            {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    handle.state::<AppState>().embedder.preload();
                });
            }

            tracing::info!("bbrain core ready");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::provider::configure_provider,
            commands::provider::validate_provider,
            commands::provider::list_provider_models,
            commands::provider::remove_provider,
            commands::library::import_papers,
            discover::search_papers,
            discover::import_discovered_paper,
            discover::configure_semantic_scholar,
            discover::remove_semantic_scholar,
            commands::library::list_papers,
            commands::library::get_paper,
            commands::library::update_paper,
            commands::library::delete_paper,
            commands::library::create_group,
            commands::library::list_groups,
            commands::library::update_group,
            commands::library::delete_group,
            commands::library::list_tags,
            commands::viewer::get_viewer_document,
            commands::viewer::read_paper_bytes,
            commands::viewer::read_thumbnail,
            commands::viewer::get_page_sentences,
            commands::viewer::submit_extraction,
            commands::viewer::submit_thumbnail,
            commands::viewer::report_extraction_failure,
            commands::viewer::list_highlights,
            commands::viewer::save_highlight,
            commands::viewer::update_highlight,
            commands::viewer::delete_highlight,
            commands::jobs::list_blocked_jobs,
            commands::jobs::retry_job,
            commands::jobs::cancel_job,
            commands::jobs::pending_job_count,
            commands::ai::get_analysis,
            commands::ai::reanalyze_paper,
            commands::ai::translate_page,
            commands::ai::translate_selection,
            commands::ai::get_cached_translation,
            commands::ai::explain_selection,
            commands::ai::summarize_highlights,
            commands::chat::search_library,
            commands::chat::create_chat_session,
            commands::chat::list_chat_sessions,
            commands::chat::list_chat_messages,
            commands::chat::start_chat,
            commands::chat::cancel_chat,
            commands::chat::delete_chat_session,
            commands::graph::get_graph,
            commands::graph::get_paper_neighborhood,
            commands::graph::get_topic_graph,
            commands::graph::get_tag_note,
            commands::graph::add_manual_relation,
            commands::graph::remove_manual_relation,
            commands::graph::configure_obsidian,
            commands::graph::configure_obsidian_rest,
            commands::graph::obsidian_rest_status,
            commands::graph::export_graph_to_obsidian,
            commands::graph::sync_obsidian,
            commands::graph::list_sync_records,
        ])
        .run(tauri::generate_context!())
        .expect("error while running bbrain");
}
