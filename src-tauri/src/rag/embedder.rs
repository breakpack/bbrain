use std::path::PathBuf;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::error::{AppError, Result};

pub const MODEL_ID: &str = "intfloat/multilingual-e5-small";
pub const DIMENSION: usize = 384;
pub const MAX_INPUT_TOKENS: usize = 512;

/// e5 requires these prefixes; fastembed does not add them (§5.2).
pub const PASSAGE_PREFIX: &str = "passage: ";
pub const QUERY_PREFIX: &str = "query: ";

/// Loaded lazily: the model is downloaded on first use, not shipped in the
/// installer, so nothing blocks startup for a user with no library yet.
pub struct Embedder {
    cache_dir: PathBuf,
    model: Mutex<Option<TextEmbedding>>,
}

impl Embedder {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            model: Mutex::new(None),
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.model
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Downloads the model if it is not on disk yet. Blocking and CPU-heavy —
    /// callers run it on a blocking task.
    fn ensure_loaded(&self) -> Result<()> {
        let mut guard = self.model.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }

        let started = std::time::Instant::now();
        tracing::info!(model = MODEL_ID, "loading local embedding model");

        // Use the physical cores for ONNX intra-op parallelism; leave one for the
        // UI and job runner so a big paper does not stall the app.
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(1))
            .unwrap_or(2);

        // The default cache dir is a *relative* path, which resolves against the
        // bundle's working directory. Always pass the app data dir explicitly.
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_cache_dir(self.cache_dir.clone())
                .with_show_download_progress(false)
                .with_max_length(MAX_INPUT_TOKENS)
                .with_intra_threads(threads),
        )
        .map_err(|error| {
            tracing::warn!(%error, "embedding model unavailable");
            AppError::EmbeddingModelUnavailable
        })?;

        tracing::info!(elapsed_ms = started.elapsed().as_millis(), threads, "embedding model ready");
        *guard = Some(model);
        Ok(())
    }

    /// Loads the model now if it is not already loaded. Called in the background
    /// at startup so the first paper's indexing does not pay the one-time load
    /// cost (~10s) synchronously.
    pub fn preload(&self) {
        if let Err(error) = self.ensure_loaded() {
            tracing::info!(code = ?error.code(), "embedding model not preloaded; will load on first use");
        }
    }

    pub fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|text| format!("{PASSAGE_PREFIX}{text}"))
            .collect();
        self.embed(prefixed)
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut vectors = self.embed(vec![format!("{QUERY_PREFIX}{text}")])?;
        vectors
            .pop()
            .ok_or_else(|| AppError::Internal("embedding returned nothing".into()))
    }

    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.ensure_loaded()?;

        let mut guard = self.model.lock().unwrap_or_else(|e| e.into_inner());
        let model = guard
            .as_mut()
            .ok_or_else(|| AppError::Internal("embedding model not loaded".into()))?;

        let vectors = model
            .embed(texts, None)
            .map_err(|error| AppError::Internal(format!("embedding failed: {error}")))?;

        for vector in &vectors {
            if vector.len() != DIMENSION {
                return Err(AppError::Internal(format!(
                    "embedding has {} dimensions, expected {DIMENSION}",
                    vector.len()
                )));
            }
        }

        Ok(vectors)
    }
}

/// f32 little-endian bytes: the blob layout sqlite-vec expects.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|value| value.to_le_bytes()).collect()
}

pub fn from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect()
}

/// fastembed L2-normalizes its output, so cosine similarity is a dot product.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_round_trips_a_vector() {
        let vector = vec![0.5f32, -0.25, 0.125];

        assert_eq!(from_blob(&to_blob(&vector)), vector);
        assert_eq!(to_blob(&vector).len(), 12);
    }

    #[test]
    fn cosine_of_identical_unit_vectors_is_one() {
        let vector = vec![0.6f32, 0.8];

        assert!((cosine(&vector, &vector) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn e5_prefixes_are_applied_because_fastembed_does_not_add_them() {
        assert_eq!(PASSAGE_PREFIX, "passage: ");
        assert_eq!(QUERY_PREFIX, "query: ");
    }
}
