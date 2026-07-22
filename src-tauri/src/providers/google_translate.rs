//! Free machine translation via Google's public `gtx` endpoint. Used for the
//! reader's page and selection translation instead of the LLM provider: a
//! dedicated translator is far faster and costs nothing, and no API key is
//! needed. This is not a configured provider — it needs no credentials and is
//! never used for analysis or chat, only literal translation.
//!
//! A whole page is sent as ONE request (POST, so long pages are not capped by a
//! URL length limit). The endpoint segments the input into sentences and returns
//! one chunk per sentence, carrying both the translation and the original text;
//! the concatenation of the originals reconstructs the input exactly, which lets
//! the caller map each translated segment back to its source location.

use crate::error::{AppError, Result};
use crate::providers;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";

/// One translated sentence segment and the source text it was translated from.
#[derive(Debug, Clone)]
pub struct Segment {
    pub translated: String,
    pub original: String,
}

/// Translates `text` and returns the per-sentence segments. The originals, in
/// order, concatenate back to `text`, so a caller that knows where each source
/// sentence sits in `text` can attribute every segment to a sentence.
pub async fn translate_segments(text: &str, target_language: &str) -> Result<Vec<Segment>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = providers::client()?;
    let response = client
        .post(ENDPOINT)
        .query(&[
            ("client", "gtx"),
            ("sl", "auto"),
            ("tl", target_language),
            ("dt", "t"),
        ])
        // POST form body: long pages exceed a GET query string's length limit.
        .form(&[("q", text)])
        .send()
        .await
        .map_err(|error| providers::map_transport(&error))?;

    if !response.status().is_success() {
        let retry_after = providers::retry_after_seconds(response.headers());
        return Err(providers::map_status(response.status(), retry_after));
    }

    // The body echoes the source text, so it is parsed but never logged (§16.1).
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| AppError::ProviderResponse("translation response was not valid json".into()))?;

    parse_segments(&body)
}

/// Translates `text` into a single string (segments joined). Used for the
/// free-form selection translation, which needs no source mapping.
pub async fn translate_text(text: &str, target_language: &str) -> Result<String> {
    let segments = translate_segments(text, target_language).await?;
    let joined = segments
        .iter()
        .map(|segment| segment.translated.as_str())
        .collect::<String>();

    let joined = joined.trim().to_string();
    if joined.is_empty() {
        return Err(AppError::ProviderResponse("translation was empty".into()));
    }
    Ok(joined)
}

/// Pulls the `[translated, original, ...]` sentence chunks out of the gtx
/// response. Kept pure so the brittle array indexing is covered by tests against
/// captured real responses.
fn parse_segments(body: &serde_json::Value) -> Result<Vec<Segment>> {
    let chunks = body
        .get(0)
        .and_then(|value| value.as_array())
        .ok_or_else(|| AppError::ProviderResponse("translation response had no segments".into()))?;

    let mut segments = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let translated = chunk.get(0).and_then(|value| value.as_str());
        let original = chunk.get(1).and_then(|value| value.as_str());
        if let (Some(translated), Some(original)) = (translated, original) {
            segments.push(Segment {
                translated: translated.to_string(),
                original: original.to_string(),
            });
        }
    }

    if segments.is_empty() {
        return Err(AppError::ProviderResponse("translation was empty".into()));
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A real two-sentence response captured from the endpoint.
    fn sample() -> serde_json::Value {
        json!([
            [
                ["Transformer는 28.4 BLEU를 달성합니다. ", "The Transformer achieves 28.4 BLEU. ", null, null, 3],
                ["그것은 빠릅니다.", "It is fast.", null, null, 3],
            ],
            null,
            "en"
        ])
    }

    #[test]
    fn parses_each_sentence_with_its_original() {
        let segments = parse_segments(&sample()).unwrap();

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].translated, "Transformer는 28.4 BLEU를 달성합니다. ");
        assert_eq!(segments[0].original, "The Transformer achieves 28.4 BLEU. ");
        assert_eq!(segments[1].original, "It is fast.");
    }

    #[test]
    fn the_originals_reconstruct_the_input() {
        let segments = parse_segments(&sample()).unwrap();
        let rebuilt: String = segments.iter().map(|s| s.original.as_str()).collect();

        assert_eq!(rebuilt, "The Transformer achieves 28.4 BLEU. It is fast.");
    }

    #[test]
    fn a_response_without_segments_is_an_error() {
        assert!(parse_segments(&json!([null, null, "en"])).is_err());
        assert!(parse_segments(&json!("nonsense")).is_err());
    }
}
