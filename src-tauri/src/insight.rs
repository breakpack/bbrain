//! In-reader AI helpers (Liner-style): explain a selected passage in plain
//! language, and synthesize the reader's highlights into a summary. Both run
//! through the user's active provider (§5.1) — the same one analysis and chat
//! use — so switching providers (incl. DeepSeek) switches these too.

use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::analysis::resolve_provider;
use crate::db::highlight_repo;
use crate::error::{AppError, Result};
use crate::providers::request::StructuredRequest;
use crate::state::AppState;

/// Explains a passage the reader selected, in plain Korean.
pub async fn explain_selection(app: &AppHandle, text: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("설명할 텍스트가 없습니다.".into()));
    }

    let active = resolve_provider(app)?;
    let request = StructuredRequest {
        model: active.model.clone(),
        system: "당신은 학술 논문을 쉽게 풀어 설명하는 도우미입니다. 배경지식이 적은 \
                 독자도 이해할 수 있도록 정확하고 간결하게 한국어로 설명하세요. 원문에 \
                 없는 내용을 지어내지 마세요."
            .into(),
        instructions: "<paper> 안의 문장이나 구절을 한국어로 풀어 설명하여 explanation \
                       필드로 반환하세요. 어려운 용어는 쉽게 풀되 의미를 왜곡하지 마세요."
            .into(),
        source_material: trimmed.to_string(),
        schema: json!({
            "type": "object",
            "properties": { "explanation": { "type": "string" } },
            "required": ["explanation"],
            "additionalProperties": false
        }),
        schema_name: "selection_explanation".into(),
        max_output_tokens: 2048,
    };

    let value = active.client.generate_structured(request).await?;
    value["explanation"]
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AppError::ProviderResponse("explanation was empty".into()))
}

/// Synthesizes everything the reader highlighted in one paper into a summary.
pub async fn summarize_highlights(app: &AppHandle, paper_id: &str) -> Result<String> {
    let highlights = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        highlight_repo::list(&conn, paper_id)?
    };

    let passages: Vec<String> = highlights
        .iter()
        .map(|highlight| highlight.selected_text.trim())
        .filter(|text| text.len() > 2)
        .enumerate()
        .map(|(index, text)| format!("{}. {text}", index + 1))
        .collect();

    if passages.is_empty() {
        return Err(AppError::InvalidInput("종합할 하이라이트가 없습니다.".into()));
    }

    let active = resolve_provider(app)?;
    let request = StructuredRequest {
        model: active.model.clone(),
        system: "당신은 독자가 한 논문에서 표시한 하이라이트들을 종합해 핵심을 정리하는 \
                 도우미입니다. 하이라이트에 담긴 내용만으로 한국어로 요약하고, 없는 내용을 \
                 지어내지 마세요."
            .into(),
        instructions: "<paper> 안은 한 논문에서 독자가 하이라이트한 구절들입니다. 이들을 \
                       종합해 핵심 논지와 주요 포인트를 한국어로 정리하여 summary 필드로 \
                       반환하세요."
            .into(),
        source_material: passages.join("\n"),
        schema: json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "required": ["summary"],
            "additionalProperties": false
        }),
        schema_name: "highlight_summary".into(),
        max_output_tokens: 2048,
    };

    let value = active.client.generate_structured(request).await?;
    value["summary"]
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AppError::ProviderResponse("summary was empty".into()))
}
