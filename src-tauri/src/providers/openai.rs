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

const MODELS_URL: &str = "https://api.openai.com/v1/models";
const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";

pub struct OpenAiProvider {
    api_key: String,
}

impl OpenAiProvider {
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

impl LlmProvider for OpenAiProvider {
    fn provider(&self) -> Provider {
        Provider::OpenAi
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

        Ok(select_text_models(list.data.into_iter().map(|m| m.id)))
    }

    /// Uses the Responses API with a strict JSON schema, so the model cannot
    /// return prose where Bbrain expects structured data.
    async fn generate_structured(&self, request: StructuredRequest) -> Result<serde_json::Value> {
        let body = json!({
            "model": request.model,
            "instructions": format!("{}\n\n{}", request.system, SOURCE_GUARD),
            "input": format!("{}\n\n{}", request.instructions, wrap_source(&request.source_material)),
            "max_output_tokens": request.max_output_tokens,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema,
                }
            }
        });

        let response = client()?
            .post(RESPONSES_URL)
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

        let text = extract_output_text(&payload)
            .ok_or_else(|| AppError::ProviderResponse("no output text".into()))?;

        serde_json::from_str(&text)
            .map_err(|_| AppError::ProviderResponse("output was not valid json".into()))
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        sink: mpsc::Sender<ChatDelta>,
    ) -> Result<String> {
        let input: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|message| json!({ "role": message.role, "content": message.content }))
            .collect();

        let body = json!({
            "model": request.model,
            "instructions": request.system,
            "input": input,
            "max_output_tokens": request.max_output_tokens,
            "stream": true,
        });

        let response = client()?
            .post(RESPONSES_URL)
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

                if event["type"] == "response.output_text.delta" {
                    if let Some(delta) = event["delta"].as_str() {
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

/// The Responses API nests text under `output[].content[]`; `output_text` is a
/// convenience field some SDKs add, so accept either.
fn extract_output_text(payload: &serde_json::Value) -> Option<String> {
    if let Some(text) = payload["output_text"].as_str() {
        return Some(text.to_string());
    }

    let mut collected = String::new();
    for item in payload["output"].as_array()? {
        for part in item["content"].as_array().into_iter().flatten() {
            if let Some(text) = part["text"].as_str() {
                collected.push_str(text);
            }
        }
    }

    (!collected.is_empty()).then_some(collected)
}

/// Keeps text-generation models and drops embedding, audio, image and moderation
/// endpoints, which cannot answer an analysis or chat request.
fn select_text_models(ids: impl Iterator<Item = String>) -> Vec<ModelDescriptor> {
    const EXCLUDED: &[&str] = &[
        "embedding",
        "whisper",
        "tts",
        "dall-e",
        "moderation",
        "audio",
        "image",
        "realtime",
        "transcribe",
        "codex",
    ];

    let mut models: Vec<ModelDescriptor> = ids
        .filter(|id| {
            let lowered = id.to_lowercase();
            !EXCLUDED.iter().any(|marker| lowered.contains(marker))
        })
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
    fn drops_non_text_models() {
        let models = select_text_models(
            [
                "gpt-5.6-terra",
                "text-embedding-3-small",
                "whisper-1",
                "dall-e-3",
                "omni-moderation-latest",
                "gpt-4o-realtime-preview",
            ]
            .into_iter()
            .map(String::from),
        );

        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-5.6-terra"]);
    }

    #[test]
    fn sorts_and_deduplicates() {
        let models =
            select_text_models(["gpt-b", "gpt-a", "gpt-a"].into_iter().map(String::from));

        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-a", "gpt-b"]);
    }

    #[test]
    fn reads_output_text_from_the_nested_response_shape() {
        let payload = serde_json::json!({
            "output": [
                { "content": [ { "type": "output_text", "text": "{\"a\":1}" } ] }
            ]
        });

        assert_eq!(extract_output_text(&payload).as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn reads_output_text_from_the_flat_convenience_field() {
        let payload = serde_json::json!({ "output_text": "hello" });

        assert_eq!(extract_output_text(&payload).as_deref(), Some("hello"));
    }

    #[test]
    fn a_response_with_no_text_is_reported_rather_than_guessed() {
        let payload = serde_json::json!({ "output": [] });

        assert!(extract_output_text(&payload).is_none());
    }
}
