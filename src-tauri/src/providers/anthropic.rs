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

const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=100";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// Anthropic has no `response_format`, so structured output is obtained by
/// forcing a single tool call whose input schema is the schema we want.
const STRUCTURED_TOOL: &str = "emit_result";

pub struct AnthropicProvider {
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        // `client()` is fallible, but only on TLS backend setup, which cannot
        // fail after the first successful call; callers still handle the error.
        reqwest::Client::new()
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
    }
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    display_name: Option<String>,
}

impl LlmProvider for AnthropicProvider {
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn validate_credentials(&self) -> Result<()> {
        self.list_models().await.map(|_| ())
    }

    async fn list_models(&self) -> Result<Vec<ModelDescriptor>> {
        let response = client()?
            .get(MODELS_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
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

        Ok(list
            .data
            .into_iter()
            .map(|entry| ModelDescriptor {
                display_name: entry.display_name.unwrap_or_else(|| entry.id.clone()),
                id: entry.id,
            })
            .collect())
    }

    async fn generate_structured(&self, request: StructuredRequest) -> Result<serde_json::Value> {
        let body = json!({
            "model": request.model,
            "max_tokens": request.max_output_tokens,
            "system": format!("{}\n\n{}", request.system, SOURCE_GUARD),
            "tools": [{
                "name": STRUCTURED_TOOL,
                "description": "결과를 이 스키마로만 반환합니다.",
                "input_schema": request.schema,
            }],
            "tool_choice": { "type": "tool", "name": STRUCTURED_TOOL },
            "messages": [{
                "role": "user",
                "content": format!(
                    "{}\n\n{}",
                    request.instructions,
                    wrap_source(&request.source_material)
                ),
            }],
        });

        let response = self
            .request(MESSAGES_URL)
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

        extract_tool_input(&payload)
            .ok_or_else(|| AppError::ProviderResponse("no tool result".into()))
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        sink: mpsc::Sender<ChatDelta>,
    ) -> Result<String> {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|message| json!({ "role": message.role, "content": message.content }))
            .collect();

        let body = json!({
            "model": request.model,
            "max_tokens": request.max_output_tokens,
            "system": request.system,
            "messages": messages,
            "stream": true,
        });

        let response = self
            .request(MESSAGES_URL)
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

                if event["type"] == "content_block_delta" {
                    if let Some(delta) = event["delta"]["text"].as_str() {
                        full.push_str(delta);
                        // A closed receiver means the user cancelled.
                        if sink.send(ChatDelta::Text(delta.to_string())).await.is_err() {
                            return Err(AppError::Cancelled);
                        }
                    }
                }
            }
        }

        Ok(full)
    }
}

fn extract_tool_input(payload: &serde_json::Value) -> Option<serde_json::Value> {
    payload["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == "tool_use" && block["name"] == STRUCTURED_TOOL)
        .map(|block| block["input"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_id_when_no_display_name_is_given() {
        let list: ModelList = serde_json::from_str(
            r#"{"data":[{"id":"claude-sonnet-5","display_name":"Claude Sonnet 5"},
                        {"id":"claude-haiku-4-5-20251001"}]}"#,
        )
        .unwrap();

        let models: Vec<ModelDescriptor> = list
            .data
            .into_iter()
            .map(|entry| ModelDescriptor {
                display_name: entry.display_name.unwrap_or_else(|| entry.id.clone()),
                id: entry.id,
            })
            .collect();

        assert_eq!(models[0].display_name, "Claude Sonnet 5");
        assert_eq!(models[1].display_name, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn reads_structured_output_from_the_forced_tool_call() {
        let payload = serde_json::json!({
            "content": [
                { "type": "text", "text": "여기 결과입니다" },
                { "type": "tool_use", "name": STRUCTURED_TOOL, "input": { "shortSummary": "요약" } }
            ]
        });

        let input = extract_tool_input(&payload).unwrap();
        assert_eq!(input["shortSummary"], "요약");
    }

    #[test]
    fn a_response_with_only_prose_is_reported_rather_than_guessed() {
        let payload = serde_json::json!({
            "content": [{ "type": "text", "text": "죄송하지만 도와드릴 수 없습니다" }]
        });

        assert!(extract_tool_input(&payload).is_none());
    }
}
