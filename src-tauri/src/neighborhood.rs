//! ConnectedPapers-style focus graph. One paper sits at the centre; its nearest
//! neighbours by embedding similarity surround it, with similarity edges among
//! them so clusters of related work read as clusters, plus any citation edges
//! resolved inside the library. Papers older than the focus read as precedent,
//! newer ones as derivative — the lineage the UI lays a year axis on.
//!
//! Everything is local and free (§12): it reuses the paper embeddings already in
//! the index and the citation edges `relations.rs` already resolves. It never
//! calls a provider or an external citation API — an external source (Semantic
//! Scholar and the like) would be a separate, opt-in addition.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::settings_repo;
use crate::error::{AppError, Result};
use crate::rag::embedder::{cosine, from_blob};

/// How many nearest neighbours to gather around the focus paper. Larger than the
/// five kept for the whole-library graph (§12.1) — a focus view wants a fuller
/// neighbourhood.
const NEIGHBORHOOD_SIZE: usize = 16;
/// A neighbour must clear this cosine similarity to the focus paper to appear.
/// Slightly below the whole-library relation threshold so the neighbourhood is
/// not sparse.
const FOCUS_THRESHOLD: f32 = 0.70;
/// Two neighbours are linked to each other above this similarity, so precedent
/// and derivative clusters hold together.
const NEIGHBOR_THRESHOLD: f32 = 0.78;
/// Cap neighbour-to-neighbour edges per node so the graph stays legible.
const MAX_NEIGHBOR_EDGES: usize = 3;

/// Where a neighbour sits relative to the focus paper in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lineage {
    /// The focus paper itself.
    Focus,
    /// Published before the focus paper — prior work it builds on.
    Precedent,
    /// Published after the focus paper — work that came later.
    Derivative,
    /// Same year, or a year is missing on either side.
    Concurrent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborNode {
    pub id: String,
    pub title: String,
    pub year: Option<i64>,
    /// Cosine similarity to the focus paper; `None` for the focus itself and for
    /// a paper pulled in only by a citation edge.
    pub similarity: Option<f64>,
    pub lineage: Lineage,
    /// True when a citation edge, resolved inside the library, ties this paper to
    /// the focus.
    pub cites_focus: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborEdge {
    pub source: String,
    pub target: String,
    /// `similarity` | `citation`.
    pub edge_type: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperNeighborhood {
    pub center_id: String,
    pub nodes: Vec<NeighborNode>,
    pub edges: Vec<NeighborEdge>,
}

fn lineage(center_year: Option<i64>, year: Option<i64>) -> Lineage {
    match (center_year, year) {
        (Some(c), Some(y)) if y < c => Lineage::Precedent,
        (Some(c), Some(y)) if y > c => Lineage::Derivative,
        _ => Lineage::Concurrent,
    }
}

/// A paper's display fields and its embedding blob, if it has one at this
/// generation.
fn paper_row(
    conn: &Connection,
    paper_id: &str,
    generation: i64,
) -> Result<Option<(String, Option<i64>, Option<Vec<u8>>)>> {
    let base: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT p.title, m.year FROM papers p
             LEFT JOIN paper_metadata m ON m.paper_id = p.id
             WHERE p.id = ?1",
            params![paper_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let Some((title, year)) = base else {
        return Ok(None);
    };

    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM paper_vectors
             WHERE paper_id = ?1 AND index_generation = ?2",
            params![paper_id, generation],
            |row| row.get(0),
        )
        .optional()?;

    Ok(Some((title, year, blob)))
}

/// Citation-linked papers (either direction) resolved inside the library.
fn citation_neighbours(conn: &Connection, paper_id: &str) -> Result<HashSet<String>> {
    let mut statement = conn.prepare(
        "SELECT source_paper_id, target_paper_id FROM relations
         WHERE relation_type = 'citation' AND (source_paper_id = ?1 OR target_paper_id = ?1)",
    )?;
    let rows = statement.query_map(params![paper_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut set = HashSet::new();
    for row in rows {
        let (source, target) = row?;
        let other = if source == paper_id { target } else { source };
        if other != paper_id {
            set.insert(other);
        }
    }
    Ok(set)
}

/// The focus graph for one paper. Missing paper → `NotFound`. A paper with no
/// embedding yet (still importing/analysing) returns just its own node, so the
/// UI can show the centre with an empty neighbourhood rather than an error.
pub fn load(conn: &Connection, paper_id: &str) -> Result<PaperNeighborhood> {
    let generation = settings_repo::get(conn)?.index_generation;

    let (center_title, center_year, center_blob) = paper_row(conn, paper_id, generation)?
        .ok_or_else(|| AppError::NotFound(format!("paper {paper_id}")))?;

    let mut nodes = vec![NeighborNode {
        id: paper_id.to_string(),
        title: center_title,
        year: center_year,
        similarity: None,
        lineage: Lineage::Focus,
        cites_focus: false,
    }];
    let mut edges: Vec<NeighborEdge> = Vec::new();

    // Candidate neighbours and their similarity to the focus (None until known).
    let mut candidates: Vec<(String, Option<f64>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([paper_id.to_string()]);

    if let Some(center_blob) = &center_blob {
        let mut statement = conn.prepare(
            "SELECT paper_id, distance FROM paper_vectors
             WHERE embedding MATCH ?1 AND k = ?2 AND index_generation = ?3
             ORDER BY distance",
        )?;
        // Ask for one extra: the focus paper always matches itself.
        let rows = statement.query_map(
            params![center_blob, (NEIGHBORHOOD_SIZE + 1) as i64, generation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;
        for row in rows {
            let (id, distance) = row?;
            if id == paper_id {
                continue;
            }
            // vec0 cosine distance is 1 - similarity.
            let similarity = 1.0 - distance;
            if similarity as f32 >= FOCUS_THRESHOLD && seen.insert(id.clone()) {
                candidates.push((id, Some(similarity)));
            }
            if candidates.len() >= NEIGHBORHOOD_SIZE {
                break;
            }
        }
    }

    // Citation-linked papers join the neighbourhood even when they fall below the
    // similarity threshold — an explicit reference is a strong signal.
    let cited = citation_neighbours(conn, paper_id)?;
    for id in &cited {
        if seen.insert(id.clone()) {
            candidates.push((id.clone(), None));
        }
    }

    // Materialize each candidate, keeping its embedding for the pairwise pass.
    let mut vectors: HashMap<String, Vec<f32>> = HashMap::new();
    if let Some(center_blob) = &center_blob {
        vectors.insert(paper_id.to_string(), from_blob(center_blob));
    }

    for (id, similarity) in &candidates {
        let Some((title, year, blob)) = paper_row(conn, id, generation)? else {
            continue; // deleted between queries
        };
        if let Some(blob) = &blob {
            vectors.insert(id.clone(), from_blob(blob));
        }
        let cites_focus = cited.contains(id);
        nodes.push(NeighborNode {
            id: id.clone(),
            title,
            year,
            similarity: *similarity,
            lineage: lineage(center_year, year),
            cites_focus,
        });

        // Centre → neighbour edge. Prefer the similarity edge; a purely
        // citation-linked neighbour gets a citation edge instead.
        if let Some(similarity) = similarity {
            edges.push(NeighborEdge {
                source: paper_id.to_string(),
                target: id.clone(),
                edge_type: "similarity".into(),
                weight: *similarity,
            });
        }
        if cites_focus {
            edges.push(NeighborEdge {
                source: paper_id.to_string(),
                target: id.clone(),
                edge_type: "citation".into(),
                weight: 1.0,
            });
        }
    }

    edges.extend(neighbor_edges(&nodes, &vectors, paper_id));

    Ok(PaperNeighborhood {
        center_id: paper_id.to_string(),
        nodes,
        edges,
    })
}

/// Similarity edges among the neighbours themselves (excluding the focus, whose
/// edges are already drawn). Each neighbour keeps its strongest few, deduplicated.
fn neighbor_edges(
    nodes: &[NeighborNode],
    vectors: &HashMap<String, Vec<f32>>,
    center_id: &str,
) -> Vec<NeighborEdge> {
    let ids: Vec<&str> = nodes
        .iter()
        .map(|node| node.id.as_str())
        .filter(|id| *id != center_id && vectors.contains_key(*id))
        .collect();

    let mut edges: HashMap<(usize, usize), f64> = HashMap::new();
    for a in 0..ids.len() {
        let mut ranked: Vec<(usize, f32)> = Vec::new();
        for b in 0..ids.len() {
            if a == b {
                continue;
            }
            let similarity = cosine(&vectors[ids[a]], &vectors[ids[b]]);
            if similarity >= NEIGHBOR_THRESHOLD {
                ranked.push((b, similarity));
            }
        }
        ranked.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        for (b, similarity) in ranked.into_iter().take(MAX_NEIGHBOR_EDGES) {
            let pair = (a.min(b), a.max(b));
            edges.insert(pair, similarity as f64);
        }
    }

    edges
        .into_iter()
        .map(|((a, b), weight)| NeighborEdge {
            source: ids[a].to_string(),
            target: ids[b].to_string(),
            edge_type: "similarity".into(),
            weight,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;
    use crate::rag::embedder::to_blob;

    /// A 384-d unit vector: mostly along `axis`, with a little on `tilt` so two
    /// papers can be near without being identical.
    fn vector(axis: usize, tilt: usize, tilt_weight: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        v[axis] = 1.0;
        v[tilt] += tilt_weight;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    fn add_paper(conn: &Connection, id: &str, title: &str, year: Option<i64>) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", title, ImportStatus::Ready).unwrap();
        if let Some(year) = year {
            conn.execute(
                "UPDATE paper_metadata SET year = ?2 WHERE paper_id = ?1",
                params![id, year],
            )
            .unwrap();
        }
    }

    fn add_vector(conn: &Connection, id: &str, vector: &[f32]) {
        conn.execute(
            "INSERT INTO paper_vectors (paper_id, index_generation, embedding)
             VALUES (?1, 1, ?2)",
            params![id, to_blob(vector)],
        )
        .unwrap();
    }

    #[test]
    fn lineage_is_read_from_the_years() {
        assert_eq!(lineage(Some(2018), Some(2015)), Lineage::Precedent);
        assert_eq!(lineage(Some(2018), Some(2020)), Lineage::Derivative);
        assert_eq!(lineage(Some(2018), Some(2018)), Lineage::Concurrent);
        assert_eq!(lineage(Some(2018), None), Lineage::Concurrent);
        assert_eq!(lineage(None, Some(2015)), Lineage::Concurrent);
    }

    #[test]
    fn a_missing_paper_is_not_found() {
        let db = Database::open_in_memory().unwrap();
        let err = load(&db.conn(), "nope").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[test]
    fn a_paper_without_an_embedding_yet_returns_just_itself() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        add_paper(&conn, "focus", "Focus", Some(2018));

        let graph = load(&conn, "focus").unwrap();

        assert_eq!(graph.center_id, "focus");
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].lineage, Lineage::Focus);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn neighbours_are_gathered_by_similarity_and_classified_by_year() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        add_paper(&conn, "focus", "Focus", Some(2018));
        add_paper(&conn, "prior", "Prior work", Some(2015));
        add_paper(&conn, "later", "Later work", Some(2020));
        add_paper(&conn, "far", "Unrelated", Some(2019));

        add_vector(&conn, "focus", &vector(0, 1, 0.05));
        add_vector(&conn, "prior", &vector(0, 1, 0.12)); // ~0.99 cosine to focus
        add_vector(&conn, "later", &vector(0, 2, 0.20)); // ~0.98 cosine to focus
        add_vector(&conn, "far", &vector(200, 201, 0.1)); // orthogonal → excluded

        let graph = load(&conn, "focus").unwrap();

        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"prior") && ids.contains(&"later"));
        assert!(!ids.contains(&"far"), "an orthogonal paper is below the threshold");

        let prior = graph.nodes.iter().find(|n| n.id == "prior").unwrap();
        let later = graph.nodes.iter().find(|n| n.id == "later").unwrap();
        assert_eq!(prior.lineage, Lineage::Precedent);
        assert_eq!(later.lineage, Lineage::Derivative);
        assert!(prior.similarity.unwrap() > 0.7);

        // Centre→neighbour similarity edges, plus a neighbour↔neighbour edge since
        // prior and later are both close to the focus (and so to each other).
        let has = |s: &str, t: &str| {
            graph.edges.iter().any(|e| {
                e.edge_type == "similarity"
                    && ((e.source == s && e.target == t) || (e.source == t && e.target == s))
            })
        };
        assert!(has("focus", "prior") && has("focus", "later"));
        assert!(has("prior", "later"), "neighbours similar to each other are linked");
    }

    #[test]
    fn a_cited_paper_joins_even_without_a_strong_embedding_match() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        add_paper(&conn, "focus", "Focus", Some(2018));
        add_paper(&conn, "cited", "Cited but distant", Some(2010));
        add_vector(&conn, "focus", &vector(0, 1, 0.05));
        add_vector(&conn, "cited", &vector(200, 201, 0.1)); // far in embedding space

        crate::relations::add_manual_relation(&conn, "focus", "cited").unwrap();
        // Turn the manual edge into a citation edge for the test.
        conn.execute(
            "UPDATE relations SET relation_type = 'citation' WHERE target_paper_id = 'cited'",
            [],
        )
        .unwrap();

        let graph = load(&conn, "focus").unwrap();

        let cited = graph.nodes.iter().find(|n| n.id == "cited").unwrap();
        assert!(cited.cites_focus);
        assert_eq!(cited.lineage, Lineage::Precedent);
        assert!(cited.similarity.is_none(), "it is here for the citation, not similarity");
        assert!(graph
            .edges
            .iter()
            .any(|e| e.edge_type == "citation" && e.target == "cited"));
    }
}
