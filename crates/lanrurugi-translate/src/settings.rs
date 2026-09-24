//! Server-stored Translation Backend Selection and Target Language Preference (data-model.md,
//! research.md §8).
//!
//! Only the **cloud/API-key** half of backend selection lives here. A locally-hosted selection is
//! deliberately client-side (`localStorage`) and has no server endpoint at all: a `127.0.0.1`
//! target is only meaningful on the device it was configured on, and silently applying it on
//! another device would point at a different machine entirely.
//!
//! Target language is server-stored even though backend selection is split, because a language
//! choice has no such device locality.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_storage::keys::CONFIG_KEY;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::credentials::CredentialRef;

/// Redis `LRR_CONFIG` field names. Additive — no Phase 1 field is touched.
const FIELD_PROVIDER: &str = "translation_provider";
const FIELD_ENDPOINT: &str = "translation_endpoint";
const FIELD_MODEL: &str = "translation_model";
const FIELD_TARGET_LANGUAGE: &str = "translation_target_language";
const FIELD_LOOKAHEAD: &str = "translation_lookahead_pages";
const FIELD_BATCH_PAGES: &str = "translation_batch_pages";

/// Default look-ahead window (FR-011).
pub const DEFAULT_LOOKAHEAD_PAGES: u32 = 3;
/// Pages per translation request (research.md §15: 2–4, mirroring the OCR batch group size).
pub const DEFAULT_BATCH_PAGES: u32 = 3;
pub const MIN_BATCH_PAGES: u32 = 2;
pub const MAX_BATCH_PAGES: u32 = 4;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

/// Which category of backend a selection refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendCategory {
    Cloud,
    Local,
}

/// A cloud provider this build knows how to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudProvider {
    OpenAiCompatible,
    Anthropic,
    DeepSeek,
}

impl CloudProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai-compatible",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai-compatible" => Some(Self::OpenAiCompatible),
            "anthropic" => Some(Self::Anthropic),
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }

    /// The credential handle for this provider. Never the secret itself.
    pub fn credential_ref(self) -> CredentialRef {
        CredentialRef::for_provider(self.as_str())
    }
}

/// The server-stored settings. Note there is no credential field — only a reference, resolved
/// server-side at call time (FR-006).
///
/// Deliberately carries no `enabled` field — whether translation is on is scoped per-archive/
/// per-Tankoubon (`TranslationScopeRepository` below), not a single account-wide switch. This
/// struct is everything *else* that genuinely is account-wide: which backend, which language,
/// how far to look ahead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationSettings {
    pub provider: Option<CloudProvider>,
    /// Non-secret connection detail (base URL).
    pub endpoint: Option<String>,
    pub model: Option<String>,
    /// BCP-47 tag. `None` means "fall back to the browser's own language" — resolved at read time
    /// on the client, never persisted as a value (FR-004).
    pub target_language: Option<String>,
    pub lookahead_pages: u32,
    pub batch_pages: u32,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self {
            provider: None,
            endpoint: None,
            model: None,
            target_language: None,
            lookahead_pages: DEFAULT_LOOKAHEAD_PAGES,
            batch_pages: DEFAULT_BATCH_PAGES,
        }
    }
}

impl TranslationSettings {
    /// Whether a cloud backend is fully configured enough to attempt a call. Drives FR-021's
    /// "guide the user to configuration rather than silently failing".
    pub fn cloud_backend_configured(&self) -> bool {
        self.provider.is_some()
    }

    /// Clamps the batch size into the research.md §15 range.
    pub fn effective_batch_pages(&self) -> u32 {
        self.batch_pages.clamp(MIN_BATCH_PAGES, MAX_BATCH_PAGES)
    }
}

#[derive(Clone)]
pub struct TranslationSettingsRepository {
    pool: Pool,
}

impl TranslationSettingsRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self) -> Result<TranslationSettings, SettingsError> {
        let mut conn = self.pool.get().await?;
        // `hgetall` rather than a multi-field `hget`: `LRR_CONFIG` is a small hash this codebase
        // already reads wholesale elsewhere, and it avoids depending on the multi-field `hget`
        // return shape.
        let all: std::collections::HashMap<String, String> = conn.hgetall(CONFIG_KEY).await?;

        let get = |field: &str| -> Option<String> {
            all.get(field)
                .cloned()
                .filter(|s: &String| !s.trim().is_empty())
        };

        Ok(TranslationSettings {
            provider: get(FIELD_PROVIDER)
                .as_deref()
                .and_then(CloudProvider::parse),
            endpoint: get(FIELD_ENDPOINT),
            model: get(FIELD_MODEL),
            target_language: get(FIELD_TARGET_LANGUAGE),
            lookahead_pages: get(FIELD_LOOKAHEAD)
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_LOOKAHEAD_PAGES),
            batch_pages: get(FIELD_BATCH_PAGES)
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_BATCH_PAGES),
        })
    }

    pub async fn save(&self, settings: &TranslationSettings) -> Result<(), SettingsError> {
        let mut conn = self.pool.get().await?;
        let pairs: Vec<(&str, String)> = vec![
            (
                FIELD_PROVIDER,
                settings
                    .provider
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_default(),
            ),
            (
                FIELD_ENDPOINT,
                settings.endpoint.clone().unwrap_or_default(),
            ),
            (FIELD_MODEL, settings.model.clone().unwrap_or_default()),
            (
                FIELD_TARGET_LANGUAGE,
                settings.target_language.clone().unwrap_or_default(),
            ),
            (FIELD_LOOKAHEAD, settings.lookahead_pages.to_string()),
            (FIELD_BATCH_PAGES, settings.batch_pages.to_string()),
        ];

        let _: () = conn.hset_multiple(CONFIG_KEY, &pairs).await?;
        Ok(())
    }
}

/// Whether translation is on, scoped per-archive or per-Tankoubon rather than account-wide.
///
/// **Why per-scope, not a single global switch**: turning translation on for one book must not
/// silently turn it on for every unrelated archive in the library — the earlier global-`enabled`
/// design did exactly that, confirmed live as a real reported defect. **Why Tankoubon-first,
/// archive-as-fallback** rather than always per-archive: a Tankoubon's member archives are chapters
/// of one continuous work, so a reader who turns translation on partway through expects it to stay
/// on for every subsequent chapter of that same book without re-toggling per file — an archive that
/// isn't a Tankoubon member has no such grouping to inherit from, so it falls back to its own
/// independent key.
///
/// Deliberately two independent keys per Tankoubon-member archive (its own archive-scoped key,
/// which [`Self::is_enabled`] never actually reads while a Tankoubon claims it — see that method)
/// rather than one shared key: keeps the storage shape uniform (every archive always has exactly
/// one key it *could* own outright) and is what makes [`Self::inherit_from_tankoubon`] able to
/// write a real, standalone value into each member the moment the Tankoubon goes away, rather than
/// needing to fabricate the concept of "this archive's own setting" from scratch at delete time.
#[derive(Clone)]
pub struct TranslationScopeRepository {
    pool: Pool,
}

fn archive_scope_key(archive_id: &str) -> String {
    format!("LANRURUGI_TRANSLATION_ENABLED_ARCHIVE_{archive_id}")
}

fn tank_scope_key(tank_id: &str) -> String {
    // `tank_id` is a `TankId`'s own string value, already `TANK_<timestamp>`-prefixed (see
    // `lanrurugi-core::ids`) — no separate `_TANK_` literal here, unlike `archive_scope_key`'s
    // plain `ArchiveId` (which carries no such built-in prefix). An earlier version of this
    // function prepended one anyway, producing keys double-prefixed as `..._TANK_TANK_<id>`
    // (harmless — still unique — but a naming mistake, cleaned up here).
    format!("LANRURUGI_TRANSLATION_ENABLED_{tank_id}")
}

impl TranslationScopeRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Resolves whether translation is on for `archive_id`, given the Tankoubons (if any) it's
    /// currently a member of.
    ///
    /// `member_of` is the caller's job to resolve (typically via `GroupingRepository::for_archive`)
    /// rather than this repository reaching into `lanrurugi-storage` itself — this crate has no
    /// existing dependency on the Tankoubon/Grouping data model, and every call site here already
    /// has that answer in hand from resolving `VolumeId` for font-pattern purposes anyway.
    /// Multiple memberships (an archive can belong to more than one Tankoubon) resolve to "on" if
    /// *any* of them has it on — being findable through more than one collection shouldn't make a
    /// reader's prior choice harder to honour, only easier.
    pub async fn is_enabled(
        &self,
        archive_id: &ArchiveId,
        member_of: &[TankId],
    ) -> Result<bool, SettingsError> {
        let mut conn = self.pool.get().await?;

        for tank_id in member_of {
            let raw: Option<String> = conn.get(tank_scope_key(tank_id.as_str())).await?;
            if raw.as_deref() == Some("1") {
                return Ok(true);
            }
        }
        if !member_of.is_empty() {
            // A Tankoubon-member archive's own key is never consulted while it has a Tankoubon to
            // inherit from — `set_enabled` never writes one for a member archive in the first
            // place (the toggle always targets the Tankoubon), so falling through to it here would
            // just be reading a key that's never actually set for this archive.
            return Ok(false);
        }

        let raw: Option<String> = conn.get(archive_scope_key(archive_id.as_str())).await?;
        Ok(raw.as_deref() == Some("1"))
    }

    /// Sets the switch for whichever scope actually governs `archive_id` right now — the archive
    /// itself if it belongs to no Tankoubon, otherwise every Tankoubon in `member_of` (plural
    /// membership means one toggle affects every collection this archive is filed under, matching
    /// `is_enabled`'s "any membership on means on" resolution).
    pub async fn set_enabled(
        &self,
        archive_id: &ArchiveId,
        member_of: &[TankId],
        enabled: bool,
    ) -> Result<(), SettingsError> {
        let mut conn = self.pool.get().await?;
        let value = if enabled { "1" } else { "0" };

        if member_of.is_empty() {
            let _: () = conn
                .set(archive_scope_key(archive_id.as_str()), value)
                .await?;
            return Ok(());
        }
        for tank_id in member_of {
            let _: () = conn.set(tank_scope_key(tank_id.as_str()), value).await?;
        }
        Ok(())
    }

    /// Pushes a Tankoubon's translation switch down onto every one of its member archives' own
    /// keys, then clears the Tankoubon's own key — called when a Tankoubon is deleted, so each
    /// former member keeps behaving exactly as it did a moment before, as a standalone archive,
    /// instead of silently reverting to "off" the instant it loses its collection.
    pub async fn inherit_from_tankoubon(
        &self,
        tank_id: &TankId,
        member_archive_ids: &[ArchiveId],
    ) -> Result<(), SettingsError> {
        let mut conn = self.pool.get().await?;
        let key = tank_scope_key(tank_id.as_str());
        let raw: Option<String> = conn.get(&key).await?;
        let value = if raw.as_deref() == Some("1") {
            "1"
        } else {
            "0"
        };

        for archive_id in member_archive_ids {
            let _: () = conn
                .set(archive_scope_key(archive_id.as_str()), value)
                .await?;
        }
        let _: () = conn.del(&key).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_round_trips_through_its_wire_name() {
        for p in [
            CloudProvider::OpenAiCompatible,
            CloudProvider::Anthropic,
            CloudProvider::DeepSeek,
        ] {
            assert_eq!(CloudProvider::parse(p.as_str()), Some(p));
        }
        assert_eq!(CloudProvider::parse("nope"), None);
    }

    #[test]
    fn batch_size_is_clamped_into_the_supported_range() {
        let mut s = TranslationSettings {
            batch_pages: 99,
            ..Default::default()
        };
        assert_eq!(s.effective_batch_pages(), MAX_BATCH_PAGES);
        s.batch_pages = 0;
        assert_eq!(s.effective_batch_pages(), MIN_BATCH_PAGES);
    }

    #[test]
    fn an_unset_target_language_stays_none_for_browser_fallback() {
        // FR-004: absence means "use the browser's own language", resolved client-side — it must
        // not be persisted as a concrete value here.
        assert_eq!(TranslationSettings::default().target_language, None);
    }

    #[test]
    fn a_backend_is_unconfigured_until_a_provider_is_chosen() {
        let mut s = TranslationSettings::default();
        assert!(!s.cloud_backend_configured());
        s.provider = Some(CloudProvider::DeepSeek);
        assert!(s.cloud_backend_configured());
    }

    #[test]
    fn settings_carry_a_credential_reference_never_a_secret() {
        let json = serde_json::to_string(&TranslationSettings {
            provider: Some(CloudProvider::Anthropic),
            ..Default::default()
        })
        .unwrap();
        assert!(
            !json.contains("api_key"),
            "settings must never carry a secret"
        );
    }
}
