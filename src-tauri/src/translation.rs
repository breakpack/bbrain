use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::analysis::{resolve_provider, ActiveProvider};
use crate::db::page_repo;
use crate::db::settings_repo::{self, Settings};
use crate::error::{AppError, Result};
use crate::providers::request::StructuredRequest;
use crate::providers::{google_translate, Provider};
use crate::state::AppState;
use crate::time::now_iso8601;

/// Bump when the translation pipeline changes in a way that invalidates saved
/// results: it is part of the cache key, so old translations are not silently
/// reused. Bumped to 3 when a page became a single whole-page request whose
/// segments map back to source sentences (the unit shape changed) (§9.4).
pub const PROMPT_VERSION: &str = "3";

/// Identity of the free Google engine, stored in the cache row's provider/model
/// columns. The LLM engine instead stores the active provider + model, so the
/// two engines never share a cached translation (§9.4).
const TRANSLATOR_PROVIDER: &str = "google";
const TRANSLATOR_MODEL: &str = "gtx";

/// Which engine translates the reader's page/selection. `translation_engine` is
/// a free-text settings column; anything other than `llm` means the free engine.
fn use_llm_engine(settings: &Settings) -> bool {
    settings.translation_engine == "llm"
}

/// The `(provider, model)` written to (and matched against) the translation cache
/// row, implementing the §9.4 cache key. Google is fixed; the LLM engine keys on
/// the active provider and its model so re-selecting either invalidates old
/// translations. `None` when the LLM engine is chosen but nothing is configured
/// yet — the caller treats that as "no cached translation" / "cannot translate".
fn cache_identity(settings: &Settings) -> Option<(String, String)> {
    if use_llm_engine(settings) {
        let provider = settings.active_provider?;
        let model = match provider {
            Provider::OpenAi => settings.openai_model.clone(),
            Provider::Anthropic => settings.anthropic_model.clone(),
            Provider::DeepSeek => settings.deepseek_model.clone(),
        }?;
        Some((provider.as_str().to_string(), model))
    } else {
        Some((TRANSLATOR_PROVIDER.to_string(), TRANSLATOR_MODEL.to_string()))
    }
}

/// Human name for the target language, used in the LLM prompt. Falls back to the
/// raw code so an unlisted language still translates.
fn language_name(code: &str) -> &str {
    match code {
        "ko" => "한국어",
        "en" => "영어",
        "ja" => "일본어",
        "zh" => "중국어",
        other => other,
    }
}

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

    // Cache key: paper + page + page text hash + language + engine identity +
    // pipeline version. A saved result is returned without a network call (§9.4).
    let settings = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        if let Some(cached) = read_cached_page(&conn, paper_id, page_number, target_language)? {
            return Ok(cached);
        }
        settings_repo::get(&conn)?
    };

    let (provider_tag, model_tag) = cache_identity(&settings)
        .ok_or_else(|| AppError::ModelUnsupported("no provider selected for AI translation".into()))?;

    let units = if use_llm_engine(&settings) {
        // The LLM path sends only sentence IDs and originals, and its result must
        // keep every input ID (§9.4); resolve the active provider for the call.
        let active = resolve_provider(app)?;
        translate_page_llm(&active, &sentences, target_language).await?
    } else {
        let (source, ranges) = build_source(&sentences);
        let segments = google_translate::translate_segments(&source, target_language).await?;
        map_segments(&ranges, &segments)
    };

    let result = PageTranslation {
        page_number,
        target_language: target_language.to_string(),
        units,
        cached: false,
    };

    // Only a complete translation is cached (§9.4). The engine identity is stored
    // so switching engine/provider/model does not reuse this row.
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
            provider_tag,
            model_tag,
            PROMPT_VERSION,
            serde_json::to_string(&result).unwrap_or_default(),
            now_iso8601(),
        ],
    )?;

    Ok(result)
}

/// Translates a page with the active AI provider. Only the sentence IDs and their
/// originals are sent; the response must return a translation for every input ID
/// (§9.4). A missing or empty sentence is an error, so a partial page is never
/// cached as complete.
async fn translate_page_llm(
    active: &ActiveProvider,
    sentences: &[page_repo::Sentence],
    target_language: &str,
) -> Result<Vec<TranslatedUnit>> {
    let items: Vec<serde_json::Value> = sentences
        .iter()
        .map(|sentence| json!({ "id": sentence.id, "text": sentence.text }))
        .collect();

    let schema = json!({
        "type": "object",
        "properties": {
            "translations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "text": { "type": "string" }
                    },
                    "required": ["id", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["translations"],
        "additionalProperties": false
    });

    let request = StructuredRequest {
        model: active.model.clone(),
        system: "당신은 학술 논문 번역기입니다. 원문의 의미를 정확히 옮기고 전문 용어를 자연스럽게 번역하세요."
            .into(),
        instructions: format!(
            "<paper> 안에는 각 문장의 id와 원문(text)이 담긴 JSON 배열이 있습니다. \
             각 문장을 {}(으)로 번역하세요. 모든 입력 id를 그대로 유지하고 하나도 빠뜨리거나 \
             추가하지 마세요. translations 배열에 각 항목을 {{id, text}} 형태로 반환하세요.",
            language_name(target_language)
        ),
        source_material: serde_json::to_string(&items).unwrap_or_default(),
        schema,
        schema_name: "page_translation".into(),
        max_output_tokens: 8192,
    };

    let value = active.client.generate_structured(request).await?;

    let mut translations: HashMap<String, String> = HashMap::new();
    for entry in value["translations"].as_array().into_iter().flatten() {
        if let (Some(id), Some(text)) = (entry["id"].as_str(), entry["text"].as_str()) {
            translations.insert(id.to_string(), text.to_string());
        }
    }

    let mut units = Vec::with_capacity(sentences.len());
    for (index, sentence) in sentences.iter().enumerate() {
        let text = translations
            .get(&sentence.id)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            // A dropped sentence breaks click-to-source, so reject the whole page
            // rather than caching a page missing lines (§9.4).
            .ok_or_else(|| AppError::ProviderResponse("translation omitted a sentence".into()))?;

        units.push(TranslatedUnit {
            id: format!("u{index}"),
            text,
            sentence_ids: vec![sentence.id.clone()],
            paragraph_index: sentence.paragraph_index,
        });
    }

    Ok(units)
}

/// Translates a free-form selection with the active AI provider.
async fn translate_selection_llm(
    active: &ActiveProvider,
    text: &str,
    target_language: &str,
) -> Result<String> {
    let schema = json!({
        "type": "object",
        "properties": { "translation": { "type": "string" } },
        "required": ["translation"],
        "additionalProperties": false
    });

    let request = StructuredRequest {
        model: active.model.clone(),
        system: "당신은 번역기입니다. 원문의 의미를 정확히 옮기세요.".into(),
        instructions: format!(
            "<paper> 안의 텍스트를 {}(으)로 번역하여 translation 필드로 반환하세요.",
            language_name(target_language)
        ),
        source_material: text.to_string(),
        schema,
        schema_name: "selection_translation".into(),
        max_output_tokens: 4096,
    };

    let value = active.client.generate_structured(request).await?;
    value["translation"]
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AppError::ProviderResponse("translation was empty".into()))
}

/// Translates a free-form selection the reader dragged over the page with the
/// configured engine. Ad-hoc and not cached — a selection has no stable identity
/// to key on, and the reader asked for this exact span once (§9.3).
pub async fn translate_selection(
    app: &AppHandle,
    text: &str,
    target_language: &str,
) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("selection is empty".into()));
    }

    let use_llm = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        use_llm_engine(&settings_repo::get(&conn)?)
    };

    if use_llm {
        let active = resolve_provider(app)?;
        translate_selection_llm(&active, trimmed, target_language).await
    } else {
        google_translate::translate_text(trimmed, target_language).await
    }
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

    // The cache is engine-specific: a Google translation must not surface when the
    // LLM engine is active, and vice versa (§9.4). An LLM engine with no provider
    // configured has no identity, so there is nothing to restore.
    let Some((provider_tag, model_tag)) = cache_identity(&settings_repo::get(conn)?) else {
        return Ok(None);
    };

    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM translations
             WHERE paper_id = ?1 AND page_number = ?2 AND target_language = ?3
               AND source_hash = ?4 AND provider = ?5 AND model = ?6 AND prompt_version = ?7
             ORDER BY created_at DESC LIMIT 1",
            params![
                paper_id,
                page_number,
                target_language,
                info.text_hash,
                provider_tag,
                model_tag,
                PROMPT_VERSION
            ],
            |row| row.get(0),
        )
        .optional()?;

    let Some(translation) = payload
        .and_then(|json| serde_json::from_str::<PageTranslation>(&json).ok())
    else {
        return Ok(None);
    };

    // Guard against sentence-identity drift: a re-extraction from before
    // deterministic sentence ids rewrote every id while the page text (and so
    // the source_hash) stayed identical — the cached units then reference
    // sentences that no longer exist and hover↔source mapping silently dies.
    // Such a cache is unusable; report a miss so the page is re-translated.
    let live_ids: std::collections::HashSet<String> =
        page_repo::page_sentences(conn, paper_id, page_number)?
            .into_iter()
            .map(|sentence| sentence.id)
            .collect();
    let referenced: Vec<&String> = translation
        .units
        .iter()
        .flat_map(|unit| unit.sentence_ids.iter())
        .collect();
    if !referenced.is_empty() && !referenced.iter().any(|id| live_ids.contains(*id)) {
        return Ok(None);
    }

    Ok(Some(PageTranslation {
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
                sentences: vec![page_repo::ExtractedSentence {
                    order_index: 0,
                    paragraph_index: 0,
                    text: text.into(),
                    rects: vec![page_repo::NormalizedRect {
                        x: 0.1,
                        y: 0.1,
                        width: 0.5,
                        height: 0.02,
                    }],
                }],
            }],
        )
        .unwrap();
        page_repo::text_hash(text)
    }

    /// Stores a cached translation whose units reference the page's REAL
    /// sentence ids — mirroring what a live translation run would persist.
    fn store_translation(db: &Database, paper_id: &str, page: i64, hash: &str, lang: &str) {
        let sentence_ids: Vec<String> = page_repo::page_sentences(&db.conn(), paper_id, page)
            .unwrap()
            .into_iter()
            .map(|sentence| sentence.id)
            .collect();
        let payload = PageTranslation {
            page_number: page,
            target_language: lang.into(),
            units: vec![TranslatedUnit {
                id: "u0".into(),
                text: "번역문".into(),
                sentence_ids,
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

    /// The bug this guards: sentence ids were regenerated by a re-extraction
    /// while the page text (and so the cache's source_hash) stayed identical.
    /// The cache then still served units pointing at sentences that no longer
    /// exist — the translation rendered, but hover↔source mapping was dead.
    #[test]
    fn a_cache_referencing_vanished_sentences_reports_a_miss() {
        let db = Database::open_in_memory().unwrap();
        let hash = seed_page(&db, "p1", 1, "page text");
        store_translation(&db, "p1", 1, &hash, "ko");

        // Simulate the pre-deterministic-id world: same text, new ids.
        db.conn()
            .execute("UPDATE sentences SET id = 'orphaned-' || id", [])
            .unwrap();

        assert!(
            read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_none(),
            "a cache whose sentence references all vanished must be a miss"
        );
    }

    #[test]
    fn the_prompt_version_is_part_of_the_cache_identity() {
        // A pipeline change must invalidate cached translations, so the constant
        // is stored with every row rather than assumed.
        assert!(!PROMPT_VERSION.is_empty());
    }

    fn base_settings() -> Settings {
        Settings {
            language: "ko".into(),
            active_provider: None,
            openai_model: None,
            anthropic_model: None,
            deepseek_model: None,
            has_openai_key: false,
            has_anthropic_key: false,
            has_deepseek_key: false,
            translation_language: "ko".into(),
            analysis_language: "ko".into(),
            translation_engine: "google".into(),
            obsidian_vault_path: None,
            obsidian_rest_url: None,
            has_obsidian_rest_key: false,
            has_semantic_scholar_key: false,
            embedding_model_id: "intfloat/multilingual-e5-small".into(),
            embedding_dimension: 384,
            index_generation: 1,
            network_notice_accepted_at: None,
            onboarding_completed_at: None,
        }
    }

    #[test]
    fn the_free_engine_keys_the_cache_on_the_fixed_google_identity() {
        let settings = base_settings();
        assert_eq!(
            cache_identity(&settings),
            Some(("google".to_string(), "gtx".to_string()))
        );
    }

    #[test]
    fn the_llm_engine_keys_the_cache_on_the_active_provider_and_model() {
        let settings = Settings {
            translation_engine: "llm".into(),
            active_provider: Some(Provider::DeepSeek),
            deepseek_model: Some("deepseek-chat".into()),
            ..base_settings()
        };
        assert_eq!(
            cache_identity(&settings),
            Some(("deepseek".to_string(), "deepseek-chat".to_string()))
        );
    }

    #[test]
    fn the_llm_engine_without_a_provider_has_no_cache_identity() {
        let settings = Settings {
            translation_engine: "llm".into(),
            ..base_settings()
        };
        assert!(cache_identity(&settings).is_none());
    }

    #[test]
    fn switching_the_engine_does_not_reuse_the_other_engines_translation() {
        let db = Database::open_in_memory().unwrap();
        let hash = seed_page(&db, "p1", 1, "Hello world");
        // A page translated by the free engine (provider 'google', model 'gtx').
        store_translation(&db, "p1", 1, &hash, "ko");

        // Reading under the default (google) engine restores it...
        assert!(read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_some());

        // ...but after switching to the LLM engine the Google row is not served.
        settings_repo::update(
            &db.conn(),
            &settings_repo::SettingsPatch {
                translation_engine: Some("llm".into()),
                active_provider: Some(Provider::Anthropic),
                anthropic_model: Some("claude-sonnet-5".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            read_cached_page(&db.conn(), "p1", 1, "ko").unwrap().is_none(),
            "the LLM engine must not reuse a Google translation"
        );
    }
}
