use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{AppError, Result};
use crate::providers::Provider;

const SERVICE: &str = "com.bbrain.desktop";

/// API keys live in the OS credential store (Keychain / Credential Manager).
/// SQLite only ever holds the reference returned by `credential_ref`
/// (DEVELOPMENT.md §16.1).
pub trait CredentialStore: Send + Sync {
    fn set(&self, provider: Provider, api_key: &str) -> Result<String>;
    fn get(&self, provider: Provider) -> Result<Option<String>>;
    fn delete(&self, provider: Provider) -> Result<()>;
}

pub fn credential_ref(provider: Provider) -> String {
    format!("{SERVICE}/{}", provider.as_str())
}

pub struct OsCredentialStore;

impl OsCredentialStore {
    /// Wrapped in the session cache so the OS keychain is read at most once per
    /// provider per session (fewer OS prompts, less key exposure — §16.1).
    pub fn new() -> CachingCredentialStore<Self> {
        CachingCredentialStore::new(Self)
    }

    fn entry(provider: Provider) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new(SERVICE, provider.as_str())?)
    }
}

impl CredentialStore for OsCredentialStore {
    fn set(&self, provider: Provider, api_key: &str) -> Result<String> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err(AppError::InvalidInput("empty api key".into()));
        }
        Self::entry(provider)?.set_password(key)?;
        Ok(credential_ref(provider))
    }

    fn get(&self, provider: Provider) -> Result<Option<String>> {
        match Self::entry(provider)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, provider: Provider) -> Result<()> {
        match Self::entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The Obsidian Local REST API key sits beside the provider keys in the OS
/// credential store, but is not an LLM provider — so it gets its own account
/// name rather than a `Provider` variant. Cached in memory after the first
/// read (same §16.1 rationale as the provider cache: fewer keychain prompts).
pub const OBSIDIAN_REST_ACCOUNT: &str = "obsidian-rest";

static OBSIDIAN_KEY_CACHE: std::sync::Mutex<Option<Option<String>>> =
    std::sync::Mutex::new(None);

fn obsidian_entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, OBSIDIAN_REST_ACCOUNT)?)
}

pub fn set_obsidian_rest_key(api_key: &str) -> Result<String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(AppError::InvalidInput("empty api key".into()));
    }
    obsidian_entry()?.set_password(key)?;
    *OBSIDIAN_KEY_CACHE.lock().unwrap() = Some(Some(key.to_string()));
    Ok(format!("{SERVICE}/{OBSIDIAN_REST_ACCOUNT}"))
}

pub fn get_obsidian_rest_key() -> Result<Option<String>> {
    if let Some(cached) = OBSIDIAN_KEY_CACHE.lock().unwrap().clone() {
        return Ok(cached);
    }
    let value = match obsidian_entry()?.get_password() {
        Ok(key) => Some(key),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => return Err(e.into()),
    };
    *OBSIDIAN_KEY_CACHE.lock().unwrap() = Some(value.clone());
    Ok(value)
}

pub fn delete_obsidian_rest_key() -> Result<()> {
    match obsidian_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(e.into()),
    }
    *OBSIDIAN_KEY_CACHE.lock().unwrap() = Some(None);
    Ok(())
}

/// Semantic Scholar is not an LLM provider, so its optional API key uses a
/// dedicated keychain account and never expands the `Provider` enum.
pub const SEMANTIC_SCHOLAR_ACCOUNT: &str = "semantic-scholar";

static SEMANTIC_SCHOLAR_KEY_CACHE: std::sync::Mutex<Option<Option<String>>> =
    std::sync::Mutex::new(None);

fn semantic_scholar_entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, SEMANTIC_SCHOLAR_ACCOUNT)?)
}

pub fn set_semantic_scholar_key(api_key: &str) -> Result<String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(AppError::InvalidInput("empty semantic scholar api key".into()));
    }
    semantic_scholar_entry()?.set_password(key)?;
    *SEMANTIC_SCHOLAR_KEY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Some(key.to_string()));
    Ok(format!("{SERVICE}/{SEMANTIC_SCHOLAR_ACCOUNT}"))
}

pub fn get_semantic_scholar_key() -> Result<Option<String>> {
    if let Some(cached) = SEMANTIC_SCHOLAR_KEY_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(cached);
    }
    let value = match semantic_scholar_entry()?.get_password() {
        Ok(key) => Some(key),
        Err(keyring::Error::NoEntry) => None,
        Err(error) => return Err(error.into()),
    };
    *SEMANTIC_SCHOLAR_KEY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(value.clone());
    Ok(value)
}

pub fn delete_semantic_scholar_key() -> Result<()> {
    match semantic_scholar_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(error) => return Err(error.into()),
    }
    *SEMANTIC_SCHOLAR_KEY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
    Ok(())
}

/// Serves each provider's key from memory after the first read, and invalidates
/// that entry on any write, so a re-keyed provider is never served a stale
/// value. Wraps any `CredentialStore`.
pub struct CachingCredentialStore<S: CredentialStore> {
    inner: S,
    cache: Mutex<HashMap<Provider, String>>,
}

impl<S: CredentialStore> CachingCredentialStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<Provider, String>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<S: CredentialStore> CredentialStore for CachingCredentialStore<S> {
    fn set(&self, provider: Provider, api_key: &str) -> Result<String> {
        let reference = self.inner.set(provider, api_key)?;
        self.cache().insert(provider, api_key.trim().to_string());
        Ok(reference)
    }

    fn get(&self, provider: Provider) -> Result<Option<String>> {
        if let Some(key) = self.cache().get(&provider) {
            return Ok(Some(key.clone()));
        }
        match self.inner.get(provider)? {
            Some(key) => {
                self.cache().insert(provider, key.clone());
                Ok(Some(key))
            }
            None => Ok(None),
        }
    }

    fn delete(&self, provider: Provider) -> Result<()> {
        self.cache().remove(&provider);
        self.inner.delete(provider)
    }
}

/// In-memory store used by tests so they never touch the developer's keychain.
/// Counts reads so a test can prove the cache avoids re-reading.
#[cfg(test)]
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<&'static str, String>>,
    reads: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn read_count(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn set(&self, provider: Provider, api_key: &str) -> Result<String> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err(AppError::InvalidInput("empty api key".into()));
        }
        self.entries
            .lock()
            .unwrap()
            .insert(provider.as_str(), key.to_string());
        Ok(credential_ref(provider))
    }

    fn get(&self, provider: Provider) -> Result<Option<String>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.entries.lock().unwrap().get(provider.as_str()).cloned())
    }

    fn delete(&self, provider: Provider) -> Result<()> {
        self.entries.lock().unwrap().remove(provider.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_ref_is_scoped_per_provider() {
        assert_eq!(credential_ref(Provider::OpenAi), "com.bbrain.desktop/openai");
        assert_eq!(
            credential_ref(Provider::Anthropic),
            "com.bbrain.desktop/anthropic"
        );
    }

    #[test]
    fn store_round_trips_and_deletes() {
        let store = MemoryCredentialStore::new();

        let reference = store.set(Provider::OpenAi, "  sk-test  ").unwrap();
        assert_eq!(reference, credential_ref(Provider::OpenAi));
        assert_eq!(store.get(Provider::OpenAi).unwrap().as_deref(), Some("sk-test"));

        store.delete(Provider::OpenAi).unwrap();
        assert!(store.get(Provider::OpenAi).unwrap().is_none());
    }

    #[test]
    fn rejects_an_empty_key() {
        let store = MemoryCredentialStore::new();
        let error = store.set(Provider::Anthropic, "   ").unwrap_err();
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn the_cache_reads_the_backing_store_only_once_per_provider() {
        let store = CachingCredentialStore::new(MemoryCredentialStore::new());
        store.set(Provider::Anthropic, "sk-ant").unwrap();

        // set seeds the cache, so these reads never reach the backing store.
        assert_eq!(store.get(Provider::Anthropic).unwrap().as_deref(), Some("sk-ant"));
        assert_eq!(store.get(Provider::Anthropic).unwrap().as_deref(), Some("sk-ant"));
        assert_eq!(store.inner.read_count(), 0);
    }

    #[test]
    fn a_cache_miss_reads_the_backing_store_once_then_serves_from_memory() {
        let inner = MemoryCredentialStore::new();
        inner.set(Provider::OpenAi, "sk-openai").unwrap();
        let store = CachingCredentialStore::new(inner);

        store.get(Provider::OpenAi).unwrap();
        store.get(Provider::OpenAi).unwrap();

        assert_eq!(store.inner.read_count(), 1, "the second read is served from cache");
    }

    #[test]
    fn re_keying_a_provider_invalidates_the_cached_value() {
        let store = CachingCredentialStore::new(MemoryCredentialStore::new());
        store.set(Provider::OpenAi, "old-key").unwrap();

        store.set(Provider::OpenAi, "new-key").unwrap();

        assert_eq!(store.get(Provider::OpenAi).unwrap().as_deref(), Some("new-key"));
    }

    #[test]
    fn deleting_evicts_the_cached_value() {
        let store = CachingCredentialStore::new(MemoryCredentialStore::new());
        store.set(Provider::Anthropic, "sk-ant").unwrap();

        store.delete(Provider::Anthropic).unwrap();

        assert!(store.get(Provider::Anthropic).unwrap().is_none());
    }

    #[test]
    fn deleting_a_missing_key_succeeds() {
        let store = MemoryCredentialStore::new();
        assert!(store.delete(Provider::Anthropic).is_ok());
    }
}
