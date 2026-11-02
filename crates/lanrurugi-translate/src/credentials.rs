//! Server-side credential resolution (constitution Principle V, FR-006).
//!
//! A cloud provider's API key lives only in Redis and is read here, immediately before the provider
//! call. It is never returned in an API response, never logged, and never reaches the browser —
//! callers outside this crate work with a [`CredentialRef`] (an opaque handle) rather than the
//! secret itself.
//!
//! Follows the same shape Phase 1 already uses for `llm_api_key`: the settings surface exposes only
//! a "is a key set" boolean, never the value.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_storage::keys::CONFIG_KEY;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("no credential is configured for provider {0:?}")]
    NotConfigured(String),
}

/// An opaque reference to a stored secret. Deliberately carries no secret material, so it is safe
/// to include in responses, logs, and backups.
///
/// `Debug` is hand-written for the same reason the secret type below is: nothing about a credential
/// should ever be printable by accident.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(pub String);

impl CredentialRef {
    /// The Redis `LRR_CONFIG` field holding this provider's key.
    pub fn for_provider(provider: &str) -> Self {
        Self(format!("translation_api_key_{provider}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The ref itself is not secret, but printing it alongside secrets in a struct dump
        // invites confusion about which is which.
        write!(f, "CredentialRef({})", self.0)
    }
}

/// A resolved secret. Never `Debug`-printable, never `Display`-able, never serializable — the type
/// system is what keeps it out of a log line or a response body.
pub struct Secret(String);

impl Secret {
    /// The only way to read the value. Named to make every call site visibly deliberate.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Clone)]
pub struct CredentialStore {
    pool: Pool,
}

impl CredentialStore {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Resolves a credential immediately before a provider call.
    pub async fn resolve(&self, cred_ref: &CredentialRef) -> Result<Secret, CredentialError> {
        let mut conn = self.pool.get().await?;
        let value: Option<String> = conn.hget(CONFIG_KEY, cred_ref.as_str()).await?;

        value
            .filter(|v| !v.trim().is_empty())
            .map(Secret)
            .ok_or_else(|| CredentialError::NotConfigured(cred_ref.as_str().to_string()))
    }

    /// Stores/updates a provider's credential.
    pub async fn store(
        &self,
        cred_ref: &CredentialRef,
        secret: &str,
    ) -> Result<(), CredentialError> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hset(CONFIG_KEY, cred_ref.as_str(), secret).await?;
        Ok(())
    }

    /// Removes a provider's credential.
    pub async fn clear(&self, cred_ref: &CredentialRef) -> Result<(), CredentialError> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(CONFIG_KEY, cred_ref.as_str()).await?;
        Ok(())
    }

    /// Whether a credential is set — the only credential fact any API response may expose
    /// (FR-006).
    pub async fn is_set(&self, cred_ref: &CredentialRef) -> Result<bool, CredentialError> {
        let mut conn = self.pool.get().await?;
        let value: Option<String> = conn.hget(CONFIG_KEY, cred_ref.as_str()).await?;
        Ok(value.is_some_and(|v| !v.trim().is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_refs_are_namespaced_per_provider() {
        assert_eq!(
            CredentialRef::for_provider("anthropic").as_str(),
            "translation_api_key_anthropic"
        );
        assert_ne!(
            CredentialRef::for_provider("anthropic"),
            CredentialRef::for_provider("deepseek")
        );
    }

    #[test]
    fn a_secret_never_prints_its_value() {
        let secret = Secret("sk-super-secret-value".to_string());
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("sk-super-secret-value"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn exposing_a_secret_is_explicit_and_returns_the_value() {
        let secret = Secret("sk-abc".to_string());
        assert_eq!(secret.expose(), "sk-abc");
    }

    #[test]
    fn a_credential_ref_carries_no_secret_material() {
        // The ref is safe to serialize; it names a Redis field, it isn't the value.
        let json = serde_json::to_string(&CredentialRef::for_provider("openai")).unwrap();
        assert_eq!(json, "\"translation_api_key_openai\"");
    }
}
