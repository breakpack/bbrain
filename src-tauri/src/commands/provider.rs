use serde::Deserialize;
use tauri::State;

use crate::db::settings_repo;
use crate::error::{AppError, CommandResult, Result};
use crate::providers::{
    anthropic::AnthropicProvider, deepseek::DeepSeekProvider, openai::OpenAiProvider, LlmProvider,
    ModelDescriptor, Provider, ProviderStatus,
};
use crate::secrets::CredentialStore;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureProviderInput {
    pub provider: Provider,
    /// Sent once, written straight to the OS credential store, never persisted
    /// in SQLite and never logged.
    pub api_key: String,
    /// Optional; falls back to the provider's balanced preset.
    pub model: Option<String>,
}

/// Stores the key, verifies it against the provider, and remembers the chosen
/// model. A key that fails validation is removed again so the app never sits in
/// a "configured but broken" state.
#[tauri::command]
pub async fn configure_provider(
    state: State<'_, AppState>,
    input: ConfigureProviderInput,
) -> CommandResult<ProviderStatus> {
    let provider = input.provider;
    let reference = state.credentials.set(provider, &input.api_key)?;

    let models = match list_models(provider, &input.api_key).await {
        Ok(models) => models,
        Err(error) => {
            // Roll back so `has_*_key` cannot report a key that does not work.
            let _ = state.credentials.delete(provider);
            return Err(error.into());
        }
    };

    let model = input
        .model
        .unwrap_or_else(|| provider.default_model().to_string());
    let model_available = is_model_available(&models, &model);

    {
        let conn = state.db.conn();
        settings_repo::set_credential_ref(&conn, provider, Some(&reference))?;

        let mut patch = settings_repo::SettingsPatch {
            active_provider: Some(provider),
            ..Default::default()
        };
        match provider {
            Provider::OpenAi => patch.openai_model = Some(model.clone()),
            Provider::Anthropic => patch.anthropic_model = Some(model.clone()),
            Provider::DeepSeek => patch.deepseek_model = Some(model.clone()),
        }
        settings_repo::update(&conn, &patch)?;
    }

    tracing::info!(provider = provider.as_str(), "provider configured");

    Ok(ProviderStatus {
        provider,
        valid: true,
        model_available: Some(model_available),
    })
}

/// Re-checks a stored key and whether the stored model is still offered.
#[tauri::command]
pub async fn validate_provider(
    state: State<'_, AppState>,
    provider: Provider,
) -> CommandResult<ProviderStatus> {
    let api_key = require_key(state.credentials.as_ref(), provider)?;
    let models = list_models(provider, &api_key).await?;

    let stored_model = {
        let conn = state.db.conn();
        let settings = settings_repo::get(&conn)?;
        match provider {
            Provider::OpenAi => settings.openai_model,
            Provider::Anthropic => settings.anthropic_model,
            Provider::DeepSeek => settings.deepseek_model,
        }
    };

    Ok(ProviderStatus {
        provider,
        valid: true,
        model_available: stored_model
            .as_deref()
            .map(|model| is_model_available(&models, model)),
    })
}

#[tauri::command]
pub async fn list_provider_models(
    state: State<'_, AppState>,
    provider: Provider,
) -> CommandResult<Vec<ModelDescriptor>> {
    let api_key = require_key(state.credentials.as_ref(), provider)?;
    Ok(list_models(provider, &api_key).await?)
}

/// Forgets a provider key. Local features keep working; AI jobs go back to
/// waiting for a key.
#[tauri::command]
pub fn remove_provider(state: State<'_, AppState>, provider: Provider) -> CommandResult<()> {
    state.credentials.delete(provider)?;

    let conn = state.db.conn();
    settings_repo::set_credential_ref(&conn, provider, None)?;

    let settings = settings_repo::get(&conn)?;
    if settings.active_provider == Some(provider) {
        let fallback = other_configured_provider(&settings, provider);
        conn.execute(
            "UPDATE settings SET active_provider = ?1 WHERE id = 1",
            [fallback.map(|p| p.as_str())],
        )?;
    }

    tracing::info!(provider = provider.as_str(), "provider removed");
    Ok(())
}

/// Picks any other provider that still has a key, so removing the active
/// provider hands off to a working one rather than leaving nothing selected.
fn other_configured_provider(
    settings: &settings_repo::Settings,
    removed: Provider,
) -> Option<Provider> {
    [Provider::OpenAi, Provider::Anthropic, Provider::DeepSeek]
        .into_iter()
        .find(|&candidate| candidate != removed && has_key(settings, candidate))
}

fn has_key(settings: &settings_repo::Settings, provider: Provider) -> bool {
    match provider {
        Provider::OpenAi => settings.has_openai_key,
        Provider::Anthropic => settings.has_anthropic_key,
        Provider::DeepSeek => settings.has_deepseek_key,
    }
}

fn require_key(store: &dyn CredentialStore, provider: Provider) -> Result<String> {
    store
        .get(provider)?
        .ok_or_else(|| AppError::NotFound(format!("{} api key", provider.as_str())))
}

async fn list_models(provider: Provider, api_key: &str) -> Result<Vec<ModelDescriptor>> {
    match provider {
        Provider::OpenAi => OpenAiProvider::new(api_key).list_models().await,
        Provider::Anthropic => AnthropicProvider::new(api_key).list_models().await,
        Provider::DeepSeek => DeepSeekProvider::new(api_key).list_models().await,
    }
}

/// An unavailable model is reported, never silently replaced (§5.1).
fn is_model_available(models: &[ModelDescriptor], model: &str) -> bool {
    models.iter().any(|m| m.id == model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::settings_repo::Settings;
    use crate::secrets::MemoryCredentialStore;

    fn descriptor(id: &str) -> ModelDescriptor {
        ModelDescriptor {
            id: id.into(),
            display_name: id.into(),
        }
    }

    fn settings(openai: bool, anthropic: bool, active: Option<Provider>) -> Settings {
        Settings {
            language: "ko".into(),
            active_provider: active,
            openai_model: None,
            anthropic_model: None,
            deepseek_model: None,
            has_openai_key: openai,
            has_anthropic_key: anthropic,
            has_deepseek_key: false,
            translation_language: "ko".into(),
            translation_engine: "google".into(),
            obsidian_vault_path: None,
            embedding_model_id: "intfloat/multilingual-e5-small".into(),
            embedding_dimension: 384,
            index_generation: 1,
            network_notice_accepted_at: None,
            onboarding_completed_at: None,
        }
    }

    #[test]
    fn model_availability_is_an_exact_match() {
        let models = vec![descriptor("claude-sonnet-5"), descriptor("claude-opus-4-8")];

        assert!(is_model_available(&models, "claude-sonnet-5"));
        assert!(!is_model_available(&models, "claude-sonnet"));
        assert!(!is_model_available(&models, "gpt-5.6-terra"));
    }

    #[test]
    fn removing_the_active_provider_falls_back_to_the_other_configured_one() {
        let state = settings(true, true, Some(Provider::OpenAi));
        assert_eq!(
            other_configured_provider(&state, Provider::OpenAi),
            Some(Provider::Anthropic)
        );
    }

    #[test]
    fn removing_the_only_provider_leaves_none_active() {
        let state = settings(true, false, Some(Provider::OpenAi));
        assert_eq!(other_configured_provider(&state, Provider::OpenAi), None);
    }

    #[test]
    fn a_missing_key_is_reported_as_not_found() {
        let store = MemoryCredentialStore::new();
        let error = require_key(&store, Provider::OpenAi).unwrap_err();
        assert!(matches!(error, AppError::NotFound(_)));
    }

    #[test]
    fn a_stored_key_is_returned_for_the_matching_provider_only() {
        let store = MemoryCredentialStore::new();
        store.set(Provider::Anthropic, "sk-ant-test").unwrap();

        assert_eq!(
            require_key(&store, Provider::Anthropic).unwrap(),
            "sk-ant-test"
        );
        assert!(require_key(&store, Provider::OpenAi).is_err());
    }
}
