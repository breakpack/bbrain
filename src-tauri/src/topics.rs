//! Topic graph — the "second brain". Instead of one node per paper, this builds
//! a graph of concepts extracted from each paper's AI analysis (its keywords and
//! suggested tags, which the model already distilled from the summary). Candidate
//! topics are merged across the whole library by embedding similarity, so
//! "Transformer", "Transformers", and "트랜스포머" collapse into one concept. The
//! graph is a derived cache: it is rebuilt whenever the set of analyses changes.
//!
//! Everything here is local and free — it reuses the on-device embedding model,
//! never a provider (§12).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::analysis::schema::PaperAnalysisV1;
use crate::db::{page_repo, settings_repo};
use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::rag::embedder::cosine;
use crate::state::AppState;
use crate::time::now_iso8601;

/// Two candidate topics merge into one concept when their embeddings are at least
/// this similar. High enough that distinct concepts stay apart, low enough to
/// catch plurals, casing, and cross-language synonyms.
const MERGE_THRESHOLD: f32 = 0.85;
/// Two distinct concepts get a "semantic" edge when their centroids are this
/// close — conceptually related even if never discussed in the same paper.
const SEMANTIC_THRESHOLD: f32 = 0.80;
/// Cap semantic edges per topic so the graph stays legible.
const MAX_SEMANTIC_NEIGHBORS: usize = 4;
/// Ignore one-character candidates — they are noise, not concepts.
const MIN_LABEL_CHARS: usize = 2;

// --- wire types --------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicPaperRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicNode {
    pub id: String,
    pub label: String,
    pub paper_count: i64,
    pub papers: Vec<TopicPaperRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicGraph {
    pub nodes: Vec<TopicNode>,
    pub edges: Vec<TopicEdge>,
}

// --- extraction & clustering (pure) ------------------------------------------

/// Lowercased, whitespace-collapsed form used to key and merge identical strings.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The concept candidates a paper contributes: the keywords and suggested tags
/// its analysis already distilled from the summary.
fn extract_candidates(analysis: &PaperAnalysisV1) -> Vec<String> {
    analysis
        .keywords
        .iter()
        .chain(analysis.suggested_tags.iter())
        .map(|s| s.trim().to_string())
        .filter(|s| s.chars().count() >= MIN_LABEL_CHARS)
        .collect()
}

/// One distinct candidate string and the papers it appears in.
#[derive(Debug, Clone)]
struct UniqueTopic {
    label: String,
    papers: Vec<String>,
}

/// A merged concept: a representative label, the union of member papers, and the
/// centroid of member embeddings.
#[derive(Debug, Clone)]
struct Cluster {
    label: String,
    papers: BTreeSet<String>,
    sum: Vec<f32>,
    centroid: Vec<f32>,
}

fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return vector.to_vec();
    }
    vector.iter().map(|v| v / norm).collect()
}

/// Greedy single-pass clustering: candidates are visited most-frequent first (so
/// popular concepts seed clusters and lend their label), and each joins the
/// nearest existing cluster above the merge threshold, or starts its own.
fn cluster(unique: &[UniqueTopic], vectors: &[Vec<f32>], threshold: f32) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();

    for (index, topic) in unique.iter().enumerate() {
        let vector = &vectors[index];

        let mut best: Option<(usize, f32)> = None;
        for (ci, existing) in clusters.iter().enumerate() {
            let similarity = cosine(&existing.centroid, vector);
            if similarity >= threshold && best.map_or(true, |(_, s)| similarity > s) {
                best = Some((ci, similarity));
            }
        }

        match best {
            Some((ci, _)) => {
                let cluster = &mut clusters[ci];
                for (a, b) in cluster.sum.iter_mut().zip(vector) {
                    *a += b;
                }
                cluster.centroid = normalized(&cluster.sum);
                cluster.papers.extend(topic.papers.iter().cloned());
            }
            None => clusters.push(Cluster {
                label: topic.label.clone(),
                papers: topic.papers.iter().cloned().collect(),
                sum: vector.clone(),
                centroid: normalized(vector),
            }),
        }
    }

    clusters
}

/// Co-occurrence edges: two topics are linked with weight = the number of papers
/// that discuss both. Returned as `(i, j, count)` with `i < j`.
fn cooccurrence_edges(clusters: &[Cluster]) -> Vec<(usize, usize, f64)> {
    let mut by_paper: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ci, cluster) in clusters.iter().enumerate() {
        for paper in &cluster.papers {
            by_paper.entry(paper.as_str()).or_default().push(ci);
        }
    }

    let mut counts: HashMap<(usize, usize), f64> = HashMap::new();
    for topics in by_paper.values() {
        for a in 0..topics.len() {
            for b in (a + 1)..topics.len() {
                let (i, j) = (topics[a].min(topics[b]), topics[a].max(topics[b]));
                *counts.entry((i, j)).or_default() += 1.0;
            }
        }
    }

    counts.into_iter().map(|((i, j), c)| (i, j, c)).collect()
}

/// Semantic edges: for each topic, its strongest few neighbours above the
/// threshold, excluding pairs that already have a co-occurrence edge. Returned as
/// `(i, j, cosine)` with `i < j`, deduplicated.
fn semantic_edges(
    clusters: &[Cluster],
    threshold: f32,
    max_neighbors: usize,
    cooccurring: &HashSet<(usize, usize)>,
) -> Vec<(usize, usize, f64)> {
    let mut edges: HashMap<(usize, usize), f64> = HashMap::new();

    for i in 0..clusters.len() {
        let mut neighbors: Vec<(usize, f32)> = Vec::new();
        for j in 0..clusters.len() {
            if i == j {
                continue;
            }
            let pair = (i.min(j), i.max(j));
            if cooccurring.contains(&pair) {
                continue;
            }
            let similarity = cosine(&clusters[i].centroid, &clusters[j].centroid);
            if similarity >= threshold {
                neighbors.push((j, similarity));
            }
        }
        neighbors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (j, similarity) in neighbors.into_iter().take(max_neighbors) {
            let pair = (i.min(j), i.max(j));
            edges.insert(pair, similarity as f64);
        }
    }

    edges.into_iter().map(|((i, j), s)| (i, j, s)).collect()
}

// --- rebuild & load ----------------------------------------------------------

/// A fingerprint of everything the topic graph is derived from, so a rebuild is
/// skipped when nothing relevant has changed.
fn signature(conn: &Connection) -> Result<String> {
    let generation = settings_repo::get(conn)?.index_generation;

    // JOIN papers: an orphaned analysis (its paper deleted while foreign keys
    // were off, e.g. rows from early dev builds) must neither enter the
    // signature nor the rebuild — inserting its topics violates the
    // paper_topics FK and kills the whole graph page.
    let mut statement = conn.prepare(
        "SELECT a.paper_id, a.content_hash FROM analyses a
         JOIN papers p ON p.id = a.paper_id
         ORDER BY a.paper_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut material = format!("gen={generation};");
    for (paper_id, hash) in rows {
        material.push_str(&paper_id);
        material.push(':');
        material.push_str(&hash);
        material.push(';');
    }
    Ok(page_repo::text_hash(&material))
}

fn stored_signature(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT signature FROM topic_build WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?)
}

/// Rebuilds the topic graph from every analysis in the library and persists it.
/// CPU-bound (embedding), so callers run it off the async runtime.
fn rebuild_blocking(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();

    let (analyses, sig) = {
        let conn = state.db.conn();
        // Same JOIN as signature(): only analyses whose paper still exists.
        let mut statement = conn.prepare(
            "SELECT a.paper_id, a.structured_json FROM analyses a
             JOIN papers p ON p.id = a.paper_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let analyses: Vec<(String, PaperAnalysisV1)> = rows
            .into_iter()
            .filter_map(|(paper_id, json)| {
                serde_json::from_str::<PaperAnalysisV1>(&json)
                    .ok()
                    .map(|analysis| (paper_id, analysis))
            })
            .collect();
        (analyses, signature(&conn)?)
    };

    // Aggregate identical candidate strings across the library.
    let mut aggregate: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    for (paper_id, analysis) in &analyses {
        for candidate in extract_candidates(analysis) {
            let key = normalize(&candidate);
            if key.is_empty() {
                continue;
            }
            let entry = aggregate
                .entry(key)
                .or_insert_with(|| (candidate.clone(), BTreeSet::new()));
            entry.1.insert(paper_id.clone());
        }
    }

    let mut unique: Vec<UniqueTopic> = aggregate
        .into_iter()
        .map(|(_, (label, papers))| UniqueTopic {
            label,
            papers: papers.into_iter().collect(),
        })
        .collect();
    // Most-frequent first so popular concepts seed clusters; stable tiebreak.
    unique.sort_by(|a, b| b.papers.len().cmp(&a.papers.len()).then(a.label.cmp(&b.label)));

    let clusters = if unique.is_empty() {
        Vec::new()
    } else {
        let labels: Vec<String> = unique.iter().map(|topic| topic.label.clone()).collect();
        let vectors = state.embedder.embed_passages(&labels)?;
        cluster(&unique, &vectors, MERGE_THRESHOLD)
    };

    let cooccurrence = cooccurrence_edges(&clusters);
    let cooccurring: HashSet<(usize, usize)> =
        cooccurrence.iter().map(|&(i, j, _)| (i, j)).collect();
    let semantic = semantic_edges(&clusters, SEMANTIC_THRESHOLD, MAX_SEMANTIC_NEIGHBORS, &cooccurring);

    persist(&state, &clusters, &cooccurrence, &semantic, &sig)?;
    Ok(())
}

fn persist(
    state: &AppState,
    clusters: &[Cluster],
    cooccurrence: &[(usize, usize, f64)],
    semantic: &[(usize, usize, f64)],
    sig: &str,
) -> Result<()> {
    let mut conn = state.db.conn();
    let tx = conn.transaction()?;

    // A full rebuild: clear the derived cache first. topics cascades to its
    // children, but delete explicitly for clarity.
    tx.execute("DELETE FROM topic_edges", [])?;
    tx.execute("DELETE FROM paper_topics", [])?;
    tx.execute("DELETE FROM topics", [])?;

    let now = now_iso8601();
    let mut topic_ids = Vec::with_capacity(clusters.len());
    for cluster in clusters {
        let topic_id = new_id();
        tx.execute(
            "INSERT INTO topics (id, label, normalized, paper_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                topic_id,
                cluster.label,
                normalize(&cluster.label),
                cluster.papers.len() as i64,
                now,
            ],
        )?;
        for paper_id in &cluster.papers {
            tx.execute(
                "INSERT INTO paper_topics (paper_id, topic_id) VALUES (?1, ?2)",
                params![paper_id, topic_id],
            )?;
        }
        topic_ids.push(topic_id);
    }

    for &(i, j, weight) in cooccurrence {
        tx.execute(
            "INSERT OR REPLACE INTO topic_edges
               (source_topic_id, target_topic_id, edge_type, weight)
             VALUES (?1, ?2, 'cooccurrence', ?3)",
            params![topic_ids[i], topic_ids[j], weight],
        )?;
    }
    for &(i, j, weight) in semantic {
        tx.execute(
            "INSERT OR REPLACE INTO topic_edges
               (source_topic_id, target_topic_id, edge_type, weight)
             VALUES (?1, ?2, 'semantic', ?3)",
            params![topic_ids[i], topic_ids[j], weight],
        )?;
    }

    tx.execute(
        "INSERT INTO topic_build (id, signature, built_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET signature = excluded.signature, built_at = excluded.built_at",
        params![sig, now],
    )?;

    tx.commit()?;
    Ok(())
}

/// Loads the persisted topic graph for the UI.
pub fn load_topic_graph(conn: &Connection) -> Result<TopicGraph> {
    let mut node_statement =
        conn.prepare("SELECT id, label, paper_count FROM topics ORDER BY paper_count DESC")?;
    let node_rows = node_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut nodes = Vec::with_capacity(node_rows.len());
    for (id, label, paper_count) in node_rows {
        let mut paper_statement = conn.prepare(
            "SELECT p.id, p.title FROM paper_topics t
             JOIN papers p ON p.id = t.paper_id
             WHERE t.topic_id = ?1
             ORDER BY p.title",
        )?;
        let papers = paper_statement
            .query_map(params![id], |row| {
                Ok(TopicPaperRef {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        nodes.push(TopicNode {
            id,
            label,
            paper_count,
            papers,
        });
    }

    let mut edge_statement = conn
        .prepare("SELECT source_topic_id, target_topic_id, edge_type, weight FROM topic_edges")?;
    let edges = edge_statement
        .query_map([], |row| {
            Ok(TopicEdge {
                source: row.get(0)?,
                target: row.get(1)?,
                edge_type: row.get(2)?,
                weight: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(TopicGraph { nodes, edges })
}

/// Returns the topic graph, rebuilding first if the analyses changed since the
/// last build (or if `force` is set). The rebuild runs off the async runtime.
pub async fn ensure_and_load(app: &AppHandle, force: bool) -> Result<TopicGraph> {
    let needs_rebuild = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        force || stored_signature(&conn)? != Some(signature(&conn)?)
    };

    if needs_rebuild {
        let app = app.clone();
        tokio::task::spawn_blocking(move || rebuild_blocking(&app))
            .await
            .map_err(|e| AppError::Internal(format!("topic rebuild task: {e}")))??;
    }

    let state = app.state::<AppState>();
    let conn = state.db.conn();
    load_topic_graph(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::schema::PaperAnalysisV1;

    fn analysis(keywords: &[&str], tags: &[&str]) -> PaperAnalysisV1 {
        PaperAnalysisV1 {
            schema_version: "1".into(),
            short_summary: String::new(),
            detailed_summary: String::new(),
            research_problem: String::new(),
            contributions: vec![],
            methodology: String::new(),
            results: vec![],
            limitations: vec![],
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            suggested_tags: tags.iter().map(|s| s.to_string()).collect(),
            follow_up_questions: vec![],
        }
    }

    fn unit(label: &str, papers: &[&str]) -> UniqueTopic {
        UniqueTopic {
            label: label.into(),
            papers: papers.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn an_orphaned_analysis_does_not_enter_the_signature() {
        // A paper deleted while foreign keys were off (early dev builds) leaves
        // its analysis behind; rebuilding from it violates the paper_topics FK
        // and takes the whole graph page down. The signature — and therefore
        // the rebuild input — must only see analyses whose paper still exists.
        let db = crate::db::Database::open_in_memory().unwrap();
        let conn = db.conn();

        conn.execute_batch(
            "INSERT INTO papers (id, sha256, title, managed_path, import_status, page_count,
                                 is_favorite, created_at, updated_at)
             VALUES ('p1', 'h1', 'Alive', '/x', 'ready', 1, 0, '2026-01-01', '2026-01-01');
             INSERT INTO analyses (paper_id, schema_version, provider, model, content_hash,
                                   structured_json, markdown, created_at)
             VALUES ('p1', '1', 'anthropic', 'm', 'hash-alive', '{}', '', '2026-01-01');
             PRAGMA foreign_keys = OFF;
             INSERT INTO analyses (paper_id, schema_version, provider, model, content_hash,
                                   structured_json, markdown, created_at)
             VALUES ('ghost', '1', 'anthropic', 'm', 'hash-ghost', '{}', '', '2026-01-01');
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();

        let with_ghost = signature(&conn).unwrap();
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        conn.execute("DELETE FROM analyses WHERE paper_id = 'ghost'", []).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        let without_ghost = signature(&conn).unwrap();

        assert_eq!(with_ghost, without_ghost, "orphan must be invisible to the signature");
    }

    #[test]
    fn candidates_come_from_keywords_and_tags_and_skip_noise() {
        let a = analysis(&["Transformer", "Attention", "x"], &["nlp", " "]);
        let candidates = extract_candidates(&a);
        assert!(candidates.contains(&"Transformer".to_string()));
        assert!(candidates.contains(&"nlp".to_string()));
        assert!(!candidates.iter().any(|c| c == "x"), "one-char candidate is dropped");
        assert!(!candidates.iter().any(|c| c.trim().is_empty()));
    }

    #[test]
    fn near_vectors_merge_into_one_concept() {
        // Two labels with nearly identical vectors merge; a distant one stays apart.
        let unique = vec![
            unit("Transformer", &["p1"]),
            unit("Transformers", &["p2"]),
            unit("Photosynthesis", &["p3"]),
        ];
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.99, 0.01, 0.0],
            vec![0.0, 1.0, 0.0],
        ];

        let clusters = cluster(&unique, &vectors, 0.85);

        assert_eq!(clusters.len(), 2, "the two transformer variants merge");
        let transformer = clusters.iter().find(|c| c.papers.contains("p1")).unwrap();
        assert!(transformer.papers.contains("p2"), "merged cluster carries both papers");
    }

    #[test]
    fn cooccurrence_counts_shared_papers() {
        let clusters = vec![
            Cluster {
                label: "A".into(),
                papers: ["p1", "p2"].iter().map(|s| s.to_string()).collect(),
                sum: vec![1.0, 0.0],
                centroid: vec![1.0, 0.0],
            },
            Cluster {
                label: "B".into(),
                papers: ["p1", "p2", "p3"].iter().map(|s| s.to_string()).collect(),
                sum: vec![0.0, 1.0],
                centroid: vec![0.0, 1.0],
            },
        ];

        let edges = cooccurrence_edges(&clusters);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0], (0, 1, 2.0), "A and B share two papers");
    }

    #[test]
    fn semantic_edges_skip_pairs_that_already_cooccur() {
        let clusters = vec![
            Cluster {
                label: "A".into(),
                papers: BTreeSet::new(),
                sum: vec![1.0, 0.0],
                centroid: vec![1.0, 0.0],
            },
            Cluster {
                label: "B".into(),
                papers: BTreeSet::new(),
                sum: vec![0.99, 0.01],
                centroid: normalized(&[0.99, 0.01]),
            },
        ];

        let mut cooccurring = HashSet::new();
        cooccurring.insert((0, 1));
        let edges = semantic_edges(&clusters, 0.8, 4, &cooccurring);
        assert!(edges.is_empty(), "no semantic edge when the pair already co-occurs");

        let edges = semantic_edges(&clusters, 0.8, 4, &HashSet::new());
        assert_eq!(edges.len(), 1, "otherwise a semantic edge is added");
    }
}
