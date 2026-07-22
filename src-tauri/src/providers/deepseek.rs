//! DeepSeek adapter. DeepSeek exposes an OpenAI-*compatible* API, but the
//! compatible surface is the classic **Chat Completions** endpoint
//! (`/chat/completions`), not OpenAI's newer Responses API that `openai.rs`
//! targets. So this adapter cannot share `openai.rs`'s request bodies; it mirrors
//! `anthropic.rs`/`openai.rs` in shape while speaking the Chat Completions wire
//! format (also used by many other OpenAI-compatible providers).
//!
//! Structured output uses DeepSeek's JSON mode (`response_format: json_object`):
//! the schema is stated in the prompt and the model is constrained to emit a JSON
//! object. Bbrain re-validates that object against its own schema before storing,
//! so a drifting response is rejected rather than trusted.

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use super::request::{wrap_source, ChatRequest, StructuredRequest, SOURCE_GUARD};
use super::sse::SseDecoder;
use super::{
    client, map_status, map_transport, retry_after_seconds, ChatDelta, LlmProvider,
    ModelDescriptor, Provider,
};
use crate::error::{AppError, Result};

const MODELS_URL: &str = "https://api.deepseek.com/models";
const CHAT_URL: &str = "https://api.deepseek.com/chat/completions";

pub struct DeepSeekProvider {
    api_key: String,
}

impl DeepSeekProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

impl LlmProvider for DeepSeekProvider {
    fn provider(&self) -> Provider {
        Provider::DeepSeek
    }

    async fn validate_credentials(&self) -> Result<()> {
        self.list_models().await.map(|_| ())
    }

    async fn list_models(&self) -> Result<Vec<ModelDescriptor>> {
        let response = client()?
            .get(MODELS_URL)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;

        if !response.status().is_success() {
            let retry_after = retry_after_seconds(response.headers());
            return Err(map_status(response.status(), retry_after));
        }

        let list: ModelList = response
            .json()
            .await
            .map_err(|_| AppError::ProviderResponse("model list".into()))?;

        Ok(to_descriptors(list.data.into_iter().map(|m| m.id)))
    }

    /// Chat Completions with JSON mode. The schema is spelled out in the prompt
    /// (JSON mode only guarantees *some* valid JSON object, not schema adherence),
    /// and the caller re-validates the result.
    async fn generate_structured(&self, request: StructuredRequest) -> Result<serde_json::Value> {
        let schema_text = serde_json::to_string_pretty(&request.schema)
            .unwrap_or_else(|_| request.schema.to_string());
        // JSON mode requires the word "json" to appear in the prompt.
        let system = format!(
            "{}\n\n{}\n\n결과는 다음 JSON 스키마를 정확히 따르는 JSON 객체 하나로만 반환하세요. \
             설명이나 코드펜스 없이 JSON만 출력하세요.\n스키마:\n{}",
            request.system, SOURCE_GUARD, schema_text
        );

        let body = json!({
            "model": request.model,
            "max_tokens": request.max_output_tokens,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": system },
                {
                    "role": "user",
                    "content": format!(
                        "{}\n\n{}",
                        request.instructions,
                        wrap_source(&request.source_material)
                    ),
                }
            ],
        });

        let response = client()?
            .post(CHAT_URL)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;

        if !response.status().is_success() {
            let retry_after = retry_after_seconds(response.headers());
            return Err(map_status(response.status(), retry_after));
        }

        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|_| AppError::ProviderResponse("structured response".into()))?;

        let text = extract_message_content(&payload)
            .ok_or_else(|| AppError::ProviderResponse("no output text".into()))?;

        serde_json::from_str(strip_code_fence(&text))
            .map_err(|_| AppError::ProviderResponse("output was not valid json".into()))
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        sink: mpsc::Sender<ChatDelta>,
    ) -> Result<String> {
        let mut messages: Vec<serde_json::Value> = Vec::with_capacity(request.messages.len() + 1);
        if !request.system.is_empty() {
            messages.push(json!({ "role": "system", "content": request.system }));
        }
        messages.extend(
            request
                .messages
                .iter()
                .map(|message| json!({ "role": message.role, "content": message.content })),
        );

        let body = json!({
            "model": request.model,
            "max_tokens": request.max_output_tokens,
            "messages": messages,
            "stream": true,
        });

        let response = client()?
            .post(CHAT_URL)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;

        if !response.status().is_success() {
            let retry_after = retry_after_seconds(response.headers());
            return Err(map_status(response.status(), retry_after));
        }

        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut full = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| map_transport(&e))?;

            for payload in decoder.push(&chunk) {
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    continue;
                };

                if let Some(delta) = event["choices"][0]["delta"]["content"].as_str() {
                    if delta.is_empty() {
                        continue;
                    }
                    full.push_str(delta);
                    // A closed receiver means the user cancelled.
                    if sink.send(ChatDelta::Text(delta.to_string())).await.is_err() {
                        return Err(AppError::Cancelled);
                    }
                }
            }
        }

        Ok(full)
    }
}

/// Chat Completions nests the reply under `choices[0].message.content`.
fn extract_message_content(payload: &serde_json::Value) -> Option<String> {
    payload["choices"]
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

/// JSON mode should return a bare object, but a model occasionally wraps it in a
/// ```json fence anyway; peel it so the parse below still succeeds.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop an optional language tag on the opening fence's line.
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.trim_start_matches(['\n', '\r']);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn to_descriptors(ids: impl Iterator<Item = String>) -> Vec<ModelDescriptor> {
    let mut models: Vec<ModelDescriptor> = ids
        .map(|id| ModelDescriptor {
            display_name: id.clone(),
            id,
        })
        .collect();

    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_models_sorted_and_deduplicated() {
        let models = to_descriptors(
            ["deepseek-reasoner", "deepseek-chat", "deepseek-chat"]
                .into_iter()
                .map(String::from),
        );

        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["deepseek-chat", "deepseek-reasoner"]);
    }

    #[test]
    fn reads_content_from_a_chat_completion() {
        let payload = serde_json::json!({
            "choices": [
                { "message": { "role": "assistant", "content": "{\"a\":1}" } }
            ]
        });

        assert_eq!(
            extract_message_content(&payload).as_deref(),
            Some("{\"a\":1}")
        );
    }

    #[test]
    fn a_response_with_no_content_is_reported_rather_than_guessed() {
        let payload = serde_json::json!({ "choices": [] });

        assert!(extract_message_content(&payload).is_none());
    }

    #[test]
    fn a_fenced_json_object_is_unwrapped() {
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("  {\"a\":1}  "), "{\"a\":1}");
    }
}
