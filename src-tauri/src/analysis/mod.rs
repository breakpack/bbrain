pub mod markdown;
pub mod schema;

use rusqlite::params;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{page_repo, paper_repo, settings_repo};
use crate::error::{AppError, Result};
use crate::providers::request::StructuredRequest;
use crate::providers::{AnyProvider, Provider};
use crate::state::AppState;
use crate::time::now_iso8601;
use schema::PaperAnalysisV1;

/// Conservative character budget per request. A paper longer than this is
/// summarized section by section first (DEVELOPMENT.md §10.4).
const DIRECT_CHARACTER_BUDGET: usize = 120_000;
const SECTION_CHARACTER_BUDGET: usize = 40_000;

// The output language is NOT fixed here — `instructions()` appends the rule
// from the "AI 정리 언어" setting, so this must stay language-neutral.
const SYSTEM: &str = "\
당신은 연구 논문을 정리하는 도우미입니다. \
제공된 논문 텍스트에 실제로 있는 내용만 사용하고 추측하지 마세요. \
기여점과 결과에는 근거가 있는 페이지 번호를 함께 제시하세요. \
근거를 찾을 수 없으면 페이지 목록을 비워 두세요.";

/// The active provider plus its model, or a clear reason why AI work cannot run.
pub struct ActiveProvider {
    pub provider: Provider,
    pub model: String,
    pub client: AnyProvider,
}

pub fn resolve_provider(app: &AppHandle) -> Result<ActiveProvider> {
    let state = app.state::<AppState>();

    let settings = {
        let conn = state.db.conn();
        settings_repo::get(&conn)?
    };

    let provider = settings
        .active_provider
        .ok_or_else(|| AppError::NotFound("no active provider".into()))?;

    let model = match provider {
        Provider::OpenAi => settings.openai_model,
        Provider::Anthropic => settings.anthropic_model,
        Provider::DeepSeek => settings.deepseek_model,
    }
    .ok_or_else(|| AppError::ModelUnsupported("no model selected".into()))?;

    let api_key = state
        .credentials
        .get(provider)?
        // A configured provider with no key in the keychain means the user
        // removed it outside the app; treat it as an auth failure so the job
        // waits for a key instead of retrying.
        .ok_or(AppError::ProviderAuth)?;

    Ok(ActiveProvider {
        client: AnyProvider::new(provider, &api_key),
        provider,
        model,
    })
}

/// Runs the analysis job for one paper.
pub async fn analyze_paper(app: &AppHandle, paper_id: &str) -> Result<()> {
    let (paper, pages, content_hash) = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();

        let paper = paper_repo::get(&conn, paper_id)?;
        let pages = page_repo::document_text(&conn, paper_id)?;
        if pages.is_empty() {
            return Err(AppError::InvalidInput("paper has no extracted text".into()));
        }

        let content_hash = page_repo::text_hash(
            &pages
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        (paper, pages, content_hash)
    };

    let active = match resolve_provider(app) {
        Ok(active) => active,
        Err(error) => {
            // No key yet: the paper is readable, the analysis simply waits.
            let state = app.state::<AppState>();
            paper_repo::set_status(
                &state.db.conn(),
                paper_id,
                paper_repo::ImportStatus::WaitingForAi,
            )?;
            return Err(error);
        }
    };

    {
        let state = app.state::<AppState>();
        paper_repo::set_status(
            &state.db.conn(),
            paper_id,
            paper_repo::ImportStatus::Analyzing,
        )?;
    }

    let page_count = pages.len() as i64;
    let source = build_source(&pages);
    let (analysis_language, existing_tags) = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        let language = settings_repo::get(&conn)?.analysis_language;
        let tags = paper_repo::all_tags(&conn)?
            .into_iter()
            .map(|tag| tag.display_name)
            .take(100)
            .collect::<Vec<_>>();
        (language, tags)
    };

    let analysis = generate_with_repair(
        &active,
        &source,
        page_count,
        &analysis_language,
        &existing_tags,
    )
    .await?;
    let markdown = markdown::render(&paper, &analysis, &[]);

    {
        let state = app.state::<AppState>();
        let conn = state.db.conn();

        conn.execute(
            "INSERT INTO analyses
               (paper_id, schema_version, provider, model, content_hash, structured_json,
                markdown, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(paper_id) DO UPDATE SET
               schema_version = excluded.schema_version,
               provider = excluded.provider,
               model = excluded.model,
               content_hash = excluded.content_hash,
               structured_json = excluded.structured_json,
               markdown = excluded.markdown,
               created_at = excluded.created_at",
            params![
                paper_id,
                schema::SCHEMA_VERSION,
                active.provider.as_str(),
                active.model,
                content_hash,
                serde_json::to_string(&analysis).unwrap_or_default(),
                markdown,
                now_iso8601(),
            ],
        )?;

        // AI may suggest tags, but never reassigns the user's groups (§3).
        for tag in &analysis.suggested_tags {
            if let Ok(tag_id) = paper_repo::upsert_tag(&conn, tag, "ai") {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO paper_tags (paper_id, tag_id) VALUES (?1, ?2)",
                    params![paper_id, tag_id],
                );
            }
        }

        save_tag_insights(&conn, paper_id, &analysis.tag_insights)?;

        if let Some(abstract_text) = analysis.short_summary.get(..).map(str::to_string) {
            conn.execute(
                "UPDATE paper_metadata SET abstract = COALESCE(abstract, ?1) WHERE paper_id = ?2",
                params![abstract_text, paper_id],
            )?;
        }

        paper_repo::reindex_fts(&conn, paper_id)?;
        paper_repo::set_status(&conn, paper_id, paper_repo::ImportStatus::Ready)?;

        // Relations depend on the analysis, so they are enqueued once it lands.
        // The version carries the analysis timestamp: a fixed version would
        // collide with the previous run's idempotency key (even a failed one),
        // silently suppressing the refresh after every re-analysis.
        let followup_version = format!("{}-{}", crate::jobs::RELATIONS_VERSION, now_iso8601());
        crate::jobs::queue::enqueue(
            &conn,
            Some(paper_id),
            crate::jobs::JobType::Relations,
            &followup_version,
        )?;
        if settings_repo::get(&conn)?.obsidian_vault_path.is_some() {
            crate::jobs::queue::enqueue(
                &conn,
                Some(paper_id),
                crate::jobs::JobType::ObsidianSync,
                &followup_version,
            )?;
        }
    }

    let _ = app.emit(crate::commands::viewer::PAPER_CHANGED, paper_id);
    let _ = app.emit(crate::commands::library::LIBRARY_CHANGED, ());
    Ok(())
}

/// Writes each tag insight as this paper's row on that concept's note.
/// Keyed by `(tag_id, paper_id)`, so re-analysing the same paper replaces only
/// its own row — every other paper's entry on the same concept note survives.
fn save_tag_insights(
    conn: &rusqlite::Connection,
    paper_id: &str,
    insights: &[schema::TagInsight],
) -> Result<()> {
    for insight in insights {
        if let Ok(tag_id) = paper_repo::upsert_tag(conn, &insight.tag, "ai") {
            conn.execute(
                "INSERT INTO tag_note_entries
                   (tag_id, paper_id, insight_md, evidence_pages, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(tag_id, paper_id) DO UPDATE SET
                   insight_md = excluded.insight_md,
                   evidence_pages = excluded.evidence_pages,
                   updated_at = excluded.updated_at",
                params![
                    tag_id,
                    paper_id,
                    insight.insight,
                    serde_json::to_string(&insight.evidence_pages).unwrap_or_default(),
                    now_iso8601(),
                ],
            )?;
        }
    }
    Ok(())
}

/// A structured response that fails validation gets exactly one repair attempt,
/// then the job fails (DEVELOPMENT.md §10.2).
async fn generate_with_repair(
    active: &ActiveProvider,
    source: &str,
    page_count: i64,
    language: &str,
    existing_tags: &[String],
) -> Result<PaperAnalysisV1> {
    let request = |instructions: String| StructuredRequest {
        model: active.model.clone(),
        system: SYSTEM.to_string(),
        instructions,
        source_material: source.to_string(),
        schema: schema::json_schema(),
        schema_name: "paper_analysis_v1".into(),
        max_output_tokens: 8_000,
    };

    let first = active
        .client
        .generate_structured(request(instructions(page_count, language, existing_tags)))
        .await?;

    match schema::validate(first, page_count) {
        Ok(analysis) => Ok(analysis),
        Err(error) => {
            tracing::warn!(code = ?error.code(), "analysis failed validation; repairing once");

            let repaired = active
                .client
                .generate_structured(request(format!(
                    "{}\n\n이전 응답이 스키마 검증에 실패했습니다. 모든 필수 필드를 채우고 \
                     evidencePages는 1부터 {page_count} 사이의 정수만 사용하세요.",
                    instructions(page_count, language, existing_tags)
                )))
                .await?;

            schema::validate(repaired, page_count)
        }
    }
}

/// `language` chooses the tongue of every narrative field (설정의 "AI 정리
/// 언어"). Keywords/tags stay in the paper's own language either way — they
/// feed the topic graph and search, where the original terms match best.
/// `existing_tags` are the library's current tag display names (최대 100개):
/// listing them steers the model to reuse a concept's existing spelling
/// instead of forking a near-duplicate tag, so the concept note in
/// `tag_note_entries` accumulates on one row per concept.
fn instructions(page_count: i64, language: &str, existing_tags: &[String]) -> String {
    let language_rule = match language {
        "en" => {
            "Write every narrative field (summaries, researchProblem, methodology, \
             results, limitations) in English."
        }
        _ => {
            "모든 서술 필드(요약·연구 문제·방법·결과·한계)는 반드시 한국어로 작성하세요. \
             전문 용어는 필요하면 원어를 괄호로 병기합니다. keywords와 suggestedTags는 \
             논문 원어 그대로 둡니다."
        }
    };
    let existing_tags_note = if existing_tags.is_empty() {
        String::new()
    } else {
        format!(
            "라이브러리에 이미 있는 태그 목록: {}. ",
            existing_tags.join(", ")
        )
    };
    let tag_rule = format!(
        "\n\n{existing_tags_note}이 목록에 같은 개념이 있으면 반드시 같은 표기를 재사용하고, \
         목록에 없는 새로운 개념일 때만 새 태그를 만드세요. 태그는 1~3단어의 짧은 개념어로만 \
         짓고, 문장이나 긴 구절을 태그로 만들지 마세요. 각 suggestedTag마다 tagInsights에 \
         이 논문에서 그 개념이 무엇이고 어떻게 쓰이는지를 1~3문장으로 쓰고 evidencePages에 \
         근거 페이지를 넣으세요."
    );
    format!(
        "다음 {page_count}쪽 논문을 분석해 스키마에 맞는 JSON으로만 답하세요. \
         evidencePages에는 해당 주장을 실제로 확인할 수 있는 페이지 번호만 넣으세요. \
         {language_rule}{tag_rule}"
    )
}

/// Page markers let the model cite pages it can actually point at. Long papers
/// are trimmed section by section rather than truncated mid-sentence.
fn build_source(pages: &[(i64, String)]) -> String {
    let full: String = pages
        .iter()
        .map(|(number, text)| format!("[page {number}]\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n");

    if full.len() <= DIRECT_CHARACTER_BUDGET {
        return full;
    }

    // Keep the front matter (abstract, intro) and the tail (conclusion), which
    // carry the claims an analysis needs, and sample the middle.
    let head_budget = SECTION_CHARACTER_BUDGET;
    let tail_budget = SECTION_CHARACTER_BUDGET;

    let mut head = String::new();
    for (number, text) in pages {
        if head.len() + text.len() > head_budget {
            break;
        }
        head.push_str(&format!("[page {number}]\n{text}\n\n"));
    }

    let mut tail = String::new();
    for (number, text) in pages.iter().rev() {
        if tail.len() + text.len() > tail_budget {
            break;
        }
        tail.insert_str(0, &format!("[page {number}]\n{text}\n\n"));
    }

    format!("{head}\n[중략]\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(count: usize, size: usize) -> Vec<(i64, String)> {
        (1..=count)
            .map(|number| (number as i64, "가".repeat(size)))
            .collect()
    }

    /// 설정의 "AI 정리 언어"가 지시문에 그대로 반영되는지 — 기본은 한국어,
    /// "en"은 영어. SYSTEM 프롬프트는 언어 중립이므로 이 지시가 유일한 결정자다.
    #[test]
    fn the_analysis_language_setting_drives_the_instruction() {
        let korean = instructions(3, "ko", &[]);
        assert!(korean.contains("한국어로 작성"));

        let english = instructions(3, "en", &[]);
        assert!(english.contains("in English"));
        assert!(!english.contains("한국어로 작성"));

        // 알 수 없는 값은 안전하게 기본(한국어)으로.
        assert!(instructions(3, "??", &[]).contains("한국어로 작성"));
    }

    /// 라이브러리에 기존 태그가 있으면 지시문에 나열되어 같은 개념의 재표기를
    /// 막는다. 없을 때는 빈 목록 문구 없이 재사용 규칙만 남는다.
    #[test]
    fn existing_tags_are_listed_so_the_model_reuses_them() {
        let with_tags = instructions(3, "ko", &["RAG".to_string(), "Attention".to_string()]);
        assert!(with_tags.contains("RAG, Attention"));
        assert!(with_tags.contains("같은 표기를 재사용"));
        assert!(with_tags.contains("tagInsights"));

        let without_tags = instructions(3, "ko", &[]);
        assert!(!without_tags.contains("이미 있는 태그 목록"));
        assert!(without_tags.contains("같은 표기를 재사용"));
    }

    #[test]
    fn a_short_paper_is_sent_whole_with_page_markers() {
        let source = build_source(&pages(3, 100));

        assert!(source.contains("[page 1]"));
        assert!(source.contains("[page 3]"));
        assert!(!source.contains("[중략]"));
    }

    #[test]
    fn a_long_paper_keeps_its_opening_and_closing_pages() {
        let source = build_source(&pages(200, 5_000));

        assert!(source.contains("[page 1]"));
        assert!(source.contains("[page 200]"));
        assert!(source.contains("[중략]"), "the middle must be marked as elided");
        assert!(source.len() < DIRECT_CHARACTER_BUDGET * 2);
    }

    #[test]
    fn the_system_prompt_forbids_invented_evidence() {
        assert!(SYSTEM.contains("추측하지"));
        assert!(SYSTEM.contains("근거를 찾을 수 없으면"));
    }

    fn seed_paper(conn: &rusqlite::Connection, id: &str, title: &str) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", title, paper_repo::ImportStatus::Ready)
            .unwrap();
    }

    fn insight(tag: &str, text: &str, pages: Vec<i64>) -> schema::TagInsight {
        schema::TagInsight {
            tag: tag.to_string(),
            insight: text.to_string(),
            evidence_pages: pages,
        }
    }

    /// 재분석 시 같은 논문의 개념 노트 항목은 교체되고, 다른 논문이 같은
    /// 태그에 남긴 항목은 그대로 유지되어야 한다 — 노트는 논문마다 쌓인다.
    #[test]
    fn reanalysis_replaces_its_own_entry_and_keeps_other_papers() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "paper-a", "Paper A");
        seed_paper(&conn, "paper-b", "Paper B");

        save_tag_insights(&conn, "paper-a", &[insight("RAG", "첫 분석", vec![1])]).unwrap();
        save_tag_insights(&conn, "paper-b", &[insight("RAG", "다른 논문의 설명", vec![2])])
            .unwrap();

        // paper-a re-analysed: its RAG entry changes, paper-b's does not.
        save_tag_insights(&conn, "paper-a", &[insight("RAG", "재분석된 설명", vec![5])]).unwrap();

        let mut statement = conn
            .prepare(
                "SELECT paper_id, insight_md, evidence_pages FROM tag_note_entries
                 WHERE tag_id = (SELECT id FROM tags WHERE normalized_name = 'rag')
                 ORDER BY paper_id",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(rows.len(), 2, "one row per paper on the same concept note");
        assert_eq!(rows[0], ("paper-a".into(), "재분석된 설명".into(), "[5]".into()));
        assert_eq!(rows[1], ("paper-b".into(), "다른 논문의 설명".into(), "[2]".into()));
    }
}
