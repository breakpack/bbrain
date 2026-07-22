use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::settings_repo;
use crate::error::Result;
use crate::state::AppState;
use crate::time::now_iso8601;

/// Semantic neighbours must clear this cosine similarity, and only the top five
/// are kept (DEVELOPMENT.md §12.1).
pub const SIMILARITY_THRESHOLD: f64 = 0.75;
pub const MAX_NEIGHBOURS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    Semantic,
    Manual,
    Citation,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub source_paper_id: String,
    pub target_paper_id: String,
    pub relation_type: RelationType,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub year: Option<i64>,
    pub group_ids: Vec<String>,
    pub tag_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<Relation>,
}

/// Recomputes this paper's semantic neighbours, plus any citation links that can
/// be resolved inside the library.
pub async fn recompute_for_paper(app: &AppHandle, paper_id: &str) -> Result<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        let generation = settings_repo::get(&conn)?.index_generation;

        recompute_semantic(&conn, paper_id, generation)?;
        recompute_citations(&conn, paper_id)?;
    }

    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

fn recompute_semantic(conn: &Connection, paper_id: &str, generation: i64) -> Result<()> {
    let vector: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM paper_vectors
             WHERE paper_id = ?1 AND index_generation = ?2",
            params![paper_id, generation],
            |row| row.get(0),
        )
        .ok();

    let Some(vector) = vector else {
        tracing::info!(paper = %paper_id, "no paper vector yet; skipping semantic relations");
        return Ok(());
    };

    // Ask for one extra neighbour: the paper always matches itself.
    let mut statement = conn.prepare(
        "SELECT paper_id, distance FROM paper_vectors
         WHERE embedding MATCH ?1 AND k = ?2 AND index_generation = ?3
         ORDER BY distance",
    )?;

    let rows = statement.query_map(
        params![vector, (MAX_NEIGHBOURS + 1) as i64, generation],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
    )?;

    let mut neighbours = Vec::new();
    for row in rows {
        let (neighbour_id, distance) = row?;
        if neighbour_id == paper_id {
            continue;
        }
        // vec0 cosine distance is 1 - similarity.
        let similarity = 1.0 - distance;
        if similarity >= SIMILARITY_THRESHOLD {
            neighbours.push((neighbour_id, similarity));
        }
    }
    neighbours.truncate(MAX_NEIGHBOURS);

    // Only semantic edges are recomputed; manual links the user made survive.
    conn.execute(
        "DELETE FROM relations WHERE source_paper_id = ?1 AND relation_type = 'semantic'",
        params![paper_id],
    )?;

    let now = now_iso8601();
    for (target, score) in neighbours {
        conn.execute(
            "INSERT OR REPLACE INTO relations
               (source_paper_id, target_paper_id, relation_type, score, provenance, created_at)
             VALUES (?1, ?2, 'semantic', ?3, 'paper_embedding', ?4)",
            params![paper_id, target, score, now],
        )?;
    }

    Ok(())
}

/// Citation edges are only created when a reference resolves to a paper the user
/// actually has, by DOI or normalized title (§12.1).
fn recompute_citations(conn: &Connection, paper_id: &str) -> Result<()> {
    let text: String = conn
        .query_row(
            "SELECT COALESCE(GROUP_CONCAT(text, char(10)), '') FROM pages WHERE paper_id = ?1",
            params![paper_id],
            |row| row.get(0),
        )
        .unwrap_or_default();

    if text.is_empty() {
        return Ok(());
    }

    let haystack = normalize_title(&text);
    let mut candidates: Vec<(String, String)> = Vec::new();

    let mut statement = conn.prepare(
        "SELECT p.id, p.title, m.doi FROM papers p
         LEFT JOIN paper_metadata m ON m.paper_id = p.id
         WHERE p.id != ?1",
    )?;
    let rows = statement.query_map(params![paper_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    for row in rows {
        let (other_id, title, doi) = row?;

        let by_doi = doi
            .as_deref()
            .filter(|doi| doi.len() > 6)
            .is_some_and(|doi| text.contains(doi));

        // A short title would match half the library, so require some length.
        let normalized = normalize_title(&title);
        let by_title = normalized.len() >= 20 && haystack.contains(&normalized);

        if by_doi || by_title {
            candidates.push((other_id, if by_doi { "doi" } else { "title" }.to_string()));
        }
    }

    conn.execute(
        "DELETE FROM relations WHERE source_paper_id = ?1 AND relation_type = 'citation'",
        params![paper_id],
    )?;

    let now = now_iso8601();
    for (target, provenance) in candidates {
        conn.execute(
            "INSERT OR REPLACE INTO relations
               (source_paper_id, target_paper_id, relation_type, score, provenance, created_at)
             VALUES (?1, ?2, 'citation', NULL, ?3, ?4)",
            params![paper_id, target, provenance, now],
        )?;
    }

    Ok(())
}

/// Lowercase, collapse whitespace, drop punctuation — so "Attention Is All You
/// Need." and "attention is all you need" are the same title.
pub fn normalize_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn add_manual_relation(conn: &Connection, source: &str, target: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO relations
           (source_paper_id, target_paper_id, relation_type, score, provenance, created_at)
         VALUES (?1, ?2, 'manual', NULL, 'user', ?3)",
        params![source, target, now_iso8601()],
    )?;
    Ok(())
}

pub fn remove_manual_relation(conn: &Connection, source: &str, target: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM relations
         WHERE source_paper_id = ?1 AND target_paper_id = ?2 AND relation_type = 'manual'",
        params![source, target],
    )?;
    Ok(())
}

pub fn related_titles(conn: &Connection, paper_id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT p.title FROM relations r JOIN papers p ON p.id = r.target_paper_id
         WHERE r.source_paper_id = ?1
         ORDER BY r.score DESC",
    )?;
    let rows = statement.query_map(params![paper_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn load_graph(conn: &Connection) -> Result<Graph> {
    let mut node_statement = conn.prepare(
        "SELECT p.id, p.title, m.year FROM papers p
         LEFT JOIN paper_metadata m ON m.paper_id = p.id",
    )?;
    let node_rows = node_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut nodes = Vec::new();
    for (id, title, year) in node_rows {
        let mut group_statement =
            conn.prepare("SELECT group_id FROM paper_groups WHERE paper_id = ?1")?;
        let group_ids = group_statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut tag_statement =
            conn.prepare("SELECT tag_id FROM paper_tags WHERE paper_id = ?1")?;
        let tag_ids = tag_statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        nodes.push(GraphNode {
            id,
            title,
            year,
            group_ids,
            tag_ids,
        });
    }

    let mut edge_statement = conn
        .prepare("SELECT source_paper_id, target_paper_id, relation_type, score FROM relations")?;
    let edges = edge_statement
        .query_map([], |row| {
            let relation_type: String = row.get(2)?;
            Ok(Relation {
                source_paper_id: row.get(0)?,
                target_paper_id: row.get(1)?,
                relation_type: match relation_type.as_str() {
                    "manual" => RelationType::Manual,
                    "citation" => RelationType::Citation,
                    _ => RelationType::Semantic,
                },
                score: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Graph { nodes, edges })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn seeded() -> Database {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn();
            for id in ["p1", "p2"] {
                paper_repo::insert(&conn, id, id, "/tmp/x.pdf", "Paper", ImportStatus::Ready)
                    .unwrap();
            }
        }
        db
    }

    #[test]
    fn titles_normalize_across_case_and_punctuation() {
        assert_eq!(
            normalize_title("Attention Is All You Need."),
            normalize_title("attention is all you  need")
        );
    }

    #[test]
    fn a_manual_link_survives_a_semantic_recompute() {
        let db = seeded();
        let conn = db.conn();

        add_manual_relation(&conn, "p1", "p2").unwrap();
        // No paper vector exists, so the semantic pass is a no-op; the manual
        // edge must still be there.
        recompute_semantic(&conn, "p1", 1).unwrap();

        let graph = load_graph(&conn).unwrap();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].relation_type, RelationType::Manual);
    }

    #[test]
    fn removing_a_manual_link_leaves_the_graph_empty_again() {
        let db = seeded();
        let conn = db.conn();
        add_manual_relation(&conn, "p1", "p2").unwrap();

        remove_manual_relation(&conn, "p1", "p2").unwrap();

        assert!(load_graph(&conn).unwrap().edges.is_empty());
    }

    #[test]
    fn a_citation_edge_appears_only_when_the_cited_paper_is_in_the_library() {
        let db = seeded();
        paper_repo::update(
            &mut db.conn(),
            "p2",
            &paper_repo::PaperPatch {
                title: Some("Dense Passage Retrieval for Open Domain Question Answering".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO pages (paper_id, page_number, width, height, text, text_hash)
             VALUES ('p1', 1, 612, 792,
                     'We build on Dense Passage Retrieval for Open Domain Question Answering.', 'h')",
            [],
        )
        .unwrap();

        recompute_citations(&conn, "p1").unwrap();

        let graph = load_graph(&conn).unwrap();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].relation_type, RelationType::Citation);
        assert_eq!(graph.edges[0].target_paper_id, "p2");
    }

    #[test]
    fn an_unrelated_paper_produces_no_citation_edge() {
        let db = seeded();
        let conn = db.conn();

        conn.execute(
            "INSERT INTO pages (paper_id, page_number, width, height, text, text_hash)
             VALUES ('p1', 1, 612, 792, 'A paper about something else entirely.', 'h')",
            [],
        )
        .unwrap();

        recompute_citations(&conn, "p1").unwrap();

        assert!(load_graph(&conn).unwrap().edges.is_empty());
    }

    #[test]
    fn the_graph_carries_the_nodes_needed_for_filtering() {
        let db = seeded();
        let conn = db.conn();

        let graph = load_graph(&conn).unwrap();

        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.nodes.iter().all(|node| !node.title.is_empty()));
    }
}
