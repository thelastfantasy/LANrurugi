//! Per-call measurement and the per-feature usage ledger (`specs/014-ai-harness-hardening/
//! data-model.md` §0–§2). Every model call yields a [`ModelCallReport`]; [`record`] folds it into a
//! day-bucketed Redis hash so the figures survive restarts. Token figures are `None` when the
//! provider did not report them — never `0` as a stand-in.

use std::time::Duration;

use deadpool_redis::Pool;
use tracing::warn;

use crate::error::LlmErrorClass;

/// Which AI feature issued a call. Supplied by the caller; the seam never infers it, because an
/// inferred id silently misattributes the moment a caller is refactored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FeatureId(&'static str);

impl FeatureId {
    pub const PLUGIN_GENERATION: Self = Self("plugin_generation");
    pub const PLUGIN_LOGIN_ANALYSIS: Self = Self("plugin_login_analysis");
    pub const PLUGIN_TRIAL_ATTRIBUTION: Self = Self("plugin_trial_attribution");
    pub const ARCHIVE_SPLIT: Self = Self("archive_split");
    pub const SUBFOLDER_TANKOUBON_NAMING: Self = Self("subfolder_tankoubon_naming");
    pub const ARTIST_BACKFILL: Self = Self("artist_backfill");
    pub const TANKOUBON_RENAME: Self = Self("tankoubon_rename");
    pub const RECOMMEND_RERANK: Self = Self("recommend_rerank");
    pub const SUBSCRIPTION_CONDITION: Self = Self("subscription_condition");
    pub const PROVIDER_PRICING: Self = Self("provider_pricing");
    pub const TRANSLATION: Self = Self("translation");
    pub const TANKOUBON_GROUPING_REVIEW: Self = Self("tankoubon_grouping_review");

    /// Every feature, in the order the usage endpoint reports them.
    pub const ALL: [Self; 12] = [
        Self::PLUGIN_GENERATION,
        Self::PLUGIN_LOGIN_ANALYSIS,
        Self::PLUGIN_TRIAL_ATTRIBUTION,
        Self::ARCHIVE_SPLIT,
        Self::SUBFOLDER_TANKOUBON_NAMING,
        Self::ARTIST_BACKFILL,
        Self::TANKOUBON_RENAME,
        Self::RECOMMEND_RERANK,
        Self::SUBSCRIPTION_CONDITION,
        Self::PROVIDER_PRICING,
        Self::TRANSLATION,
        Self::TANKOUBON_GROUPING_REVIEW,
    ];

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// Token figures from one provider response. Each is `None` when not reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: Option<u64>,
    pub completion: Option<u64>,
    pub total: Option<u64>,
    pub cache_hit: Option<u64>,
    pub cache_miss: Option<u64>,
    pub reasoning: Option<u64>,
}

impl TokenUsage {
    /// Reads a chat-completions `usage` block (`body["usage"]`, or a streamed chunk's).
    pub fn from_usage_block(usage: &serde_json::Value) -> Self {
        let n = |v: &serde_json::Value| v.as_u64();
        Self {
            prompt: n(&usage["prompt_tokens"]),
            completion: n(&usage["completion_tokens"]),
            total: n(&usage["total_tokens"]),
            cache_hit: n(&usage["prompt_cache_hit_tokens"]),
            cache_miss: n(&usage["prompt_cache_miss_tokens"]),
            reasoning: n(&usage["completion_tokens_details"]["reasoning_tokens"]),
        }
    }

    pub fn is_reported(&self) -> bool {
        self.prompt.is_some() || self.completion.is_some() || self.total.is_some()
    }
}

impl std::ops::Add for TokenUsage {
    type Output = Self;

    /// Sums two attempts' figures; a field stays `None` only if neither attempt reported it.
    fn add(self, other: Self) -> Self {
        let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        Self {
            prompt: sum(self.prompt, other.prompt),
            completion: sum(self.completion, other.completion),
            total: sum(self.total, other.total),
            cache_hit: sum(self.cache_hit, other.cache_hit),
            cache_miss: sum(self.cache_miss, other.cache_miss),
            reasoning: sum(self.reasoning, other.reasoning),
        }
    }
}

/// What one call cost and how it ended. `outcome` is `None` on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallReport {
    pub feature: FeatureId,
    pub outcome: Option<LlmErrorClass>,
    pub attempts: u32,
    pub elapsed: Duration,
    pub usage: TokenUsage,
}

/// Day keys live this long, so a week window is always fully answerable.
pub use lanrurugi_storage::keys::LLM_USAGE_DAILY_TTL_SECS;

/// Counter field for a failure class in the day hash.
fn failure_field(class: LlmErrorClass) -> Option<&'static str> {
    Some(match class {
        LlmErrorClass::NotConfigured => return None,
        LlmErrorClass::Auth => "fail_auth",
        LlmErrorClass::Balance => "fail_balance",
        LlmErrorClass::RateLimited => "fail_rate_limited",
        LlmErrorClass::Transient => "fail_transient",
        LlmErrorClass::Timeout => "fail_timeout",
        LlmErrorClass::Malformed => "fail_malformed",
        LlmErrorClass::Rejected => "fail_rejected",
        LlmErrorClass::Cancelled => "cancelled",
    })
}

/// The hash increments one report contributes, as `(field, amount)`.
pub fn increments(report: &ModelCallReport) -> Vec<(&'static str, u64)> {
    let mut out = vec![
        ("requests", 1),
        ("elapsed_ms", report.elapsed.as_millis() as u64),
    ];
    let u = &report.usage;
    for (field, value) in [
        ("prompt_tokens", u.prompt),
        ("completion_tokens", u.completion),
        ("total_tokens", u.total),
        ("prompt_cache_hit_tokens", u.cache_hit),
        ("prompt_cache_miss_tokens", u.cache_miss),
        ("reasoning_tokens", u.reasoning),
    ] {
        if let Some(v) = value {
            out.push((field, v));
        }
    }
    if !u.is_reported() {
        out.push(("unreported_token_calls", 1));
    }
    if let Some(field) = report.outcome.and_then(failure_field) {
        out.push((field, 1));
    }
    out
}

/// Folds one report into today's ledger hash. Best effort: a Redis failure is logged, never
/// surfaced, because the ledger observes calls and must not change their outcome.
pub async fn record(redis_config: &Pool, report: &ModelCallReport) {
    write(redis_config, report.feature, &increments(report)).await;
}

/// Counts one answer a feature served from its own result cache — a model call it did not need
/// to make. Kept apart from `requests`, which only ever counts calls that reached the provider.
pub async fn record_local_cache_hit(redis_config: &Pool, feature: FeatureId) {
    write(redis_config, feature, &[("local_cache_hits", 1)]).await;
}

/// Counts one conversation round that did not extend the previous round's request
/// (`crate::prefix`), so the provider's prefix cache missed from that point on.
pub async fn record_prefix_break(redis_config: &Pool, feature: FeatureId) {
    write(redis_config, feature, &[("prefix_breaks", 1)]).await;
}

async fn write(redis_config: &Pool, feature: FeatureId, fields: &[(&'static str, u64)]) {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let key = lanrurugi_storage::keys::llm_usage_daily_key(feature.as_str(), &date);
    let mut pipe = deadpool_redis::redis::pipe();
    for (field, amount) in fields {
        pipe.hincr(&key, *field, *amount).ignore();
    }
    pipe.cmd("EXPIRE")
        .arg(&key)
        .arg(LLM_USAGE_DAILY_TTL_SECS)
        .arg("NX")
        .ignore();
    let result = match redis_config.get().await {
        Ok(mut conn) => pipe
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = result {
        warn!(feature = feature.as_str(), error = %e, "llm usage ledger write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_block_reads_cache_and_reasoning_tokens() {
        let usage = TokenUsage::from_usage_block(&json!({
            "prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120,
            "prompt_cache_hit_tokens": 70, "prompt_cache_miss_tokens": 30,
            "completion_tokens_details": { "reasoning_tokens": 5 }
        }));
        assert_eq!(usage.cache_hit, Some(70));
        assert_eq!(usage.cache_miss, Some(30));
        assert_eq!(usage.reasoning, Some(5));
        assert!(usage.is_reported());
    }

    #[test]
    fn absent_usage_is_none_not_zero() {
        let usage = TokenUsage::from_usage_block(&serde_json::Value::Null);
        assert_eq!(usage, TokenUsage::default());
        assert!(!usage.is_reported());
    }

    #[test]
    fn increments_count_unreported_calls_and_failures() {
        let report = ModelCallReport {
            feature: FeatureId::ARCHIVE_SPLIT,
            outcome: Some(LlmErrorClass::Timeout),
            attempts: 1,
            elapsed: Duration::from_millis(1500),
            usage: TokenUsage::default(),
        };
        let fields = increments(&report);
        assert!(fields.contains(&("requests", 1)));
        assert!(fields.contains(&("elapsed_ms", 1500)));
        assert!(fields.contains(&("unreported_token_calls", 1)));
        assert!(fields.contains(&("fail_timeout", 1)));
        assert!(!fields.iter().any(|(f, _)| *f == "prompt_tokens"));
    }

    #[test]
    fn cancelled_is_its_own_counter() {
        let report = ModelCallReport {
            feature: FeatureId::PLUGIN_GENERATION,
            outcome: Some(LlmErrorClass::Cancelled),
            attempts: 1,
            elapsed: Duration::ZERO,
            usage: TokenUsage::default(),
        };
        let fields = increments(&report);
        assert!(fields.contains(&("cancelled", 1)));
        assert!(!fields.iter().any(|(f, _)| f.starts_with("fail_")));
    }

    #[test]
    fn adding_attempts_keeps_unreported_fields_none() {
        let a = TokenUsage {
            prompt: Some(10),
            ..Default::default()
        };
        let sum = a + TokenUsage::default();
        assert_eq!(sum.prompt, Some(10));
        assert_eq!(sum.reasoning, None);
    }
}
