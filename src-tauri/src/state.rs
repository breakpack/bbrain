use std::collections::HashMap;
use std::sync::{Arc, Mutex};


use crate::db::Database;
use crate::jobs::JobRunner;
use crate::paths::AppPaths;
use crate::rag::embedder::Embedder;
use crate::secrets::CredentialStore;

/// Everything a command needs. The credential store is boxed behind a trait so
/// tests can swap in an in-memory implementation instead of the real keychain.
pub struct AppState {
    pub db: Arc<Database>,
    pub paths: AppPaths,
    pub credentials: Arc<dyn CredentialStore>,
    pub jobs: Arc<JobRunner>,
    pub embedder: Arc<Embedder>,
    pub chats: Arc<ChatRegistry>,
}

impl AppState {
    pub fn new(db: Arc<Database>, paths: AppPaths, credentials: Arc<dyn CredentialStore>) -> Self {
        let embedder = Arc::new(Embedder::new(paths.models_dir()));

        Self {
            db,
            paths,
            credentials,
            jobs: Arc::new(JobRunner::new()),
            embedder,
            chats: Arc::new(ChatRegistry::default()),
        }
    }
}

/// In-flight chat requests, so `cancel_chat` can stop a stream the user has
/// walked away from. Aborting the task drops the delta receiver, which is what
/// tells the provider adapter to stop reading the response.
#[derive(Default)]
pub struct ChatRegistry {
    handles: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

impl ChatRegistry {
    pub fn register(&self, request_id: &str, handle: tauri::async_runtime::JoinHandle<()>) {
        self.handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id.to_string(), handle);
    }

    pub fn forget(&self, request_id: &str) {
        self.handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);
    }

    pub fn cancel(&self, request_id: &str) {
        if let Some(handle) = self
            .handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id)
        {
            handle.abort();
        }
    }
}
