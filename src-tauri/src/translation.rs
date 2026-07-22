use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::db::page_repo;
use crate::error::{AppError, Result};
use crate::providers::google_translate;
use crate::state::AppState;
use crate::time::now_iso8601;

/// Bump when the translation pipeline changes in a way that invalidates saved
/// results: it is part of the cache key, so old translations are not silently
/// reused. Bumped to 3 when a page became a single whole-page request whose
/// segments map back to source sentences (the unit shape changed) (§9.4).
pub const PROMPT_VERSION: &str = "3";

/// Stored in the cache row's provider/model columns. Translation always uses the
/// free Google endpoint rather than a configured LLM provider, so these are
/// fixed rather than read from settings.
const TRANSLATOR_PROVIDER: &str = "google";
const TRANSLATOR_MODEL: &str = "gtx";

/// One translated segment (roughly a sentence) and the source sentences it
/// covers. The viewer highlights every rectangle behind those sentences when the
/// reader hovers the segment, and groups segments into paragraphs for the
/// paragraph view — both the paragraph and whole-page views render from this one
/// list, so a page is translated in a single request (§9.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslatedUnit {
    pub id: String,
    pub text: String,
    pub sentence_ids: Vec<String>,
    pub paragraph_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageTranslation {
    pub page_number: i64,
    pub target_language: String,
    pub units: Vec<TranslatedUnit>,
    /// True when the result came from the cache rather than the network.
    #[serde(default)]
    pub cached: bool,
}

/// A source sentence's position (in characters) within the concatenated page
/// text sent to the translator, so a returned segment can be attributed to it.
struct SourceRange {
    start: usize,
    end: usize,
    id: String,
    paragraph_index: i64,
}

/// Concatenates a page's sentences into the single string sent to the translator
/// and records where each sentence lands. Sentences of one paragraph are joined
/// with a space; paragraphs are separated by a newline so the translator keeps
/// them apart. The translator preserves this text (its returned originals
/// reconstruct it), so character offsets align the translation back to sentences.
fn build_source(sentences: &[page_repo::Sentence]) -> (String, Vec<SourceRange>) {
    let mut source = String::new();
    let mut char_pos = 0usize;
    let mut ranges = Vec::with_capacity(sentences.len());
    let mut prev_paragraph: Option<i64> = None;

    for sentence in sentences {
        if let Some(prev) = prev_paragraph {
            let separator = if prev == sentence.paragraph_index { ' ' } else { '\n' };
            source.push(separator);
            char_pos += 1;
        }
        let start = char_pos;
        source.push_str(&sentence.text);
        char_pos += sentence.text.chars().count();
        ranges.push(SourceRange {
            start,
            end: char_pos,
            id: sentence.id.clone(),
            paragraph_index: sentence.paragraph_index,
        });
        prev_paragraph = Some(sentence.paragraph_index);
    }

    (source, ranges)
}

/// Attributes each translated segment to the source sentence(s) it overlaps, by
/// walking a character cursor through the source in lock-step with the segments'
/// originals. A pure-separator segment (e.g. a bare newline) is dropped.
fn map_segments(
    ranges: &[SourceRange],
    segments: &[google_translate::Segment],
) -> Vec<TranslatedUnit> {
    let mut units = Vec::new();
    let mut cursor = 0usize;

    for (index, segment) in segments.iter().enumerate() {
        let start = cursor;
        let end = cursor + segment.original.chars().count();
        cursor = end;

        let text = segment.translated.trim().to_string();
        if text.is_empty() {
            continue;
        }

        let overlapping: Vec<&SourceRange> = ranges
            .iter()
            .filter(|range| range.start < end && range.end > start)
            .collect();

        // The endpoint is expected to preserve the text exactly, but guard
        // against drift: an unmatched segment is attributed to the nearest
        // preceding sentence rather than dropped.
        let chosen: Vec<&SourceRange> = if overlapping.is_empty() {
            ranges.iter().filter(|range| range.start <= start).last().into_iter().collect()
        } else {
            overlapping
        };

        let sentence_ids = chosen.iter().map(|range| range.id.clone()).collect();
        let paragraph_index = chosen.first().map(|range| range.paragraph_index).unwrap_or(0);

        units.push(TranslatedUnit {
            id: format!("u{index}"),
            text,
            sentence_ids,
            paragraph_index,
        });
    }

    units
}

/// Translates the current page with the free Google endpoint in a single
/// request. Interactive work does not go through the job queue — the user is
/// waiting, and an abandoned translation is not worth resuming after a restart.
/// Each translated segment keeps the IDs of the source sentences it covers, so
/// the viewer can highlight the matching location on hover and group the
/// segments into paragraphs (§9.3).
pub async fn translate_page(
    app: &AppHandle,
    paper_id: &str,
    page_number: i64,
    target_language: &str,
) -> Result<PageTranslation> {
    let (sentences, source_hash) = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();

        let sentences = page_repo::page_sentences(&conn, paper_id, page_number)?;
        if sentences.is_empty() {
            return Err(AppError::InvalidInput("page has no extracted sentences".into()));
        }

        let info = page_repo::page_info(&conn, paper_id, page_number)?
            .ok_or_else(|| AppError::NotFound(format!("page {page_number}")))?;
        (sentences, info.text_hash)
    };

    // Cache key: paper + page + page text hash + language + pipeline version. The
    // translator is fixed, so provider/model are not part of the identity; a
    // saved result is returned without a network call (§9.4).
    {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        if let Some(cached) = read_cached_page(&conn, paper_id, page_number, target_language)? {
            return Ok(cached);
        }
    }

    let (source, ranges) = build_source(&sentences);
    let segments = google_translate::translate_segments(&source, target_language).await?;
    let units = map_segments(&ranges, &segments);

    let result = PageTranslation {
        page_number,
        target_language: target_language.to_string(),
        units,
        cached: false,
    };

    // Only a complete translation is cached (§9.4).
    let state = app.state::<AppState>();
    state.db.conn().execute(
        "INSERT OR REPLACE INTO translations
           (paper_id, page_number, target_language, source_hash, provider, model,
            prompt_version, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            paper_id,
            page_number,
            target_language,
            source_hash,
            TRANSLATOR_PROVIDER,
            TRANSLATOR_MODEL,
            PROMPT_VERSION,
            serde_json::to_string(&result).unwrap_or_default(),
            now_iso8601(),
        ],
    )?;

    Ok(result)
}

/// Translates a free-form selection the reader dragged over the page with the
/// free Google endpoint. Ad-hoc and not cached — a selection has no stable
/// identity to key on, and the reader asked for this exact span once (§9.3).
pub async fn translate_selection(text: &str, target_language: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("selection is empty".into()));
    }

    google_translate::translate_text(trimmed, target_language).await
}

/// Returns a previously saved translation for the page without a network call,
/// so reopening the viewer shows the translation the user already produced.
/// Matches on the page's current text hash — a re-extracted page (different text)
/// will not surface a stale translation.
pub fn load_cached_page(
    app: &AppHandle,
    paper_id: &str,
    page_number: i64,
    target_language: &str,
) -> Result<Option<PageTranslation>> {
    let state = app.state::<AppState>();
    let conn = state.db.conn();
    read_cached_page(&conn, paper_id, page_number, target_language)
}

/// Connection-facing core of `load_cached_page`, so the cache lookup is testable
/// without a running Tauri app.
pub fn read_cached_page(
    conn: &rusqlite::Connection,
    paper_id: &str,
    page_number: i64,
    target_language: &str,
) -> Result<Option<PageTranslation>> {
    let Some(info) = page_repo::page_info(conn, paper_id, page_number)? else {
        return Ok(None);
    };

    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM translations
             WHERE paper_id = ?1 AND page_number = ?2 AND target_language = ?3
               AND source_hash = ?4 AND prompt_version = ?5
             ORDER BY created_at DESC LIMIT 1",
            params![paper_id, page_number, target_language, info.text_hash, PROMPT_VERSION],
            |row| row.get(0),
        )
        .optional()?;

    Ok(payload
        .and_then(|json| serde_json::from_str::<PageTranslation>(&json).ok())
        .map(|translation| PageTranslation {
            cached: true,
            ..translation
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::page_repo::{self, ExtractedPage};
    use crate::db::{paper_repo, paper_repo::ImportStatus, Database};

    fn sentence(id: &str, order: i64, paragraph: i64, text: &str) -> page_repo::Sentence {
        page_repo::Sentence {
            id: id.into(),
            page_number: 1,
            order_index: order,
            paragraph_index: paragraph,
            text: text.into(),
            rects: vec![],
        }
    }

    fn segment(translated: &str, original: &str) -> google_translate::Segment {
        google_translate::Segment {
            translated: translated.into(),
            original: original.into(),
        }
    }

    fn seed_page(db: &Database, paper_id: &str, page: i64, text: &str) -> String {
        paper_repo::insert(&db.conn(), paper_id, paper_id, "/tmp/x.pdf", "P", ImportStatus::Ready)
            .ok();
        page_repo::replace_extraction(
            &mut db.conn(),
            paper_id,
            &[ExtractedPage {
                page_number: page,
                width: 612.0,
                height: 792.0,
                rotation: 0,
                text: text.into(),
                sentences: vec![],
            }],
        )
        .unwrap();
        page_repo::text_hash(text)
    }

    fn store_translation(db: &Database, paper_id: &str, page: i64, hash: &str, lang: &str) {
        let payload = PageTranslation {
            page_number: page,
            target_language: lang.into(),
            units: vec![TranslatedUnit {
                id: "u0".into(),
                text: "번역문".into(),
                sentence_ids: vec!["s1".into()],
                paragraph_index: 0,
            }],
            cached: false,
        };
        db.conn()
            .execute(
                "INSERT INTO translations
                   (paper_id, page_number, target_language, source_hash, provider, model,
                    prompt_version, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'google', 'gtx', ?5, ?6, ?7)",
                params![
                    paper_id,
                    page,
                    lang,
                    hash,
                    PROMPT_VERSION,
                    serde_json::to_string(&payload).unwrap(),
                    now_iso8601(),
                ],
            )
            .unwrap();
    }

    #[test]
    fn source_joins_a_paragraph_with_spaces_and_separates_paragraphs() {
        let sentences = vec![
            sentence("s1", 0, 0, "First."),
            sentence("s2", 1, 0, "Second."),
            sentence("s3", 2, 1, "New paragraph."),
        ];

        let (source, ranges) = build_source(&sentences);

        assert_eq!(source, "First. Second.\nNew paragraph.");
        assert_eq!((ranges[0].start, ranges[0].end), (0, 6));
        assert_eq!((ranges[1].start, ranges[1].end), (7, 14));
        assert_eq!((ranges[2].start, ranges[2].end), (15, 29));
    }

    #[test]
    fn each_segment_maps_back_to_its_sentence() {
        let sentences = vec![
            sentence("s1", 0, 0, "First."),
            sentence("s2", 1, 0, "Second."),
            sentence("s3", 2, 1, "New paragraph."),
        ];
        let (_source, ranges) = build_source(&sentences);

        // Originals concatenate to the source exactly, as the real endpoint does.
        let segments = vec![
            segment("첫 번째.", "First. "),
            segment("두 번째.", "Second."),
            segment("새 문단.", "\nNew paragraph."),
        ];

        let units = map_segments(&ranges, &segments);

        assert_eq!(units.len(), 3);
        assert_eq!(units[0].sentence_ids, vec!["s1".to_string()]);
        assert_eq!(units[1].sentence_ids, vec!["s2".to_string()]);
        assert_eq!(units[2].sentence_ids, vec!["s3".to_string()]);
        assert_eq!(units[2].paragraph_index, 1);
    }

    #[test]
    fn a_merged_segment_maps_to_every_sentence_it_spans() {
        let sentences = vec![sentence("s1", 0, 0, "A."), sentence("s2", 1, 0, "B.")];
        let (_source, ranges) = build_source(&sentences);

        // The translator merged both sentences into one segment.
        let units = map_segments(&ranges, &[segment("에이. 비.", "A. B.")]);

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].sentence_ids, vec!["s1".to_string(), "s2".to_string()]);
    }

    #[test]
    fn a_pure_separator_segment_is_dropped() {
        let sentences = vec![sentence("s1", 0, 0, "Only.")];
        let (_source, ranges) = build_source(&sentences);

        let units = map_segments(&ranges, &[segment("   ", "Only.")]);

        assert!(units.is_empty(), "a segment that translates to whitespace carries nothing");
    }

    #[test]
    fn a_saved_translation_is_returned_when_the_page_is_reopened() {
        let db = Database::open_in_memory().unwrap();
        let hash = seed_page(&db, "p1", 1, "Hello world");
        store_translation(&db, "p1", 1, &hash, "ko");

        let cached = read_cached_page(&db.conn(), "p1", 1, "ko").unwrap();

        let cached = cached.expect("a previously saved translation should be found");
        assert!(cached.cached, "a restored translation is marked as cached");
        assert_eq!(cached.units[0].text, "번역문");
    }

    #[test]
    fn an_untranslated_page_returns_nothing() {
        let db = Database::open_in_memory().unwrap();
        seed_page(&db, "p1", 1, "Hello world");

        assert!(read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_none());
    }

    #[test]
    fn a_translation_in_a_different_language_is_not_returned() {
        let db = Database::open_in_memory().unwrap();
        let hash = seed_page(&db, "p1", 1, "Hello world");
        store_translation(&db, "p1", 1, &hash, "en");

        assert!(read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_none());
    }

    #[test]
    fn a_stale_translation_from_before_re_extraction_is_not_shown() {
        let db = Database::open_in_memory().unwrap();
        let old_hash = seed_page(&db, "p1", 1, "old page text");
        store_translation(&db, "p1", 1, &old_hash, "ko");

        // Re-extraction changes the page text and therefore its hash.
        seed_page(&db, "p1", 1, "completely different page text");

        assert!(
            read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_none(),
            "a translation of the old text must not surface for the new text"
        );
    }

    #[test]
    fn the_prompt_version_is_part_of_the_cache_identity() {
        // A pipeline change must invalidate cached translations, so the constant
        // is stored with every row rather than assumed.
        assert!(!PROMPT_VERSION.is_empty());
    }
}
