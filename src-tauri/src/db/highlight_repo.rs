use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::page_repo::NormalizedRect;
use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::time::now_iso8601;

/// The five predefined colors (DEVELOPMENT.md §9.5). Free-form colors are
/// rejected so highlights stay consistent with the design system.
pub const COLORS: [&str; 5] = ["yellow", "green", "blue", "pink", "purple"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub id: String,
    pub paper_id: String,
    pub page_number: i64,
    pub group_key: Option<String>,
    pub color: String,
    pub selected_text: String,
    pub rects: Vec<NormalizedRect>,
    pub note: Option<String>,
    pub created_at: String,
}

/// One user selection. A selection spanning pages arrives as several page
/// entries sharing a group key, stored as one row per page (§9.5).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveHighlightInput {
    pub paper_id: String,
    pub color: String,
    pub pages: Vec<HighlightPage>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightPage {
    pub page_number: i64,
    pub selected_text: String,
    pub rects: Vec<NormalizedRect>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightPatch {
    pub color: Option<String>,
    pub note: Option<String>,
    /// Word-level deletion trims a highlight rather than removing it: the caller
    /// sends the remaining rectangles and text for this one page's row.
    pub rects: Option<Vec<NormalizedRect>>,
    pub selected_text: Option<String>,
}

fn validate_color(color: &str) -> Result<()> {
    if COLORS.contains(&color) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!("unknown highlight color: {color}")))
    }
}

pub fn save(conn: &mut Connection, input: &SaveHighlightInput) -> Result<Vec<String>> {
    validate_color(&input.color)?;

    let pages: Vec<&HighlightPage> = input
        .pages
        .iter()
        .filter(|page| page.rects.iter().any(|rect| rect.clamped().is_valid()))
        .collect();

    if pages.is_empty() {
        return Err(AppError::InvalidInput("highlight has no usable rectangles".into()));
    }

    // Only a genuinely multi-page selection needs a group key.
    let group_key = (pages.len() > 1).then(new_id);
    let now = now_iso8601();

    let tx = conn.transaction()?;
    let mut ids = Vec::new();

    for page in pages {
        let rects: Vec<NormalizedRect> = page
            .rects
            .iter()
            .map(|rect| rect.clamped())
            .filter(|rect| rect.is_valid())
            .collect();

        let id = new_id();
        tx.execute(
            "INSERT INTO highlights
               (id, paper_id, page_number, group_key, color, selected_text, normalized_rects,
                note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                id,
                input.paper_id,
                page.page_number,
                group_key,
                input.color,
                page.selected_text,
                serde_json::to_string(&rects).unwrap_or_else(|_| "[]".into()),
                input.note,
                now
            ],
        )?;
        ids.push(id);
    }

    tx.commit()?;
    Ok(ids)
}

pub fn list(conn: &Connection, paper_id: &str) -> Result<Vec<Highlight>> {
    let mut statement = conn.prepare(
        "SELECT id, paper_id, page_number, group_key, color, selected_text, normalized_rects,
                note, created_at
         FROM highlights WHERE paper_id = ?1
         ORDER BY page_number, created_at",
    )?;
    let rows = statement.query_map(params![paper_id], |row| {
        let rects: String = row.get(6)?;
        Ok(Highlight {
            id: row.get(0)?,
            paper_id: row.get(1)?,
            page_number: row.get(2)?,
            group_key: row.get(3)?,
            color: row.get(4)?,
            selected_text: row.get(5)?,
            rects: serde_json::from_str(&rects).unwrap_or_default(),
            note: row.get(7)?,
            created_at: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Editing a highlight that is part of a multi-page selection updates the whole
/// selection, which is what the user drew.
pub fn update(conn: &Connection, highlight_id: &str, patch: &HighlightPatch) -> Result<()> {
    if let Some(color) = &patch.color {
        validate_color(color)?;
    }

    let group_key: Option<String> = conn
        .query_row(
            "SELECT group_key FROM highlights WHERE id = ?1",
            params![highlight_id],
            |row| row.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("highlight {highlight_id}")))?;

    let now = now_iso8601();

    if let Some(color) = &patch.color {
        match &group_key {
            Some(key) => conn.execute(
                "UPDATE highlights SET color = ?1, updated_at = ?2 WHERE group_key = ?3",
                params![color, now, key],
            )?,
            None => conn.execute(
                "UPDATE highlights SET color = ?1, updated_at = ?2 WHERE id = ?3",
                params![color, now, highlight_id],
            )?,
        };
    }

    if let Some(note) = &patch.note {
        conn.execute(
            "UPDATE highlights SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now, highlight_id],
        )?;
    }

    // Trimming (word deletion) rewrites this row's geometry and text only — it
    // never touches the rest of a multi-page group.
    if let Some(rects) = &patch.rects {
        let cleaned: Vec<NormalizedRect> = rects
            .iter()
            .map(|rect| rect.clamped())
            .filter(|rect| rect.is_valid())
            .collect();
        if cleaned.is_empty() {
            // Nothing left to show — remove the row rather than leave an empty one.
            conn.execute("DELETE FROM highlights WHERE id = ?1", params![highlight_id])?;
            return Ok(());
        }
        conn.execute(
            "UPDATE highlights SET normalized_rects = ?1, selected_text = ?2, updated_at = ?3
             WHERE id = ?4",
            params![
                serde_json::to_string(&cleaned).unwrap_or_else(|_| "[]".into()),
                patch.selected_text.clone().unwrap_or_default(),
                now,
                highlight_id
            ],
        )?;
    }

    Ok(())
}

pub fn delete(conn: &Connection, highlight_id: &str) -> Result<()> {
    let group_key: Option<String> = conn
        .query_row(
            "SELECT group_key FROM highlights WHERE id = ?1",
            params![highlight_id],
            |row| row.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("highlight {highlight_id}")))?;

    match group_key {
        Some(key) => conn.execute("DELETE FROM highlights WHERE group_key = ?1", params![key])?,
        None => conn.execute("DELETE FROM highlights WHERE id = ?1", params![highlight_id])?,
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn rect(y: f64) -> NormalizedRect {
        NormalizedRect { x: 0.1, y, width: 0.6, height: 0.02 }
    }

    fn seeded() -> Database {
        let db = Database::open_in_memory().unwrap();
        paper_repo::insert(&db.conn(), "p1", "h", "/tmp/p1.pdf", "Paper", ImportStatus::Ready)
            .unwrap();
        db
    }

    fn single_page(color: &str) -> SaveHighlightInput {
        SaveHighlightInput {
            paper_id: "p1".into(),
            color: color.into(),
            note: None,
            pages: vec![HighlightPage {
                page_number: 3,
                selected_text: "retrieval augmented generation".into(),
                rects: vec![rect(0.2)],
            }],
        }
    }

    #[test]
    fn saves_and_reloads_a_highlight_with_its_coordinates() {
        let db = seeded();
        save(&mut db.conn(), &single_page("yellow")).unwrap();

        let highlights = list(&db.conn(), "p1").unwrap();

        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].page_number, 3);
        assert_eq!(highlights[0].color, "yellow");
        assert_eq!(highlights[0].rects[0].y, 0.2);
        assert!(highlights[0].group_key.is_none());
    }

    #[test]
    fn a_selection_across_pages_is_stored_per_page_under_one_group() {
        let db = seeded();
        save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "green".into(),
                note: None,
                pages: vec![
                    HighlightPage {
                        page_number: 3,
                        selected_text: "end of page three".into(),
                        rects: vec![rect(0.9)],
                    },
                    HighlightPage {
                        page_number: 4,
                        selected_text: "start of page four".into(),
                        rects: vec![rect(0.05)],
                    },
                ],
            },
        )
        .unwrap();

        let highlights = list(&db.conn(), "p1").unwrap();

        assert_eq!(highlights.len(), 2);
        let key = highlights[0].group_key.clone().expect("a shared group key");
        assert_eq!(highlights[1].group_key.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn recoloring_one_page_recolors_the_whole_selection() {
        let db = seeded();
        save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "green".into(),
                note: None,
                pages: vec![
                    HighlightPage { page_number: 3, selected_text: "a".into(), rects: vec![rect(0.9)] },
                    HighlightPage { page_number: 4, selected_text: "b".into(), rects: vec![rect(0.1)] },
                ],
            },
        )
        .unwrap();
        let first = list(&db.conn(), "p1").unwrap()[0].id.clone();

        update(
            &db.conn(),
            &first,
            &HighlightPatch { color: Some("pink".into()), ..Default::default() },
        )
            .unwrap();

        let highlights = list(&db.conn(), "p1").unwrap();
        assert!(highlights.iter().all(|h| h.color == "pink"));
    }

    #[test]
    fn trimming_rewrites_only_this_rows_geometry_and_text() {
        let db = seeded();
        save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "yellow".into(),
                note: None,
                pages: vec![HighlightPage {
                    page_number: 1,
                    selected_text: "alpha beta gamma".into(),
                    rects: vec![rect(0.2)],
                }],
            },
        )
        .unwrap();
        let id = list(&db.conn(), "p1").unwrap()[0].id.clone();

        update(
            &db.conn(),
            &id,
            &HighlightPatch {
                rects: Some(vec![NormalizedRect { x: 0.1, y: 0.2, width: 0.2, height: 0.02 }]),
                selected_text: Some("alpha gamma".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let highlights = list(&db.conn(), "p1").unwrap();
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].selected_text, "alpha gamma");
        assert_eq!(highlights[0].rects.len(), 1);
        assert!((highlights[0].rects[0].width - 0.2).abs() < 1e-9);
    }

    #[test]
    fn trimming_to_no_rectangles_deletes_the_row() {
        let db = seeded();
        save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "yellow".into(),
                note: None,
                pages: vec![HighlightPage {
                    page_number: 1,
                    selected_text: "lone".into(),
                    rects: vec![rect(0.2)],
                }],
            },
        )
        .unwrap();
        let id = list(&db.conn(), "p1").unwrap()[0].id.clone();

        update(
            &db.conn(),
            &id,
            &HighlightPatch { rects: Some(vec![]), selected_text: Some(String::new()), ..Default::default() },
        )
        .unwrap();

        assert!(list(&db.conn(), "p1").unwrap().is_empty());
    }

    #[test]
    fn deleting_one_page_of_a_selection_deletes_the_selection() {
        let db = seeded();
        save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "blue".into(),
                note: None,
                pages: vec![
                    HighlightPage { page_number: 3, selected_text: "a".into(), rects: vec![rect(0.9)] },
                    HighlightPage { page_number: 4, selected_text: "b".into(), rects: vec![rect(0.1)] },
                ],
            },
        )
        .unwrap();
        let first = list(&db.conn(), "p1").unwrap()[0].id.clone();

        delete(&db.conn(), &first).unwrap();

        assert!(list(&db.conn(), "p1").unwrap().is_empty());
    }

    #[test]
    fn rejects_a_color_outside_the_defined_palette() {
        let db = seeded();
        let error = save(&mut db.conn(), &single_page("#ff0000")).unwrap_err();

        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn rejects_a_selection_with_no_usable_rectangles() {
        let db = seeded();
        let error = save(
            &mut db.conn(),
            &SaveHighlightInput {
                paper_id: "p1".into(),
                color: "yellow".into(),
                note: None,
                pages: vec![HighlightPage {
                    page_number: 1,
                    selected_text: "".into(),
                    rects: vec![NormalizedRect { x: 0.5, y: 0.5, width: 0.0, height: 0.0 }],
                }],
            },
        )
        .unwrap_err();

        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn highlights_survive_deleting_and_reopening_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");

        {
            let db = Database::open(&path).unwrap();
            paper_repo::insert(&db.conn(), "p1", "h", "/tmp/p1.pdf", "P", ImportStatus::Ready)
                .unwrap();
            save(&mut db.conn(), &single_page("purple")).unwrap();
        }

        let reopened = Database::open(&path).unwrap();
        let highlights = list(&reopened.conn(), "p1").unwrap();

        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].rects[0].y, 0.2);
    }
}
