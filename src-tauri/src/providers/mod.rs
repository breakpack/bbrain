pub mod anthropic;
pub mod deepseek;
pub mod google_translate;
pub mod openai;
pub mod request;
pub mod sse;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::error::{AppError, Result};
use request::{ChatRequest, StructuredRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
// "lowercase", NOT "snake_case": the frontend sends "openai"/"deepseek", and
// snake_case would expect "open_ai"/"deep_seek" — the IPC deserialization then
// fails before the command runs (only "anthropic" is identical under both).
// Must match `as_str`/`from_str` below and `Provider` in src/lib/types.ts.
#[serde(rename_all = "lowercase")]
pub enum Provider {
    OpenAi,
    Anthropic,
    DeepSeek,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::DeepSeek => "DeepSeek",
        }
    }

    /// Balanced starting preset as of 2026-07-14 (DEVELOPMENT.md §5.1). This is
    /// an initial value, not a permanent constant — the user can pick any text
    /// model the provider lists.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-5.6-terra",
            Self::Anthropic => "claude-sonnet-5",
            // `deepseek-reasoner` is stronger but slower and its structured-output
            // support is narrower, so the balanced default is the chat model.
            Self::DeepSeek => "deepseek-chat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: Provider,
    pub valid: bool,
    /// Present when the stored model is no longer offered by the provider.
    /// Bbrain asks the user to re-select rather than silently substituting.
    pub model_available: Option<bool>,
}

/// One streamed chunk of an assistant reply.
#[derive(Debug, Clone)]
pub enum ChatDelta {
    Text(String),
}

/// Common surface over OpenAI Responses and Anthropic Messages. Request shapes,
/// structured output, streaming, errors and rate-limit info are normalized
/// inside the adapters (DEVELOPMENT.md §5.1).
#[allow(async_fn_in_trait)]
pub trait LlmProvider {
    fn provider(&self) -> Provider;
    async fn validate_credentials(&self) -> Result<()>;
    async fn list_models(&self) -> Result<Vec<ModelDescriptor>>;

    /// Returns JSON already validated against the provider's schema mechanism.
    /// Bbrain re-validates it against its own schema before storing.
    async fn generate_structured(&self, request: StructuredRequest) -> Result<serde_json::Value>;

    /// Streams the reply through `sink` and returns the full text. Dropping the
    /// receiver cancels the request.
    async fn stream_chat(
        &self,
        request: ChatRequest,
        sink: mpsc::Sender<ChatDelta>,
    ) -> Result<String>;
}

/// Builds the adapter for whichever provider is active.
pub enum AnyProvider {
    OpenAi(openai::OpenAiProvider),
    Anthropic(anthropic::AnthropicProvider),
    DeepSeek(deepseek::DeepSeekProvider),
}

impl AnyProvider {
    pub fn new(provider: Provider, api_key: &str) -> Self {
        match provider {
            Provider::OpenAi => Self::OpenAi(openai::OpenAiProvider::new(api_key)),
            Provider::Anthropic => Self::Anthropic(anthropic::AnthropicProvider::new(api_key)),
            Provider::DeepSeek => Self::DeepSeek(deepseek::DeepSeekProvider::new(api_key)),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelDescriptor>> {
        match self {
            Self::OpenAi(p) => p.list_models().await,
            Self::Anthropic(p) => p.list_models().await,
            Self::DeepSeek(p) => p.list_models().await,
        }
    }

    pub async fn generate_structured(
        &self,
        request: StructuredRequest,
    ) -> Result<serde_json::Value> {
        match self {
            Self::OpenAi(p) => p.generate_structured(request).await,
            Self::Anthropic(p) => p.generate_structured(request).await,
            Self::DeepSeek(p) => p.generate_structured(request).await,
        }
    }

    pub async fn stream_chat(
        &self,
        request: ChatRequest,
        sink: mpsc::Sender<ChatDelta>,
    ) -> Result<String> {
        match self {
            Self::OpenAi(p) => p.stream_chat(request, sink).await,
            Self::Anthropic(p) => p.stream_chat(request, sink).await,
            Self::DeepSeek(p) => p.stream_chat(request, sink).await,
        }
    }
}

/// Ceiling for generation calls (구조화 분석·채팅 스트림). A non-streaming
/// analysis legitimately generates for minutes — the client-wide 30s total
/// timeout was cutting every long analysis off mid-generation, and the cut
/// surfaced as a spurious `Network` failure after three backoff retries.
/// Callers opt in per request with `.timeout(GENERATION_TIMEOUT)`, which
/// overrides the client default below.
pub const GENERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // Total-request default for quick control-plane calls (model lists,
        // key validation). Generation requests override it per request.
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Internal(format!("http client: {e}")))
}

/// Maps transport and status codes to Bbrain's retry classes (§10.2). Response
/// bodies are never captured here — they may echo the paper text or the key.
pub fn map_status(status: reqwest::StatusCode, retry_after: Option<u64>) -> AppError {
    match status.as_u16() {
        401 | 403 => AppError::ProviderAuth,
        429 => AppError::ProviderRateLimit {
            retry_after_seconds: retry_after,
        },
        500..=599 => AppError::ProviderUnavailable,
        _ => AppError::ProviderResponse(format!("unexpected status {}", status.as_u16())),
    }
}

pub fn map_transport(error: &reqwest::Error) -> AppError {
    if error.is_timeout() || error.is_connect() {
        AppError::Network
    } else {
        AppError::ProviderUnavailable
    }
}

pub fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn provider_round_trips_through_its_stored_string() {
        for provider in [Provider::OpenAi, Provider::Anthropic, Provider::DeepSeek] {
            assert_eq!(Provider::from_str(provider.as_str()), Some(provider));
        }
        assert_eq!(Provider::from_str("gemini"), None);
    }

    /// The IPC wire format must match the frontend's Provider union
    /// ("openai" | "anthropic" | "deepseek"). snake_case would silently break
    /// deserialization for every multi-word name ("open_ai", "deep_seek") —
    /// the command then fails before it runs, surfacing only as "알 수 없는
    /// 오류" in the UI.
    #[test]
    fn provider_serde_matches_the_frontend_union() {
        for provider in [Provider::OpenAi, Provider::Anthropic, Provider::DeepSeek] {
            let wire = serde_json::to_string(&provider).unwrap();
            assert_eq!(wire, format!("\"{}\"", provider.as_str()));
            let back: Provider = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, provider);
        }
    }

    #[test]
    fn auth_failures_are_not_retried() {
        assert!(matches!(
            map_status(StatusCode::UNAUTHORIZED, None),
            AppError::ProviderAuth
        ));
        assert!(matches!(
            map_status(StatusCode::FORBIDDEN, None),
            AppError::ProviderAuth
        ));
    }

    #[test]
    fn rate_limits_carry_retry_after() {
        let error = map_status(StatusCode::TOO_MANY_REQUESTS, Some(30));
        assert_eq!(error.retry_after_seconds(), Some(30));
    }

    #[test]
    fn server_errors_map_to_unavailable() {
        assert!(matches!(
            map_status(StatusCode::BAD_GATEWAY, None),
            AppError::ProviderUnavailable
        ));
    }

    #[test]
    fn error_messages_never_leak_a_body() {
        let error = map_status(StatusCode::UNAUTHORIZED, None);
        let message = error.redacted_message();
        assert!(!message.contains("sk-"));
        assert!(message.contains("API 키"));
    }
}
