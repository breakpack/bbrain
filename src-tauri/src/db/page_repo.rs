use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::ids::new_id;

/// Fraction of the unrotated page box, so a stored rectangle survives zoom,
/// rotation and a re-render (DEVELOPMENT.md §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    /// Guards against a bad extraction writing coordinates that would later
    /// render off-page.
    pub fn is_valid(&self) -> bool {
        let in_unit = |value: f64| (0.0..=1.0).contains(&value);
        in_unit(self.x)
            && in_unit(self.y)
            && self.width > 0.0
            && self.height > 0.0
            && self.x + self.width <= 1.0001
            && self.y + self.height <= 1.0001
    }

    pub fn clamped(self) -> Self {
        let x = self.x.clamp(0.0, 1.0);
        let y = self.y.clamp(0.0, 1.0);
        Self {
            x,
            y,
            width: self.width.clamp(0.0, 1.0 - x),
            height: self.height.clamp(0.0, 1.0 - y),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedSentence {
    pub order_index: i64,
    #[serde(default)]
    pub paragraph_index: i64,
    pub text: String,
    pub rects: Vec<NormalizedRect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedPage {
    pub page_number: i64,
    /// Unrotated page box, in PDF points.
    pub width: f64,
    pub height: f64,
    pub rotation: i64,
    pub text: String,
    pub sentences: Vec<ExtractedSentence>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sentence {
    pub id: String,
    pub page_number: i64,
    pub order_index: i64,
    pub paragraph_index: i64,
    pub text: String,
    pub rects: Vec<NormalizedRect>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub page_number: i64,
    pub width: f64,
    pub height: f64,
    pub rotation: i64,
    pub text_hash: String,
}

pub fn text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Replaces a paper's extraction in one transaction, so a partial write can
/// never leave sentences pointing at a page that no longer exists.
pub fn replace_extraction(conn: &mut Connection, paper_id: &str, pages: &[ExtractedPage]) -> Result<()> {
    let tx = conn.transaction()?;

    tx.execute("DELETE FROM sentences WHERE paper_id = ?1", params![paper_id])?;
    tx.execute("DELETE FROM pages WHERE paper_id = ?1", params![paper_id])?;

    for page in pages {
        tx.execute(
            "INSERT INTO pages (paper_id, page_number, width, height, rotation, text, text_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                paper_id,
                page.page_number,
                page.width,
                page.height,
                page.rotation,
                page.text,
                text_hash(&page.text)
            ],
        )?;

        for sentence in &page.sentences {
            let rects: Vec<NormalizedRect> = sentence
                .rects
                .iter()
                .map(|rect| rect.clamped())
                .filter(|rect| rect.is_valid())
                .collect();
            if rects.is_empty() {
                continue;
            }

            tx.execute(
                "INSERT INTO sentences
                   (id, paper_id, page_number, order_index, paragraph_index,
                    source_text, normalized_rects)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    new_id(),
                    paper_id,
                    page.page_number,
                    sentence.order_index,
                    sentence.paragraph_index,
                    sentence.text,
                    serde_json::to_string(&rects).unwrap_or_else(|_| "[]".into())
                ],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

pub fn page_sentences(conn: &Connection, paper_id: &str, page_number: i64) -> Result<Vec<Sentence>> {
    let mut statement = conn.prepare(
        "SELECT id, page_number, order_index, paragraph_index, source_text, normalized_rects
         FROM sentences WHERE paper_id = ?1 AND page_number = ?2
         ORDER BY order_index",
    )?;
    let rows = statement.query_map(params![paper_id, page_number], |row| {
        let rects: String = row.get(5)?;
        Ok(Sentence {
            id: row.get(0)?,
            page_number: row.get(1)?,
            order_index: row.get(2)?,
            paragraph_index: row.get(3)?,
            text: row.get(4)?,
            rects: serde_json::from_str(&rects).unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn page_text(conn: &Connection, paper_id: &str, page_number: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT text FROM pages WHERE paper_id = ?1 AND page_number = ?2",
            params![paper_id, page_number],
            |row| row.get(0),
        )
        .ok())
}

pub fn page_info(conn: &Connection, paper_id: &str, page_number: i64) -> Result<Option<PageInfo>> {
    Ok(conn
        .query_row(
            "SELECT page_number, width, height, rotation, text_hash
             FROM pages WHERE paper_id = ?1 AND page_number = ?2",
            params![paper_id, page_number],
            |row| {
                Ok(PageInfo {
                    page_number: row.get(0)?,
                    width: row.get(1)?,
                    height: row.get(2)?,
                    rotation: row.get(3)?,
                    text_hash: row.get(4)?,
                })
            },
        )
        .ok())
}

/// Full document text in reading order, used by analysis and chunking.
pub fn document_text(conn: &Connection, paper_id: &str) -> Result<Vec<(i64, String)>> {
    let mut statement = conn.prepare(
        "SELECT page_number, text FROM pages WHERE paper_id = ?1 ORDER BY page_number",
    )?;
    let rows = statement.query_map(params![paper_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// A scan with no text layer yields pages but no words. Bbrain can still display
/// it, but translation, analysis and RAG have nothing to work with (§17).
pub fn has_text_layer(conn: &Connection, paper_id: &str) -> Result<bool> {
    let characters: i64 = conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(TRIM(text))), 0) FROM pages WHERE paper_id = ?1",
        params![paper_id],
        |row| row.get(0),
    )?;
    Ok(characters > 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NormalizedRect {
        NormalizedRect { x, y, width: w, height: h }
    }

    fn page(number: i64, text: &str, sentences: Vec<ExtractedSentence>) -> ExtractedPage {
        ExtractedPage {
            page_number: number,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            text: text.into(),
            sentences,
        }
    }

    fn seeded() -> Database {
        let db = Database::open_in_memory().unwrap();
        paper_repo::insert(
            &db.conn(),
            "p1",
            "hash",
            "/tmp/p1.pdf",
            "Paper",
            ImportStatus::Extracting,
        )
        .unwrap();
        db
    }

    #[test]
    fn stores_pages_and_sentences_in_reading_order() {
        let db = seeded();
        let pages = vec![page(
            1,
            "First sentence. Second sentence.",
            vec![
                ExtractedSentence {
                    order_index: 0,
                    paragraph_index: 0,
                    text: "First sentence.".into(),
                    rects: vec![rect(0.1, 0.1, 0.5, 0.02)],
                },
                ExtractedSentence {
                    order_index: 1,
                    paragraph_index: 0,
                    text: "Second sentence.".into(),
                    rects: vec![rect(0.1, 0.13, 0.5, 0.02)],
                },
            ],
        )];

        replace_extraction(&mut db.conn(), "p1", &pages).unwrap();

        let sentences = page_sentences(&db.conn(), "p1", 1).unwrap();
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].text, "First sentence.");
        assert_eq!(sentences[1].order_index, 1);
        assert_eq!(sentences[0].rects[0].x, 0.1);
    }

    #[test]
    fn re_extraction_replaces_rather_than_duplicates() {
        let db = seeded();
        let pages = vec![page(
            1,
            "Text.",
            vec![ExtractedSentence {
                order_index: 0,
                paragraph_index: 0,
                text: "Text.".into(),
                rects: vec![rect(0.0, 0.0, 1.0, 0.05)],
            }],
        )];

        replace_extraction(&mut db.conn(), "p1", &pages).unwrap();
        replace_extraction(&mut db.conn(), "p1", &pages).unwrap();

        assert_eq!(page_sentences(&db.conn(), "p1", 1).unwrap().len(), 1);
    }

    #[test]
    fn out_of_range_rectangles_are_clamped_and_empty_ones_dropped() {
        let db = seeded();
        let pages = vec![page(
            1,
            "Text.",
            vec![
                ExtractedSentence {
                    order_index: 0,
                    paragraph_index: 0,
                    text: "Overflowing".into(),
                    rects: vec![rect(0.9, 0.9, 0.5, 0.5)],
                },
                ExtractedSentence {
                    order_index: 1,
                    paragraph_index: 0,
                    text: "Degenerate".into(),
                    rects: vec![rect(0.5, 0.5, 0.0, 0.0)],
                },
            ],
        )];

        replace_extraction(&mut db.conn(), "p1", &pages).unwrap();

        let sentences = page_sentences(&db.conn(), "p1", 1).unwrap();
        assert_eq!(sentences.len(), 1, "a sentence with no usable rect is dropped");
        let r = sentences[0].rects[0];
        assert!(r.x + r.width <= 1.0001 && r.y + r.height <= 1.0001);
    }

    #[test]
    fn the_page_text_hash_keys_the_translation_cache() {
        let db = seeded();
        replace_extraction(&mut db.conn(), "p1", &[page(1, "Hello", vec![])]).unwrap();
        let first = page_info(&db.conn(), "p1", 1).unwrap().unwrap().text_hash;

        replace_extraction(&mut db.conn(), "p1", &[page(1, "Hello world", vec![])]).unwrap();
        let second = page_info(&db.conn(), "p1", 1).unwrap().unwrap().text_hash;

        assert_ne!(first, second, "changed page text must invalidate the cache key");
        assert_eq!(first, text_hash("Hello"));
    }

    #[test]
    fn a_scan_without_a_text_layer_is_detected() {
        let db = seeded();
        replace_extraction(&mut db.conn(), "p1", &[page(1, "   ", vec![])]).unwrap();

        assert!(!has_text_layer(&db.conn(), "p1").unwrap());
    }

    #[test]
    fn a_text_pdf_reports_a_usable_text_layer() {
        let db = seeded();
        let body = "여러 문장이 담긴 본문. ".repeat(20);
        replace_extraction(&mut db.conn(), "p1", &[page(1, &body, vec![])]).unwrap();

        assert!(has_text_layer(&db.conn(), "p1").unwrap());
    }
}
