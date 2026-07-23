//! Obsidian Local REST API client — the channel Obsidian MCP servers speak.
//!
//! When the user configures the plugin's endpoint, vault writes go through a
//! running Obsidian so notes appear (and re-index) immediately; if the endpoint
//! is unreachable the caller falls back to writing files directly. The API key
//! lives only in the OS credential store (§16.1) and never appears in errors.
//!
//! The plugin serves HTTPS on 27124 with a self-signed certificate (and plain
//! HTTP on 27123 when enabled). Certificate validation is relaxed **only** for
//! loopback hosts — a remote https endpoint is still fully verified.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::db::settings_repo;
use crate::error::{AppError, Result};
use crate::secrets;

#[derive(Debug, Clone)]
pub struct RestConfig {
    pub base_url: String,
    pub api_key: String,
}

/// Loads the configured endpoint + key, or `None` when not set up.
pub fn load(conn: &rusqlite::Connection) -> Result<Option<RestConfig>> {
    let settings = settings_repo::get(conn)?;
    let Some(base_url) = settings.obsidian_rest_url else {
        return Ok(None);
    };
    let Some(api_key) = secrets::get_obsidian_rest_key()? else {
        return Ok(None);
    };
    Ok(Some(RestConfig { base_url, api_key }))
}

/// Loopback endpoints get self-signed certificates from the plugin; anything
/// else keeps full TLS verification.
fn is_loopback(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]"))
        .unwrap_or(false)
}

fn client(config: &RestConfig) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .danger_accept_invalid_certs(is_loopback(&config.base_url))
        .build()
        .map_err(|e| AppError::Internal(format!("http client: {e}")))
}

/// Vault-relative path → URL path, one encoded segment per component, so a
/// Korean note title survives and a crafted title cannot climb the URL.
fn encode_path(rel_path: &str) -> String {
    rel_path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Deserialize)]
struct StatusPayload {
    authenticated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestHealth {
    /// Reachable and the key is accepted.
    Connected,
    /// Reachable but the key is rejected.
    Unauthorized,
    /// Obsidian (or the plugin) is not answering.
    Unreachable,
}

/// GET / — the plugin answers without auth and reports whether the presented
/// key is valid, which distinguishes "Obsidian closed" from "wrong key".
pub async fn health(config: &RestConfig) -> RestHealth {
    let Ok(client) = client(config) else {
        return RestHealth::Unreachable;
    };
    let response = client
        .get(config.base_url.trim_end_matches('/'))
        .bearer_auth(&config.api_key)
        .send()
        .await;

    match response {
        Ok(r) if r.status().is_success() => match r.json::<StatusPayload>().await {
            Ok(payload) if payload.authenticated => RestHealth::Connected,
            Ok(_) => RestHealth::Unauthorized,
            Err(_) => RestHealth::Unreachable,
        },
        Ok(_) | Err(_) => RestHealth::Unreachable,
    }
}

/// PUT /vault/{path} — creates or overwrites one note.
pub async fn put_note(config: &RestConfig, rel_path: &str, content: &str) -> Result<()> {
    let url = format!(
        "{}/vault/{}",
        config.base_url.trim_end_matches('/'),
        encode_path(rel_path)
    );

    let response = client(config)?
        .put(url)
        .bearer_auth(&config.api_key)
        .header(reqwest::header::CONTENT_TYPE, "text/markdown")
        .body(content.to_string())
        .send()
        .await
        .map_err(|_| AppError::VaultUnavailable("obsidian rest unreachable".into()))?;

    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(AppError::ProviderAuth),
        status => Err(AppError::VaultUnavailable(format!("obsidian rest http {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_relax_certificate_checks() {
        assert!(is_loopback("https://127.0.0.1:27124"));
        assert!(is_loopback("https://localhost:27124"));
        assert!(is_loopback("http://127.0.0.1:27123"));
        assert!(!is_loopback("https://my-vault.example.com"));
        assert!(!is_loopback("not a url"));
    }

    #[test]
    fn korean_note_paths_are_percent_encoded_per_segment() {
        let encoded = encode_path("Bbrain/Topics/주의 기제.md");
        assert!(encoded.starts_with("Bbrain/Topics/"));
        assert!(!encoded.contains(' '));
        assert!(!encoded.contains('가'));
        // The separator itself must survive as a path separator.
        assert_eq!(encoded.matches('/').count(), 2);
    }

    #[test]
    fn a_crafted_path_cannot_climb_out_of_the_vault() {
        let encoded = encode_path("../secrets/key.md");
        assert!(!encoded.contains(".."));
        assert_eq!(encoded, "secrets/key%2Emd");
    }
}
