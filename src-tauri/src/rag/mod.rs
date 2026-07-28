pub mod chunk;
pub mod embedder;
pub mod search;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{page_repo, paper_repo, settings_repo};
use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::state::AppState;
use embedder::{to_blob, Embedder};
use search::Candidate;

/// Where a search may look (DEVELOPMENT.md §11.2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "id")]
pub enum Scope {
    Paper(String),
    Group(String),
    Library,
}

impl Default for Scope {
    fn default() -> Self {
        Self::Library
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: String,
    pub paper_id: String,
    pub paper_title: String,
    pub page_start: i64,
    pub page_end: i64,
    pub section: Option<String>,
    pub text: String,
}

/// Embeds one paper: chunk, embed, write both the vector index and the FTS row.
pub async fn embed_paper(app: &AppHandle, paper_id: &str) -> Result<()> {
    let paper_id = paper_id.to_string();
    let app = app.clone();

    // Embedding is CPU-bound; keep it off the async runtime's threads.
    tokio::task::spawn_blocking(move || embed_paper_blocking(&app, &paper_id))
        .await
        .map_err(|e| AppError::Internal(format!("embed task: {e}")))?
}

fn embed_paper_blocking(app: &AppHandle, paper_id: &str) -> Result<()> {
    let state = app.state::<AppState>();

    let (pages, generation) = {
        let conn = state.db.conn();
        let pages = page_repo::document_text(&conn, paper_id)?;
        let generation = settings_repo::get(&conn)?.index_generation;
        (pages, generation)
    };

    let chunks = chunk::chunk_document(&pages);
    if chunks.is_empty() {
        tracing::info!(paper = %paper_id, "nothing to embed");
        return Ok(());
    }

    let texts: Vec<String> = chunks.iter().map(|chunk| chunk.text.clone()).collect();
    let vectors = state.embedder.embed_passages(&texts)?;

    {
        let mut conn = state.db.conn();
        let tx = conn.transaction()?;

        // Re-embedding replaces the previous vectors for this paper rather than
        // accumulating stale ones.
        let existing: Vec<String> = {
            let mut statement = tx.prepare("SELECT id FROM chunks WHERE paper_id = ?1")?;
            let rows = statement.query_map(params![paper_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for chunk_id in &existing {
            tx.execute(
                "DELETE FROM chunk_vectors WHERE chunk_id = ?1 AND index_generation = ?2",
                params![chunk_id, generation],
            )?;
            tx.execute(
                "DELETE FROM chunks_fts WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
        }
        tx.execute("DELETE FROM chunks WHERE paper_id = ?1", params![paper_id])?;

        for (chunk, vector) in chunks.iter().zip(&vectors) {
            let chunk_id = new_id();

            tx.execute(
                "INSERT INTO chunks
                   (id, paper_id, section, page_start, page_end, text, token_count, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    chunk_id,
                    paper_id,
                    chunk.section,
                    chunk.page_start,
                    chunk.page_end,
                    chunk.text,
                    chunk.token_count,
                    chunk.content_hash
                ],
            )?;

            tx.execute(
                "INSERT INTO chunk_vectors (chunk_id, index_generation, embedding)
                 VALUES (?1, ?2, ?3)",
                params![chunk_id, generation, to_blob(vector)],
            )?;

            tx.execute(
                "INSERT INTO chunks_fts (chunk_id, paper_id, text) VALUES (?1, ?2, ?3)",
                params![chunk_id, paper_id, chunk.text],
            )?;
        }

        tx.commit()?;
    }

    // The paper-level vector drives semantic relations (§12.1). It is the mean
    // of the chunk vectors, renormalized.
    let paper_vector = mean_normalized(&vectors);
    {
        let conn = state.db.conn();
        conn.execute(
            "DELETE FROM paper_vectors WHERE paper_id = ?1 AND index_generation = ?2",
            params![paper_id, generation],
        )?;
        conn.execute(
            "INSERT INTO paper_vectors (paper_id, index_generation, embedding)
             VALUES (?1, ?2, ?3)",
            params![paper_id, generation, to_blob(&paper_vector)],
        )?;
    }

    // Return the paper to `ready` if it was only indexing and no analysis is
    // still pending — the case after a re-extraction that keeps the existing
    // analysis. During a fresh import an analyze job is queued, so this leaves
    // the status alone and lets analysis flip it to `ready` instead.
    {
        let conn = state.db.conn();
        let paper = paper_repo::get(&conn, paper_id)?;
        if paper.import_status == paper_repo::ImportStatus::Indexing {
            let analyze_pending: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM jobs
                                WHERE paper_id = ?1 AND type = 'analyze'
                                  AND status IN ('queued', 'running'))",
                [paper_id],
                |row| row.get::<_, i64>(0),
            )? == 1;
            if !analyze_pending {
                paper_repo::set_status(&conn, paper_id, paper_repo::ImportStatus::Ready)?;
            }
        }
    }

    tracing::info!(paper = %paper_id, chunks = chunks.len(), "embedded paper");
    let _ = app.emit(crate::commands::viewer::PAPER_CHANGED, paper_id);
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

fn mean_normalized(vectors: &[Vec<f32>]) -> Vec<f32> {
    let dimension = vectors.first().map(Vec::len).unwrap_or(embedder::DIMENSION);
    let mut mean = vec![0.0f32; dimension];

    for vector in vectors {
        for (slot, value) in mean.iter_mut().zip(vector) {
            *slot += value;
        }
    }

    let norm: f32 = mean.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut mean {
            *value /= norm;
        }
    }
    mean
}

/// Hybrid retrieval: vector top-40 and BM25 top-40, fused with RRF, diversified
/// with MMR, then capped at 10 chunks from at most 5 papers (§11.2).
pub fn retrieve(
    conn: &Connection,
    embedder: &Embedder,
    question: &str,
    scope: &Scope,
    generation: i64,
) -> Result<Vec<Candidate>> {
    let scoped: Vec<String> = scoped_paper_ids(conn, scope)?;
    if scoped.is_empty() {
        return Ok(Vec::new());
    }

    // Metadata and keyword search must keep working while the embedding model is
    // still downloading or the library is being re-indexed (§11.2).
    let query_vector = match embedder.embed_query(question) {
        Ok(query_vector) => Some(query_vector),
        Err(AppError::EmbeddingModelUnavailable) => {
            tracing::info!("embedding model unavailable; falling back to keyword search");
            None
        }
        Err(error) => return Err(error),
    };

    let vector_ranked = match &query_vector {
        Some(query_vector) => vector_search(conn, query_vector, generation, scope, &scoped)?,
        None => Vec::new(),
    };

    let keyword_ranked = keyword_search(conn, question, scope, &scoped)?;

    let fused = search::reciprocal_rank_fusion(&vector_ranked, &keyword_ranked);

    let mut candidates = Vec::new();
    for (chunk_id, _) in fused.iter().take(search::CANDIDATES_PER_SOURCE * 2) {
        if let Some(candidate) = load_candidate(conn, chunk_id, generation)? {
            if scoped.contains(&candidate.paper_id) {
                candidates.push(candidate);
            }
        }
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // With no query vector there is nothing to diversify against, so keep the
    // fused order.
    let selected = match query_vector {
        Some(query_vector) => search::mmr(&query_vector, &candidates, search::MAX_CHUNKS),
        None => candidates.into_iter().take(search::MAX_CHUNKS).collect(),
    };

    Ok(search::cap_papers(selected, search::MAX_PAPERS))
}

fn vector_search(
    conn: &Connection,
    query: &[f32],
    generation: i64,
    scope: &Scope,
    scoped: &[String],
) -> Result<Vec<String>> {
    // A post-filter over the library-wide top 40 can lose every chunk from the
    // current paper. For bounded scopes rank only vectors inside that scope.
    if !matches!(scope, Scope::Library) {
        let placeholders = std::iter::repeat("?")
            .take(scoped.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT v.chunk_id, v.embedding
             FROM chunk_vectors v JOIN chunks c ON c.id = v.chunk_id
             WHERE v.index_generation = ? AND c.paper_id IN ({placeholders})"
        );
        let mut values = Vec::<rusqlite::types::Value>::with_capacity(scoped.len() + 1);
        values.push(generation.into());
        values.extend(scoped.iter().cloned().map(Into::into));

        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            let blob: Vec<u8> = row.get(1)?;
            Ok((row.get::<_, String>(0)?, embedder::from_blob(&blob)))
        })?;
        let mut ranked = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        ranked.sort_by(|a, b| {
            embedder::cosine(query, &b.1)
                .partial_cmp(&embedder::cosine(query, &a.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return Ok(ranked
            .into_iter()
            .take(search::CANDIDATES_PER_SOURCE)
            .map(|(id, _)| id)
            .collect());
    }

    let mut statement = conn.prepare(
        "SELECT chunk_id FROM chunk_vectors
         WHERE embedding MATCH ?1 AND k = ?2 AND index_generation = ?3
         ORDER BY distance",
    )?;

    let rows = statement.query_map(
        params![
            to_blob(query),
            search::CANDIDATES_PER_SOURCE as i64,
            generation
        ],
        |row| row.get::<_, String>(0),
    )?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn keyword_search(
    conn: &Connection,
    question: &str,
    scope: &Scope,
    scoped: &[String],
) -> Result<Vec<String>> {
    let query = search::fts_query(question);
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let (sql, values) = if matches!(scope, Scope::Library) {
        (
            "SELECT chunk_id FROM chunks_fts
             WHERE chunks_fts MATCH ?
             ORDER BY bm25(chunks_fts)
             LIMIT ?"
                .to_string(),
            vec![
                rusqlite::types::Value::from(query),
                rusqlite::types::Value::from(search::CANDIDATES_PER_SOURCE as i64),
            ],
        )
    } else {
        let placeholders = std::iter::repeat("?")
            .take(scoped.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT chunk_id FROM chunks_fts
             WHERE chunks_fts MATCH ? AND paper_id IN ({placeholders})
             ORDER BY bm25(chunks_fts)
             LIMIT ?"
        );
        let mut values = Vec::with_capacity(scoped.len() + 2);
        values.push(rusqlite::types::Value::from(query));
        values.extend(scoped.iter().cloned().map(Into::into));
        values.push(rusqlite::types::Value::from(
            search::CANDIDATES_PER_SOURCE as i64,
        ));
        (sql, values)
    };

    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
        row.get::<_, String>(0)
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_candidate(conn: &Connection, chunk_id: &str, generation: i64) -> Result<Option<Candidate>> {
    let row = conn
        .query_row(
            "SELECT c.id, c.paper_id, c.page_start, c.page_end, c.section, c.text,
                    (SELECT v.embedding FROM chunk_vectors v
                     WHERE v.chunk_id = c.id AND v.index_generation = ?2)
             FROM chunks c WHERE c.id = ?1",
            params![chunk_id, generation],
            |row| {
                let embedding: Option<Vec<u8>> = row.get(6)?;
                Ok(Candidate {
                    chunk_id: row.get(0)?,
                    paper_id: row.get(1)?,
                    page_start: row.get(2)?,
                    page_end: row.get(3)?,
                    section: row.get(4)?,
                    text: row.get(5)?,
                    embedding: embedding
                        .map(|blob| embedder::from_blob(&blob))
                        .unwrap_or_default(),
                })
            },
        )
        .ok();

    Ok(row)
}

fn scoped_paper_ids(conn: &Connection, scope: &Scope) -> Result<Vec<String>> {
    let ids = match scope {
        Scope::Paper(paper_id) => vec![paper_id.clone()],

        Scope::Group(group_id) => {
            let mut statement =
                conn.prepare("SELECT paper_id FROM paper_groups WHERE group_id = ?1")?;
            let rows = statement.query_map(params![group_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }

        Scope::Library => {
            let mut statement = conn.prepare("SELECT id FROM papers")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };

    Ok(ids)
}

pub fn to_hits(conn: &Connection, candidates: &[Candidate]) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();

    for candidate in candidates {
        let title = paper_repo::get(conn, &candidate.paper_id)
            .map(|paper| paper.title)
            .unwrap_or_else(|_| "알 수 없는 논문".into());

        hits.push(SearchHit {
            chunk_id: candidate.chunk_id.clone(),
            paper_id: candidate.paper_id.clone(),
            paper_title: title,
            page_start: candidate.page_start,
            page_end: candidate.page_end,
            section: candidate.section.clone(),
            text: candidate.text.clone(),
        });
    }

    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn the_paper_vector_is_the_normalized_mean_of_its_chunks() {
        let vectors = vec![vec![1.0f32, 0.0], vec![0.0, 1.0]];

        let mean = mean_normalized(&vectors);

        let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "the paper vector must be a unit vector"
        );
        assert!((mean[0] - mean[1]).abs() < 1e-6);
    }

    #[test]
    fn a_paper_with_no_chunks_does_not_panic() {
        let mean = mean_normalized(&[]);
        assert_eq!(mean.len(), embedder::DIMENSION);
    }

    #[test]
    fn paper_keyword_search_cannot_leak_hits_from_another_paper() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        conn.execute(
            "INSERT INTO chunks_fts (chunk_id, paper_id, text) VALUES (?1, ?2, ?3)",
            params!["c1", "p1", "attention mechanism"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks_fts (chunk_id, paper_id, text) VALUES (?1, ?2, ?3)",
            params!["c2", "p2", "attention mechanism"],
        )
        .unwrap();

        let hits = keyword_search(
            &conn,
            "attention",
            &Scope::Paper("p1".into()),
            &["p1".into()],
        )
        .unwrap();

        assert_eq!(hits, vec!["c1"]);
    }
}
