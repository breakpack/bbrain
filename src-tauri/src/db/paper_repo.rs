use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::time::now_iso8601;

/// Processing states from DEVELOPMENT.md §7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Copying,
    Extracting,
    Indexing,
    WaitingForAi,
    Analyzing,
    Ready,
    Partial,
    Failed,
}

impl ImportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copying => "copying",
            Self::Extracting => "extracting",
            Self::Indexing => "indexing",
            Self::WaitingForAi => "waiting_for_ai",
            Self::Analyzing => "analyzing",
            Self::Ready => "ready",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "copying" => Self::Copying,
            "extracting" => Self::Extracting,
            "indexing" => Self::Indexing,
            "waiting_for_ai" => Self::WaitingForAi,
            "analyzing" => Self::Analyzing,
            "ready" => Self::Ready,
            "partial" => Self::Partial,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paper {
    pub id: String,
    pub sha256: String,
    pub title: String,
    pub import_status: ImportStatus,
    pub page_count: Option<i64>,
    pub is_favorite: bool,
    pub last_opened_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub authors: Vec<String>,
    pub year: Option<i64>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub abstract_text: Option<String>,
    pub group_ids: Vec<String>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub display_name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub sort_order: i64,
    pub paper_count: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperPatch {
    pub title: Option<String>,
    pub is_favorite: Option<bool>,
    pub year: Option<i64>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub authors: Option<Vec<String>>,
    pub group_ids: Option<Vec<String>>,
    /// Display names; normalization and creation happen here.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
    /// `all` | `inbox` | `favorites` | `processing` | `failed`
    pub view: Option<String>,
    pub group_id: Option<String>,
    pub tag_ids: Option<Vec<String>>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    pub status: Option<ImportStatus>,
    pub search: Option<String>,
    /// `recent` | `opened` | `title` | `year`
    pub sort: Option<String>,
}

pub fn insert(
    conn: &Connection,
    id: &str,
    sha256: &str,
    managed_path: &str,
    title: &str,
    status: ImportStatus,
) -> Result<()> {
    let now = now_iso8601();
    conn.execute(
        "INSERT INTO papers (id, sha256, managed_path, title, import_status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![id, sha256, managed_path, title, status.as_str(), now],
    )?;
    conn.execute(
        "INSERT INTO paper_metadata (paper_id, source) VALUES (?1, 'heuristic')",
        params![id],
    )?;
    reindex_fts(conn, id)?;
    Ok(())
}

pub fn find_by_hash(conn: &Connection, sha256: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT id FROM papers WHERE sha256 = ?1", params![sha256], |row| {
            row.get(0)
        })
        .optional()?)
}

pub fn managed_path(conn: &Connection, paper_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT managed_path FROM papers WHERE id = ?1",
        params![paper_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound(format!("paper {paper_id}")))
}

pub fn set_status(conn: &Connection, paper_id: &str, status: ImportStatus) -> Result<()> {
    conn.execute(
        "UPDATE papers SET import_status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.as_str(), now_iso8601(), paper_id],
    )?;
    Ok(())
}

pub fn set_page_count(conn: &Connection, paper_id: &str, pages: i64) -> Result<()> {
    conn.execute(
        "UPDATE papers SET page_count = ?1, updated_at = ?2 WHERE id = ?3",
        params![pages, now_iso8601(), paper_id],
    )?;
    Ok(())
}

pub fn touch_opened(conn: &Connection, paper_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE papers SET last_opened_at = ?1 WHERE id = ?2",
        params![now_iso8601(), paper_id],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, paper_id: &str) -> Result<Paper> {
    let mut paper = conn
        .query_row(
            "SELECT p.id, p.sha256, p.title, p.import_status, p.page_count, p.is_favorite,
                    p.last_opened_at, p.created_at, p.updated_at,
                    m.authors, m.year, m.venue, m.doi, m.abstract
             FROM papers p
             LEFT JOIN paper_metadata m ON m.paper_id = p.id
             WHERE p.id = ?1",
            params![paper_id],
            row_to_paper,
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("paper {paper_id}")))?;

    paper.group_ids = group_ids(conn, paper_id)?;
    paper.tags = tags_of(conn, paper_id)?;
    Ok(paper)
}

pub fn list(conn: &Connection, query: &LibraryQuery) -> Result<Vec<Paper>> {
    let mut sql = String::from(
        "SELECT p.id, p.sha256, p.title, p.import_status, p.page_count, p.is_favorite,
                p.last_opened_at, p.created_at, p.updated_at,
                m.authors, m.year, m.venue, m.doi, m.abstract
         FROM papers p
         LEFT JOIN paper_metadata m ON m.paper_id = p.id
         WHERE 1 = 1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    match query.view.as_deref() {
        Some("favorites") => sql.push_str(" AND p.is_favorite = 1"),
        // Inbox: imported but not yet filed into any group.
        Some("inbox") => sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM paper_groups g WHERE g.paper_id = p.id)",
        ),
        Some("processing") => sql.push_str(
            " AND p.import_status IN ('copying','extracting','indexing','waiting_for_ai','analyzing')",
        ),
        Some("failed") => sql.push_str(" AND p.import_status = 'failed'"),
        _ => {}
    }

    if let Some(group_id) = &query.group_id {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM paper_groups g WHERE g.paper_id = p.id AND g.group_id = ?)",
        );
        binds.push(Box::new(group_id.clone()));
    }

    if let Some(tag_ids) = &query.tag_ids {
        // Every selected tag must be present (intersection, not union).
        for tag_id in tag_ids {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM paper_tags t WHERE t.paper_id = p.id AND t.tag_id = ?)",
            );
            binds.push(Box::new(tag_id.clone()));
        }
    }

    if let Some(year_from) = query.year_from {
        sql.push_str(" AND m.year >= ?");
        binds.push(Box::new(year_from));
    }
    if let Some(year_to) = query.year_to {
        sql.push_str(" AND m.year <= ?");
        binds.push(Box::new(year_to));
    }
    if let Some(status) = query.status {
        sql.push_str(" AND p.import_status = ?");
        binds.push(Box::new(status.as_str().to_string()));
    }
    if let Some(search) = query.search.as_ref().filter(|s| !s.trim().is_empty()) {
        sql.push_str(
            " AND p.id IN (SELECT paper_id FROM papers_fts WHERE papers_fts MATCH ?)",
        );
        binds.push(Box::new(fts_query(search)));
    }

    sql.push_str(match query.sort.as_deref() {
        Some("title") => " ORDER BY p.title COLLATE NOCASE ASC",
        Some("year") => " ORDER BY m.year DESC NULLS LAST, p.created_at DESC",
        Some("opened") => " ORDER BY p.last_opened_at DESC NULLS LAST",
        _ => " ORDER BY p.created_at DESC",
    });

    let mut statement = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let rows = statement.query_map(refs.as_slice(), row_to_paper)?;

    let mut papers = Vec::new();
    for paper in rows {
        let mut paper = paper?;
        paper.group_ids = group_ids(conn, &paper.id)?;
        paper.tags = tags_of(conn, &paper.id)?;
        papers.push(paper);
    }
    Ok(papers)
}

/// FTS5 treats bare punctuation as syntax. Quote each term so a user typing
/// `transformer: attention` cannot produce a syntax error.
fn fts_query(search: &str) -> String {
    search
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn row_to_paper(row: &Row<'_>) -> rusqlite::Result<Paper> {
    let status: String = row.get(3)?;
    let authors: Option<String> = row.get(9)?;

    Ok(Paper {
        id: row.get(0)?,
        sha256: row.get(1)?,
        title: row.get(2)?,
        import_status: ImportStatus::from_str(&status).unwrap_or(ImportStatus::Failed),
        page_count: row.get(4)?,
        is_favorite: row.get::<_, i64>(5)? != 0,
        last_opened_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        authors: authors
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        year: row.get(10)?,
        venue: row.get(11)?,
        doi: row.get(12)?,
        abstract_text: row.get(13)?,
        group_ids: Vec::new(),
        tags: Vec::new(),
    })
}

pub fn update(conn: &mut Connection, paper_id: &str, patch: &PaperPatch) -> Result<Paper> {
    let tx = conn.transaction()?;
    let now = now_iso8601();

    if let Some(title) = &patch.title {
        // A hand-typed title outranks anything detected inside the PDF: once
        // marked 'user', `apply_detected_title` never touches it again.
        tx.execute(
            "UPDATE papers SET title = ?1, title_source = 'user', updated_at = ?2 WHERE id = ?3",
            params![title, now, paper_id],
        )?;
    }
    if let Some(favorite) = patch.is_favorite {
        tx.execute(
            "UPDATE papers SET is_favorite = ?1, updated_at = ?2 WHERE id = ?3",
            params![favorite as i64, now, paper_id],
        )?;
    }
    if let Some(year) = patch.year {
        tx.execute(
            "UPDATE paper_metadata SET year = ?1, source = 'user' WHERE paper_id = ?2",
            params![year, paper_id],
        )?;
    }
    if let Some(venue) = &patch.venue {
        tx.execute(
            "UPDATE paper_metadata SET venue = ?1, source = 'user' WHERE paper_id = ?2",
            params![venue, paper_id],
        )?;
    }
    if let Some(doi) = &patch.doi {
        tx.execute(
            "UPDATE paper_metadata SET doi = ?1, source = 'user' WHERE paper_id = ?2",
            params![doi, paper_id],
        )?;
    }
    if let Some(authors) = &patch.authors {
        tx.execute(
            "UPDATE paper_metadata SET authors = ?1, source = 'user' WHERE paper_id = ?2",
            params![serde_json::to_string(authors).unwrap_or_else(|_| "[]".into()), paper_id],
        )?;
    }
    if let Some(group_ids) = &patch.group_ids {
        tx.execute("DELETE FROM paper_groups WHERE paper_id = ?1", params![paper_id])?;
        for group_id in group_ids {
            tx.execute(
                "INSERT OR IGNORE INTO paper_groups (paper_id, group_id) VALUES (?1, ?2)",
                params![paper_id, group_id],
            )?;
        }
    }
    if let Some(tags) = &patch.tags {
        tx.execute("DELETE FROM paper_tags WHERE paper_id = ?1", params![paper_id])?;
        for display_name in tags {
            let tag_id = upsert_tag(&tx, display_name, "user")?;
            tx.execute(
                "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id) VALUES (?1, ?2)",
                params![paper_id, tag_id],
            )?;
        }
    }

    tx.commit()?;
    reindex_fts(conn, paper_id)?;
    get(conn, paper_id)
}

/// Tags are matched case- and whitespace-insensitively so `RAG` and `rag` are
/// the same tag, while the user's original casing is what gets displayed.
pub fn upsert_tag(conn: &Connection, display_name: &str, source: &str) -> Result<String> {
    let normalized = normalize_tag(display_name);
    if normalized.is_empty() {
        return Err(AppError::InvalidInput("empty tag".into()));
    }

    if let Some(id) = conn
        .query_row(
            "SELECT id FROM tags WHERE normalized_name = ?1",
            params![normalized],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(id);
    }

    let id = crate::ids::new_id();
    conn.execute(
        "INSERT INTO tags (id, normalized_name, display_name, source) VALUES (?1, ?2, ?3, ?4)",
        params![id, normalized, display_name.trim(), source],
    )?;
    Ok(id)
}

pub fn normalize_tag(name: &str) -> String {
    name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn tags_of(conn: &Connection, paper_id: &str) -> Result<Vec<Tag>> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.display_name, t.source
         FROM tags t JOIN paper_tags pt ON pt.tag_id = t.id
         WHERE pt.paper_id = ?1
         ORDER BY t.display_name COLLATE NOCASE",
    )?;
    let rows = statement.query_map(params![paper_id], |row| {
        Ok(Tag {
            id: row.get(0)?,
            display_name: row.get(1)?,
            source: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn all_tags(conn: &Connection) -> Result<Vec<Tag>> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.display_name, t.source FROM tags t
         WHERE EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.tag_id = t.id)
         ORDER BY t.display_name COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Tag {
            id: row.get(0)?,
            display_name: row.get(1)?,
            source: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn group_ids(conn: &Connection, paper_id: &str) -> Result<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT group_id FROM paper_groups WHERE paper_id = ?1")?;
    let rows = statement.query_map(params![paper_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn add_to_group(conn: &Connection, paper_id: &str, group_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO paper_groups (paper_id, group_id) VALUES (?1, ?2)",
        params![paper_id, group_id],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, paper_id: &str) -> Result<String> {
    let path = managed_path(conn, paper_id)?;
    // vec0 virtual tables sit outside foreign-key cascades; left behind, their
    // rows resurface as KNN neighbours pointing at deleted papers and break
    // the relations FOREIGN KEY. Chunk vectors go first — the cascade below
    // removes the chunks this subquery reads.
    conn.execute(
        "DELETE FROM chunk_vectors
         WHERE chunk_id IN (SELECT id FROM chunks WHERE paper_id = ?1)",
        params![paper_id],
    )?;
    conn.execute(
        "DELETE FROM paper_vectors WHERE paper_id = ?1",
        params![paper_id],
    )?;
    conn.execute("DELETE FROM papers WHERE id = ?1", params![paper_id])?;
    conn.execute("DELETE FROM papers_fts WHERE paper_id = ?1", params![paper_id])?;
    Ok(path)
}

/// Replaces the filename-derived placeholder with the title found inside the
/// PDF (metadata or page-1 layout). A title the user typed always wins — rows
/// whose `title_source` is 'user' are left untouched. Returns whether the
/// title actually changed.
pub fn apply_detected_title(conn: &Connection, paper_id: &str, title: &str) -> Result<bool> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Ok(false);
    }

    let changed = conn.execute(
        "UPDATE papers SET title = ?1, title_source = 'detected', updated_at = ?2
         WHERE id = ?3 AND title_source != 'user' AND title != ?1",
        params![title, now_iso8601(), paper_id],
    )?;

    if changed > 0 {
        reindex_fts(conn, paper_id)?;
    }
    Ok(changed > 0)
}

/// FTS5 external-content tables need explicit maintenance; Bbrain keeps the
/// index in step by deleting and re-inserting the row after any metadata write.
pub fn reindex_fts(conn: &Connection, paper_id: &str) -> Result<()> {
    conn.execute("DELETE FROM papers_fts WHERE paper_id = ?1", params![paper_id])?;
    conn.execute(
        "INSERT INTO papers_fts (paper_id, title, authors, abstract)
         SELECT p.id, p.title, COALESCE(m.authors, ''), COALESCE(m.abstract, '')
         FROM papers p LEFT JOIN paper_metadata m ON m.paper_id = p.id
         WHERE p.id = ?1",
        params![paper_id],
    )?;
    Ok(())
}

// --- groups -----------------------------------------------------------------

pub fn create_group(conn: &Connection, name: &str, color: Option<&str>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("empty group name".into()));
    }
    let id = crate::ids::new_id();
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM groups",
        [],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO groups (id, name, color, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, color, next_order, now_iso8601()],
    )?;
    Ok(id)
}

pub fn list_groups(conn: &Connection) -> Result<Vec<Group>> {
    let mut statement = conn.prepare(
        "SELECT g.id, g.name, g.color, g.sort_order,
                (SELECT COUNT(*) FROM paper_groups pg WHERE pg.group_id = g.id)
         FROM groups g ORDER BY g.sort_order",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Group {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            sort_order: row.get(3)?,
            paper_count: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub sort_order: Option<i64>,
}

pub fn update_group(conn: &Connection, group_id: &str, patch: &GroupPatch) -> Result<()> {
    if let Some(name) = &patch.name {
        conn.execute(
            "UPDATE groups SET name = ?1 WHERE id = ?2",
            params![name.trim(), group_id],
        )?;
    }
    if let Some(color) = &patch.color {
        conn.execute(
            "UPDATE groups SET color = ?1 WHERE id = ?2",
            params![color, group_id],
        )?;
    }
    if let Some(sort_order) = patch.sort_order {
        conn.execute(
            "UPDATE groups SET sort_order = ?1 WHERE id = ?2",
            params![sort_order, group_id],
        )?;
    }
    Ok(())
}

/// Deleting a group never deletes papers — it only unfiles them.
pub fn delete_group(conn: &Connection, group_id: &str) -> Result<()> {
    conn.execute("DELETE FROM groups WHERE id = ?1", params![group_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn seed(conn: &Connection, id: &str, title: &str) {
        insert(conn, id, id, &format!("/tmp/{id}.pdf"), title, ImportStatus::Ready).unwrap();
    }

    #[test]
    fn finds_a_paper_by_content_hash() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        insert(&conn, "p1", "abc123", "/tmp/p1.pdf", "Attention", ImportStatus::Ready).unwrap();

        assert_eq!(find_by_hash(&conn, "abc123").unwrap().as_deref(), Some("p1"));
        assert!(find_by_hash(&conn, "other").unwrap().is_none());
    }

    #[test]
    fn tags_normalize_to_one_row_across_casing_and_spacing() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        let a = upsert_tag(&conn, "RAG", "user").unwrap();
        let b = upsert_tag(&conn, "  rag  ", "ai").unwrap();

        assert_eq!(a, b);
        assert_eq!(normalize_tag("Neural  Search"), "neural search");
    }

    #[test]
    fn inbox_holds_only_papers_that_belong_to_no_group() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn();
            seed(&conn, "p1", "Filed");
            seed(&conn, "p2", "Unfiled");
            let group = create_group(&conn, "Reading List", None).unwrap();
            add_to_group(&conn, "p1", &group).unwrap();
        }

        let conn = db.conn();
        let inbox = list(
            &conn,
            &LibraryQuery {
                view: Some("inbox".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].id, "p2");
    }

    #[test]
    fn tag_filters_intersect_rather_than_union() {
        let db = Database::open_in_memory().unwrap();
        let (rag, eval) = {
            let conn = db.conn();
            seed(&conn, "p1", "Both");
            seed(&conn, "p2", "Only rag");
            (
                upsert_tag(&conn, "rag", "user").unwrap(),
                upsert_tag(&conn, "eval", "user").unwrap(),
            )
        };

        update(
            &mut db.conn(),
            "p1",
            &PaperPatch {
                tags: Some(vec!["rag".into(), "eval".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        update(
            &mut db.conn(),
            "p2",
            &PaperPatch {
                tags: Some(vec!["rag".into()]),
                ..Default::default()
            },
        )
        .unwrap();

        let conn = db.conn();
        let both = list(
            &conn,
            &LibraryQuery {
                tag_ids: Some(vec![rag.clone(), eval]),
                ..Default::default()
            },
        )
        .unwrap();
        let either = list(
            &conn,
            &LibraryQuery {
                tag_ids: Some(vec![rag]),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(both.len(), 1);
        assert_eq!(both[0].id, "p1");
        assert_eq!(either.len(), 2);
    }

    #[test]
    fn search_matches_title_and_tolerates_punctuation() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "p1", "Attention Is All You Need");
        seed(&conn, "p2", "Dense Passage Retrieval");

        let hits = list(
            &conn,
            &LibraryQuery {
                search: Some("attention:".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "p1");
    }

    #[test]
    fn year_range_filters_are_inclusive() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn();
            seed(&conn, "p1", "Old");
            seed(&conn, "p2", "New");
        }
        update(&mut db.conn(), "p1", &PaperPatch { year: Some(2017), ..Default::default() }).unwrap();
        update(&mut db.conn(), "p2", &PaperPatch { year: Some(2026), ..Default::default() }).unwrap();

        let conn = db.conn();
        let recent = list(
            &conn,
            &LibraryQuery {
                year_from: Some(2020),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "p2");
    }

    #[test]
    fn deleting_a_group_unfiles_papers_without_deleting_them() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "p1", "Paper");
        let group = create_group(&conn, "Reading List", None).unwrap();
        add_to_group(&conn, "p1", &group).unwrap();

        delete_group(&conn, &group).unwrap();

        assert!(get(&conn, "p1").is_ok());
        assert!(get(&conn, "p1").unwrap().group_ids.is_empty());
    }

    #[test]
    fn deleting_a_paper_returns_its_managed_path_and_clears_the_index() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "p1", "Paper");

        let path = delete(&conn, "p1").unwrap();

        assert_eq!(path, "/tmp/p1.pdf");
        let indexed: i64 = conn
            .query_row("SELECT count(*) FROM papers_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed, 0);
    }

    /// PDF 안에서 찾은 제목은 파일명 유래 제목을 교체하지만, 사용자가 직접
    /// 지은 제목은 절대 건드리지 않는다.
    #[test]
    fn a_detected_title_replaces_the_filename_but_never_a_user_rename() {
        let db = Database::open_in_memory().unwrap();
        let mut conn = db.conn();
        seed(&conn, "p1", "downloaded file 2301.00001");

        assert!(apply_detected_title(&conn, "p1", "  Attention Is\n All You Need ").unwrap());
        assert_eq!(get(&conn, "p1").unwrap().title, "Attention Is All You Need");

        // Same title again: no-op, reported as unchanged.
        assert!(!apply_detected_title(&conn, "p1", "Attention Is All You Need").unwrap());

        // The user renames the paper — a later re-extraction must not undo it.
        update(
            &mut conn,
            "p1",
            &PaperPatch {
                title: Some("내가 정한 제목".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!apply_detected_title(&conn, "p1", "Attention Is All You Need").unwrap());
        assert_eq!(get(&conn, "p1").unwrap().title, "내가 정한 제목");

        // An empty detection never blanks the title.
        assert!(!apply_detected_title(&conn, "p1", "   ").unwrap());
    }

    /// vec0 테이블은 FK cascade가 적용되지 않아 명시적으로 지워야 한다.
    /// 남은 고아 벡터는 KNN 이웃으로 되살아나 relations FK 위반을 일으킨다.
    #[test]
    fn deleting_a_paper_removes_its_vector_rows_too() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "p1", "Paper");

        let vector = crate::rag::embedder::to_blob(&vec![0.1f32; 384]);
        conn.execute(
            "INSERT INTO chunks (id, paper_id, page_start, page_end, text, token_count, content_hash)
             VALUES ('c1', 'p1', 1, 1, 'text', 3, 'h')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_vectors (paper_id, index_generation, embedding) VALUES ('p1', 1, ?1)",
            params![vector],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunk_vectors (chunk_id, index_generation, embedding) VALUES ('c1', 1, ?1)",
            params![vector],
        )
        .unwrap();

        delete(&conn, "p1").unwrap();

        let paper_vectors: i64 = conn
            .query_row("SELECT count(*) FROM paper_vectors", [], |row| row.get(0))
            .unwrap();
        let chunk_vectors: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vectors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(paper_vectors, 0);
        assert_eq!(chunk_vectors, 0);
    }

    #[test]
    fn sorting_by_title_is_case_insensitive() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "p1", "beta");
        seed(&conn, "p2", "Alpha");

        let sorted = list(
            &conn,
            &LibraryQuery {
                sort: Some("title".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(sorted[0].title, "Alpha");
    }
}
