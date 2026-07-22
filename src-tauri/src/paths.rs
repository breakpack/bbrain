use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::{AppError, Result};

/// App storage layout from DEVELOPMENT.md §6.3.
#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn resolve(app: &AppHandle) -> Result<Self> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::Internal(format!("app data dir unavailable: {e}")))?;
        Self::new(root)
    }

    pub fn new(root: PathBuf) -> Result<Self> {
        let paths = Self { root };
        for dir in [
            paths.root(),
            &paths.papers_dir(),
            &paths.models_dir(),
            &paths.cache_dir(),
            &paths.pdf_cache_dir(),
            &paths.translation_cache_dir(),
            &paths.logs_dir(),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::Internal(format!("could not create {dir:?}: {e}")))?;
        }
        Ok(paths)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn database(&self) -> PathBuf {
        self.root.join("library.sqlite")
    }

    pub fn papers_dir(&self) -> PathBuf {
        self.root.join("papers")
    }

    /// Managed storage for one paper. `paper_id` is a UUIDv7 we generated, so it
    /// cannot traverse — callers must never pass user-supplied names here.
    pub fn paper_dir(&self, paper_id: &str) -> PathBuf {
        self.papers_dir().join(paper_id)
    }

    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn pdf_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("pdf")
    }

    pub fn translation_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("translation")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_full_storage_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("Bbrain")).unwrap();

        assert!(paths.papers_dir().is_dir());
        assert!(paths.models_dir().is_dir());
        assert!(paths.pdf_cache_dir().is_dir());
        assert!(paths.translation_cache_dir().is_dir());
        assert!(paths.logs_dir().is_dir());
        assert_eq!(paths.database().file_name().unwrap(), "library.sqlite");
    }

    #[test]
    fn paper_dir_stays_under_papers() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let dir = paths.paper_dir("019512b0-0000-7000-8000-000000000000");
        assert!(dir.starts_with(paths.papers_dir()));
    }
}
