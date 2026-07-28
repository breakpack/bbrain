use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::providers::Provider;
use crate::time::now_iso8601;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub language: String,
    pub active_provider: Option<Provider>,
    pub openai_model: Option<String>,
    pub anthropic_model: Option<String>,
    pub deepseek_model: Option<String>,
    /// Whether a key for each provider exists in the OS credential store.
    /// The key itself never crosses the IPC boundary.
    pub has_openai_key: bool,
    pub has_anthropic_key: bool,
    pub has_deepseek_key: bool,
    pub translation_language: String,
    /// AI 정리(논문 분석) 출력 언어: `ko`(기본) 또는 `en`.
    pub analysis_language: String,
    /// Which engine translates the reader's page/selection: `google` (free) or
    /// `llm` (the active provider). Free by default.
    pub translation_engine: String,
    pub obsidian_vault_path: Option<String>,
    /// Obsidian Local REST API endpoint (the channel MCP servers use). When
    /// set and reachable, vault writes go through it; files are the fallback.
    pub obsidian_rest_url: Option<String>,
    pub has_obsidian_rest_key: bool,
    pub has_semantic_scholar_key: bool,
    pub embedding_model_id: String,
    pub embedding_dimension: i64,
    pub index_generation: i64,
    pub network_notice_accepted_at: Option<String>,
    pub onboarding_completed_at: Option<String>,
}

/// Only fields present are written; `None` leaves the stored value untouched.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub language: Option<String>,
    pub active_provider: Option<Provider>,
    pub openai_model: Option<String>,
    pub anthropic_model: Option<String>,
    pub deepseek_model: Option<String>,
    pub translation_language: Option<String>,
    pub analysis_language: Option<String>,
    pub translation_engine: Option<String>,
    pub obsidian_vault_path: Option<String>,
    pub obsidian_rest_url: Option<String>,
    pub network_notice_accepted: Option<bool>,
    pub onboarding_completed: Option<bool>,
}

/// Creates the single settings row on first run.
pub fn ensure_row(conn: &Connection) -> Result<()> {
    let now = now_iso8601();
    conn.execute(
        "INSERT OR IGNORE INTO settings (id, created_at, updated_at) VALUES (1, ?1, ?1)",
        params![now],
    )?;
    Ok(())
}

pub fn get(conn: &Connection) -> Result<Settings> {
    ensure_row(conn)?;
    let settings = conn.query_row(
        "SELECT language, active_provider, openai_model, anthropic_model,
                openai_credential_ref, anthropic_credential_ref,
                translation_language, obsidian_vault_path,
                embedding_model_id, embedding_dimension, index_generation,
                network_notice_accepted_at, onboarding_completed_at,
                deepseek_model, deepseek_credential_ref, translation_engine,
                obsidian_rest_url, obsidian_rest_credential_ref, analysis_language,
                semantic_scholar_credential_ref
         FROM settings WHERE id = 1",
        [],
        |row| {
            Ok(Settings {
                language: row.get(0)?,
                active_provider: row
                    .get::<_, Option<String>>(1)?
                    .and_then(|s| Provider::from_str(&s)),
                openai_model: row.get(2)?,
                anthropic_model: row.get(3)?,
                has_openai_key: row.get::<_, Option<String>>(4)?.is_some(),
                has_anthropic_key: row.get::<_, Option<String>>(5)?.is_some(),
                translation_language: row.get(6)?,
                obsidian_vault_path: row.get(7)?,
                embedding_model_id: row.get(8)?,
                embedding_dimension: row.get(9)?,
                index_generation: row.get(10)?,
                network_notice_accepted_at: row.get(11)?,
                onboarding_completed_at: row.get(12)?,
                deepseek_model: row.get(13)?,
                has_deepseek_key: row.get::<_, Option<String>>(14)?.is_some(),
                translation_engine: row.get(15)?,
                obsidian_rest_url: row.get(16)?,
                has_obsidian_rest_key: row.get::<_, Option<String>>(17)?.is_some(),
                analysis_language: row.get(18)?,
                has_semantic_scholar_key: row.get::<_, Option<String>>(19)?.is_some(),
            })
        },
    )?;
    Ok(settings)
}

pub fn update(conn: &Connection, patch: &SettingsPatch) -> Result<Settings> {
    ensure_row(conn)?;
    let now = now_iso8601();

    if let Some(language) = &patch.language {
        conn.execute(
            "UPDATE settings SET language = ?1, updated_at = ?2 WHERE id = 1",
            params![language, now],
        )?;
    }
    if let Some(provider) = patch.active_provider {
        conn.execute(
            "UPDATE settings SET active_provider = ?1, updated_at = ?2 WHERE id = 1",
            params![provider.as_str(), now],
        )?;
    }
    if let Some(model) = &patch.openai_model {
        conn.execute(
            "UPDATE settings SET openai_model = ?1, updated_at = ?2 WHERE id = 1",
            params![model, now],
        )?;
    }
    if let Some(model) = &patch.anthropic_model {
        conn.execute(
            "UPDATE settings SET anthropic_model = ?1, updated_at = ?2 WHERE id = 1",
            params![model, now],
        )?;
    }
    if let Some(model) = &patch.deepseek_model {
        conn.execute(
            "UPDATE settings SET deepseek_model = ?1, updated_at = ?2 WHERE id = 1",
            params![model, now],
        )?;
    }
    if let Some(language) = &patch.translation_language {
        conn.execute(
            "UPDATE settings SET translation_language = ?1, updated_at = ?2 WHERE id = 1",
            params![language, now],
        )?;
    }
    if let Some(language) = &patch.analysis_language {
        conn.execute(
            "UPDATE settings SET analysis_language = ?1, updated_at = ?2 WHERE id = 1",
            params![language, now],
        )?;
    }
    if let Some(engine) = &patch.translation_engine {
        conn.execute(
            "UPDATE settings SET translation_engine = ?1, updated_at = ?2 WHERE id = 1",
            params![engine, now],
        )?;
    }
    if let Some(path) = &patch.obsidian_vault_path {
        conn.execute(
            "UPDATE settings SET obsidian_vault_path = ?1, updated_at = ?2 WHERE id = 1",
            params![path, now],
        )?;
    }
    if let Some(url) = &patch.obsidian_rest_url {
        // An empty string disconnects: the URL and the credential marker clear
        // together (the key itself is removed from the keychain by the caller).
        let value = (!url.trim().is_empty()).then(|| url.trim().to_string());
        conn.execute(
            "UPDATE settings SET obsidian_rest_url = ?1,
                    obsidian_rest_credential_ref = CASE WHEN ?1 IS NULL THEN NULL
                                                        ELSE obsidian_rest_credential_ref END,
                    updated_at = ?2
             WHERE id = 1",
            params![value, now],
        )?;
    }
    if let Some(accepted) = patch.network_notice_accepted {
        let value = accepted.then(|| now.clone());
        conn.execute(
            "UPDATE settings SET network_notice_accepted_at = ?1, updated_at = ?2 WHERE id = 1",
            params![value, now],
        )?;
    }
    if let Some(completed) = patch.onboarding_completed {
        let value = completed.then(|| now.clone());
        conn.execute(
            "UPDATE settings SET onboarding_completed_at = ?1, updated_at = ?2 WHERE id = 1",
            params![value, now],
        )?;
    }

    get(conn)
}

/// Records that a credential for `provider` exists, storing only the lookup
/// reference — never the key material (DEVELOPMENT.md §16.1).
pub fn set_credential_ref(
    conn: &Connection,
    provider: Provider,
    credential_ref: Option<&str>,
) -> Result<()> {
    ensure_row(conn)?;
    let now = now_iso8601();
    let sql = match provider {
        Provider::OpenAi => {
            "UPDATE settings SET openai_credential_ref = ?1, updated_at = ?2 WHERE id = 1"
        }
        Provider::Anthropic => {
            "UPDATE settings SET anthropic_credential_ref = ?1, updated_at = ?2 WHERE id = 1"
        }
        Provider::DeepSeek => {
            "UPDATE settings SET deepseek_credential_ref = ?1, updated_at = ?2 WHERE id = 1"
        }
    };
    conn.execute(sql, params![credential_ref, now])?;
    Ok(())
}

/// Marks (or clears) the Obsidian REST key's presence. Only a reference — the
/// key itself lives in the OS credential store (§16.1).
pub fn set_obsidian_rest_credential_ref(
    conn: &Connection,
    credential_ref: Option<&str>,
) -> Result<()> {
    ensure_row(conn)?;
    conn.execute(
        "UPDATE settings SET obsidian_rest_credential_ref = ?1, updated_at = ?2 WHERE id = 1",
        params![credential_ref, now_iso8601()],
    )?;
    Ok(())
}

/// Marks (or clears) the Semantic Scholar key's presence. The key itself never
/// enters SQLite and is read only inside the Rust core.
pub fn set_semantic_scholar_credential_ref(
    conn: &Connection,
    credential_ref: Option<&str>,
) -> Result<()> {
    ensure_row(conn)?;
    conn.execute(
        "UPDATE settings SET semantic_scholar_credential_ref = ?1, updated_at = ?2 WHERE id = 1",
        params![credential_ref, now_iso8601()],
    )?;
    Ok(())
}

pub fn credential_ref(conn: &Connection, provider: Provider) -> Result<Option<String>> {
    ensure_row(conn)?;
    let column = match provider {
        Provider::OpenAi => "openai_credential_ref",
        Provider::Anthropic => "anthropic_credential_ref",
        Provider::DeepSeek => "deepseek_credential_ref",
    };
    let value = conn
        .query_row(
            &format!("SELECT {column} FROM settings WHERE id = 1"),
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn returns_defaults_on_first_read() {
        let db = Database::open_in_memory().unwrap();
        let settings = get(&db.conn()).unwrap();

        assert_eq!(settings.language, "ko");
        assert_eq!(settings.embedding_model_id, "intfloat/multilingual-e5-small");
        assert_eq!(settings.embedding_dimension, 384);
        assert_eq!(settings.index_generation, 1);
        assert!(settings.active_provider.is_none());
        assert!(!settings.has_openai_key);
        assert!(!settings.has_deepseek_key);
        assert!(!settings.has_semantic_scholar_key);
        assert_eq!(settings.translation_engine, "google");
        assert!(settings.onboarding_completed_at.is_none());
    }

    #[test]
    fn patch_only_touches_provided_fields() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        update(
            &conn,
            &SettingsPatch {
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let settings = update(
            &conn,
            &SettingsPatch {
                anthropic_model: Some("claude-sonnet-5".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(settings.language, "en");
        assert_eq!(settings.anthropic_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(settings.translation_language, "ko");
    }

    #[test]
    fn credential_ref_round_trips_without_storing_key_material() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        set_credential_ref(&conn, Provider::Anthropic, Some("bbrain:anthropic")).unwrap();

        assert_eq!(
            credential_ref(&conn, Provider::Anthropic).unwrap().as_deref(),
            Some("bbrain:anthropic")
        );
        assert!(credential_ref(&conn, Provider::OpenAi).unwrap().is_none());
        assert!(get(&conn).unwrap().has_anthropic_key);
    }

    #[test]
    fn clearing_a_credential_ref_marks_the_key_absent() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        set_credential_ref(&conn, Provider::OpenAi, Some("bbrain:openai")).unwrap();
        set_credential_ref(&conn, Provider::OpenAi, None).unwrap();

        assert!(!get(&conn).unwrap().has_openai_key);
    }

    #[test]
    fn semantic_scholar_marker_never_contains_key_material() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        set_semantic_scholar_credential_ref(
            &conn,
            Some("com.bbrain.desktop/semantic-scholar"),
        )
        .unwrap();

        assert!(get(&conn).unwrap().has_semantic_scholar_key);
        let stored: Option<String> = conn
            .query_row(
                "SELECT semantic_scholar_credential_ref FROM settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some("com.bbrain.desktop/semantic-scholar")
        );
    }

    #[test]
    fn accepting_the_network_notice_stamps_a_timestamp() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        let settings = update(
            &conn,
            &SettingsPatch {
                network_notice_accepted: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(settings.network_notice_accepted_at.is_some());
    }
}
