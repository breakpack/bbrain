//! Topic graph — the "second brain". One node per **tag actually attached to a
//! paper** (`tags` × `paper_tags`), so the graph, the tag chips on each paper,
//! and the per-tag concept notes (`tag_note_entries`) all describe the same
//! rows. Edges are relatedness: co-occurrence (tags on the same paper) plus
//! embedding similarity between tag labels. Before each rebuild, duplicate
//! tags are merged *in the database* — "Transformer"/"Transformers"/casing
//! variants collapse into one tag, and their concept-note entries move with
//! them. The graph is a derived cache, rebuilt whenever tags change.
//!
//! Everything here is local and free — it reuses the on-device embedding model,
//! never a provider (§12).

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db::page_repo;
use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::rag::embedder::cosine;
use crate::state::AppState;
use crate::time::now_iso8601;

/// Two tags are the same concept — and are merged in the DB — when their
/// embeddings are at least this similar. Stricter than a display-only merge
/// would be, because this one rewrites rows.
const TAG_MERGE_THRESHOLD: f32 = 0.88;
/// Two distinct concepts get a "semantic" edge when their embeddings are this
/// close — conceptually related even if never discussed in the same paper.
const SEMANTIC_THRESHOLD: f32 = 0.80;
/// Cap semantic edges per topic so the graph stays legible.
const MAX_SEMANTIC_NEIGHBORS: usize = 4;

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

// --- tag units & duplicate merging -------------------------------------------

/// Lowercased, whitespace-collapsed form stored in `topics.normalized`.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Aggressive form for duplicate detection: unicode alphanumerics only,
/// lowercased, with one trailing ASCII plural 's' stripped — "Transformers",
/// "transformer" and "Trans-former" share a key. Korean/CJK labels pass
/// through with only spacing/punctuation removed.
fn aggressive_key(label: &str) -> String {
    let mut key: String = label
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if key.len() > 3 && key.is_ascii() && key.ends_with('s') && !key.ends_with("ss") {
        key.pop();
    }
    key
}

/// One graph node in the making: a tag and the papers it is attached to.
#[derive(Debug, Clone)]
struct TagUnit {
    tag_id: String,
    label: String,
    source: String,
    papers: Vec<String>,
}

/// A node ready to persist: label, member papers, and the label's embedding.
#[derive(Debug, Clone)]
struct Cluster {
    label: String,
    papers: BTreeSet<String>,
    centroid: Vec<f32>,
}

fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return vector.to_vec();
    }
    vector.iter().map(|v| v / norm).collect()
}

/// Tags in "representative first" order: a tag the user made outranks an AI
/// one, then more papers, then the shorter (and stable) label. The first tag
/// of any duplicate group keeps its name; the rest merge into it.
fn load_tag_units(conn: &Connection) -> Result<Vec<TagUnit>> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.display_name, t.source, pt.paper_id
         FROM tags t
         JOIN paper_tags pt ON pt.tag_id = t.id
         JOIN papers p ON p.id = pt.paper_id
         ORDER BY t.id, pt.paper_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut units: Vec<TagUnit> = Vec::new();
    for (tag_id, label, source, paper_id) in rows {
        match units.last_mut() {
            Some(unit) if unit.tag_id == tag_id => unit.papers.push(paper_id),
            _ => units.push(TagUnit {
                tag_id,
                label,
                source,
                papers: vec![paper_id],
            }),
        }
    }

    units.sort_by(|a, b| {
        (b.source == "user")
            .cmp(&(a.source == "user"))
            .then(b.papers.len().cmp(&a.papers.len()))
            .then(a.label.chars().count().cmp(&b.label.chars().count()))
            .then(a.label.cmp(&b.label))
    });
    Ok(units)
}

/// Duplicate groups among `units` (which must already be representative-first):
/// same aggressive key, or label embeddings above the merge threshold. Each
/// group's first index is the tag that survives. Pure, so the policy is
/// testable without the embedding model.
fn duplicate_groups(units: &[TagUnit], vectors: &[Vec<f32>]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut key_of_group: Vec<String> = Vec::new();

    for (index, unit) in units.iter().enumerate() {
        let key = aggressive_key(&unit.label);

        let mut joined = false;
        for (gi, group) in groups.iter_mut().enumerate() {
            let same_key = !key.is_empty() && key_of_group[gi] == key;
            let similar = cosine(&vectors[group[0]], &vectors[index]) >= TAG_MERGE_THRESHOLD;
            if same_key || similar {
                group.push(index);
                joined = true;
                break;
            }
        }
        if !joined {
            groups.push(vec![index]);
            key_of_group.push(key);
        }
    }

    groups
}

/// Folds a duplicate tag into its representative: papers move over, concept-
/// note entries move with the newer insight winning per paper, and the
/// duplicate row is deleted (its remaining children cascade away).
fn merge_tag_into(conn: &Connection, keep_id: &str, dup_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id)
         SELECT paper_id, ?1 FROM paper_tags WHERE tag_id = ?2",
        params![keep_id, dup_id],
    )?;
    conn.execute(
        "INSERT INTO tag_note_entries (tag_id, paper_id, insight_md, evidence_pages, updated_at)
         SELECT ?1, paper_id, insight_md, evidence_pages, updated_at
         FROM tag_note_entries WHERE tag_id = ?2
         ON CONFLICT(tag_id, paper_id) DO UPDATE SET
           insight_md = excluded.insight_md,
           evidence_pages = excluded.evidence_pages,
           updated_at = excluded.updated_at
         WHERE excluded.updated_at > tag_note_entries.updated_at",
        params![keep_id, dup_id],
    )?;
    conn.execute("DELETE FROM tags WHERE id = ?1", params![dup_id])?;
    Ok(())
}

/// Merges duplicate tags across the library (사용자 요청: 중복 태그 정리).
/// Returns the number of tags folded away. CPU-bound (embedding).
fn merge_duplicate_tags(state: &AppState) -> Result<usize> {
    let units = {
        let conn = state.db.conn();
        load_tag_units(&conn)?
    };
    if units.len() < 2 {
        return Ok(0);
    }

    let labels: Vec<String> = units.iter().map(|unit| unit.label.clone()).collect();
    let vectors = state.embedder.embed_passages(&labels)?;
    let vectors: Vec<Vec<f32>> = vectors.iter().map(|v| normalized(v)).collect();

    let mut merged = 0;
    let conn = state.db.conn();
    for group in duplicate_groups(&units, &vectors) {
        let keep = &units[group[0]];
        for &dup_index in &group[1..] {
            merge_tag_into(&conn, &keep.tag_id, &units[dup_index].tag_id)?;
            merged += 1;
        }
    }
    if merged > 0 {
        tracing::info!(merged, "folded duplicate tags into their representatives");
    }
    Ok(merged)
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

/// A fingerprint of everything the topic graph is derived from — the tags and
/// which papers carry them — so a rebuild is skipped when nothing relevant has
/// changed. JOIN papers so a row orphaned before FK enforcement can neither
/// enter the signature nor the rebuild.
fn signature(conn: &Connection) -> Result<String> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.display_name, pt.paper_id
         FROM tags t
         JOIN paper_tags pt ON pt.tag_id = t.id
         JOIN papers p ON p.id = pt.paper_id
         ORDER BY t.id, pt.paper_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut material = String::from("tags-v2;");
    for (tag_id, label, paper_id) in rows {
        material.push_str(&tag_id);
        material.push(':');
        material.push_str(&label);
        material.push(':');
        material.push_str(&paper_id);
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

/// Rebuilds the topic graph from the tags attached to papers and persists it.
/// Duplicate tags are merged in the database first, so the graph, the tag
/// chips, and the concept notes stay one row per concept. CPU-bound
/// (embedding), so callers run it off the async runtime.
fn rebuild_blocking(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();

    merge_duplicate_tags(&state)?;

    // Read (and fingerprint) the post-merge state, so the stored signature
    // matches what the next load computes and the rebuild is not repeated.
    let (units, sig) = {
        let conn = state.db.conn();
        (load_tag_units(&conn)?, signature(&conn)?)
    };

    let clusters: Vec<Cluster> = if units.is_empty() {
        Vec::new()
    } else {
        let labels: Vec<String> = units.iter().map(|unit| unit.label.clone()).collect();
        let vectors = state.embedder.embed_passages(&labels)?;
        units
            .iter()
            .zip(vectors)
            .map(|(unit, vector)| Cluster {
                label: unit.label.clone(),
                papers: unit.papers.iter().cloned().collect(),
                centroid: normalized(&vector),
            })
            .collect()
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
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn seed_paper(conn: &Connection, id: &str) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", "Paper", ImportStatus::Ready).unwrap();
    }

    fn tag_on(conn: &Connection, paper_id: &str, label: &str, source: &str) -> String {
        let tag_id = paper_repo::upsert_tag(conn, label, source).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id) VALUES (?1, ?2)",
            params![paper_id, tag_id],
        )
        .unwrap();
        tag_id
    }

    fn unit(label: &str, source: &str, papers: &[&str]) -> TagUnit {
        TagUnit {
            tag_id: format!("id-{label}"),
            label: label.into(),
            source: source.into(),
            papers: papers.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn aggressive_keys_collapse_case_punctuation_and_ascii_plurals() {
        assert_eq!(aggressive_key("Transformers"), aggressive_key("transformer"));
        assert_eq!(aggressive_key("Self-Attention"), aggressive_key("self attention"));
        assert_eq!(aggressive_key("RAG"), "rag");
        // Double-s words are not plurals; Korean is left intact.
        assert_eq!(aggressive_key("loss"), "loss");
        assert_eq!(aggressive_key("어텐션"), "어텐션");
        assert_ne!(aggressive_key("RAG"), aggressive_key("Attention"));
    }

    #[test]
    fn duplicate_groups_merge_by_key_or_embedding_and_keep_the_representative_first() {
        // Representative-first order: the user's tag leads.
        let units = vec![
            unit("attention", "user", &["p1"]),
            unit("Attentions", "ai", &["p2"]),    // same aggressive key
            unit("어텐션", "ai", &["p3"]),         // same only by embedding
            unit("photosynthesis", "ai", &["p4"]), // unrelated
        ];
        let vectors = vec![
            vec![1.0, 0.0],
            vec![1.0, 0.0],
            normalized(&[0.95, 0.05]),
            vec![0.0, 1.0],
        ];

        let groups = duplicate_groups(&units, &vectors);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], vec![0, 1, 2], "all three attention variants fold into the user's tag");
        assert_eq!(groups[1], vec![3]);
    }

    #[test]
    fn merging_a_tag_moves_papers_and_notes_and_the_newer_insight_wins() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        seed_paper(&conn, "p2");
        let keep = tag_on(&conn, "p1", "attention", "user");
        let dup = tag_on(&conn, "p2", "Attentions", "ai");
        // p1 carries both spellings; the note on the duplicate is newer.
        conn.execute(
            "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id) VALUES ('p1', ?1)",
            params![dup],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tag_note_entries (tag_id, paper_id, insight_md, evidence_pages, updated_at)
             VALUES (?1, 'p1', '옛 설명', '[1]', '2026-01-01T00:00:00Z'),
                    (?2, 'p1', '새 설명', '[2]', '2026-06-01T00:00:00Z'),
                    (?2, 'p2', '다른 논문 설명', '[3]', '2026-06-01T00:00:00Z')",
            params![keep, dup],
        )
        .unwrap();

        merge_tag_into(&conn, &keep, &dup).unwrap();

        let tags: i64 = conn.query_row("SELECT count(*) FROM tags", [], |r| r.get(0)).unwrap();
        assert_eq!(tags, 1, "the duplicate row is gone");

        let papers: Vec<String> = conn
            .prepare("SELECT paper_id FROM paper_tags WHERE tag_id = ?1 ORDER BY paper_id")
            .unwrap()
            .query_map(params![keep], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(papers, vec!["p1", "p2"], "both papers now carry the surviving tag");

        let notes: Vec<(String, String)> = conn
            .prepare(
                "SELECT paper_id, insight_md FROM tag_note_entries
                 WHERE tag_id = ?1 ORDER BY paper_id",
            )
            .unwrap()
            .query_map(params![keep], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            notes,
            vec![
                ("p1".into(), "새 설명".into()),
                ("p2".into(), "다른 논문 설명".into())
            ],
            "notes follow the merge and the newer insight wins per paper"
        );
    }

    #[test]
    fn tag_units_come_from_paper_tags_in_representative_first_order() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        seed_paper(&conn, "p2");
        tag_on(&conn, "p1", "popular ai tag", "ai");
        tag_on(&conn, "p2", "popular ai tag", "ai");
        tag_on(&conn, "p2", "user tag", "user");
        // A tag attached to nothing is not a node.
        paper_repo::upsert_tag(&conn, "unattached", "ai").unwrap();

        let units = load_tag_units(&conn).unwrap();

        let labels: Vec<&str> = units.iter().map(|u| u.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["user tag", "popular ai tag"],
            "user tags lead, unattached tags are absent"
        );
        assert_eq!(units[1].papers.len(), 2);
    }

    #[test]
    fn the_signature_tracks_tag_attachments() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        let before = signature(&conn).unwrap();

        tag_on(&conn, "p1", "attention", "ai");
        let after = signature(&conn).unwrap();

        assert_ne!(before, after, "attaching a tag must trigger a rebuild");
    }

    #[test]
    fn cooccurrence_counts_shared_papers() {
        let clusters = vec![
            Cluster {
                label: "A".into(),
                papers: ["p1", "p2"].iter().map(|s| s.to_string()).collect(),
                centroid: vec![1.0, 0.0],
            },
            Cluster {
                label: "B".into(),
                papers: ["p1", "p2", "p3"].iter().map(|s| s.to_string()).collect(),
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
                centroid: vec![1.0, 0.0],
            },
            Cluster {
                label: "B".into(),
                papers: BTreeSet::new(),
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
