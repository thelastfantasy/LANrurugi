//! Usage Budget tracking and enforcement (FR-013/FR-014, research.md §10).
//!
//! Tracked **only for metered cloud backends** — a locally-hosted model has no cost to track, so
//! instrumenting that path would be pointless bookkeeping.
//!
//! Consumption is queryable at four granularities (current page, current archive, today, current
//! week) so a user can check spend at any time rather than only finding out when a limit is hit.
//! Day/week counters carry a real TTL so they roll over on their own instead of needing a sweep.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::entities::PageNumber;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BudgetError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, BudgetError>;

/// Day counters outlive their day by a margin so a late read still sees the value.
const DAY_TTL_SECS: u64 = 60 * 60 * 26;
/// Week counters likewise.
const WEEK_TTL_SECS: u64 = 60 * 60 * 24 * 8;
/// Per-page/per-archive counters are session-scoped context, not billing history.
const SESSION_TTL_SECS: u64 = 60 * 60 * 12;

/// Consumption at the four granularities FR-014 requires, plus the configured limit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub provider: String,
    /// `None` means no limit is configured — usage is still reported.
    pub limit: Option<u64>,
    pub consumption_current_page: u64,
    pub consumption_current_archive: u64,
    pub consumption_today: u64,
    pub consumption_current_week: u64,
}

impl UsageSnapshot {
    /// Whether today's consumption has reached the configured limit.
    ///
    /// The daily figure is what the limit applies to: a budget that only ever accumulated would
    /// permanently lock a user out after enough reading, which is not what "limit look-ahead
    /// activity to avoid unexpected charges" means.
    pub fn is_exhausted(&self) -> bool {
        matches!(self.limit, Some(limit) if self.consumption_today >= limit)
    }

    /// Remaining allowance today, if a limit is set.
    pub fn remaining_today(&self) -> Option<u64> {
        self.limit
            .map(|limit| limit.saturating_sub(self.consumption_today))
    }
}

/// Redis keys — this feature's own additive namespace.
fn key_page(provider: &str, archive_id: &ArchiveId, page: PageNumber) -> String {
    format!(
        "LRR_TRANSLATION_USAGE_PAGE_{provider}_{}_{}",
        archive_id.as_str(),
        page.get()
    )
}

fn key_archive(provider: &str, archive_id: &ArchiveId) -> String {
    format!(
        "LRR_TRANSLATION_USAGE_ARCHIVE_{provider}_{}",
        archive_id.as_str()
    )
}

fn key_day(provider: &str, day: &str) -> String {
    format!("LRR_TRANSLATION_USAGE_DAY_{provider}_{day}")
}

fn key_week(provider: &str, week: &str) -> String {
    format!("LRR_TRANSLATION_USAGE_WEEK_{provider}_{week}")
}

fn key_limit(provider: &str) -> String {
    format!("LRR_TRANSLATION_BUDGET_LIMIT_{provider}")
}

/// `YYYY-MM-DD` in the server's local timezone.
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// ISO year-week (`2026-W36`), so the week rolls over on the same boundary a user would expect.
fn current_week() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let iso = now.iso_week();
    format!("{}-W{:02}", iso.year(), iso.week())
}

#[derive(Clone)]
pub struct BudgetRepository {
    pool: Pool,
}

impl BudgetRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Records consumption (in tokens) against every granularity at once.
    pub async fn record(
        &self,
        provider: &str,
        archive_id: &ArchiveId,
        page: PageNumber,
        tokens: u64,
    ) -> Result<()> {
        if tokens == 0 {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;

        for (key, ttl) in [
            (key_page(provider, archive_id, page), SESSION_TTL_SECS),
            (key_archive(provider, archive_id), SESSION_TTL_SECS),
            (key_day(provider, &today()), DAY_TTL_SECS),
            (key_week(provider, &current_week()), WEEK_TTL_SECS),
        ] {
            let _: () = conn.incr(&key, tokens).await?;
            // Refreshed on every write: a counter being actively written to shouldn't expire
            // mid-session, and the day/week keys are named by period anyway.
            let _: () = conn.expire(&key, ttl as i64).await?;
        }
        Ok(())
    }

    /// Reads current consumption at all four granularities (FR-014).
    pub async fn snapshot(
        &self,
        provider: &str,
        archive_id: &ArchiveId,
        page: PageNumber,
    ) -> Result<UsageSnapshot> {
        let mut conn = self.pool.get().await?;

        let values: Vec<Option<u64>> = conn
            .mget(&[
                key_page(provider, archive_id, page),
                key_archive(provider, archive_id),
                key_day(provider, &today()),
                key_week(provider, &current_week()),
                key_limit(provider),
            ])
            .await?;

        let at = |i: usize| values.get(i).copied().flatten().unwrap_or(0);

        Ok(UsageSnapshot {
            provider: provider.to_string(),
            limit: values.get(4).copied().flatten().filter(|&l| l > 0),
            consumption_current_page: at(0),
            consumption_current_archive: at(1),
            consumption_today: at(2),
            consumption_current_week: at(3),
        })
    }

    /// Sets the daily budget. `None` clears it.
    pub async fn set_limit(&self, provider: &str, limit: Option<u64>) -> Result<()> {
        let mut conn = self.pool.get().await?;
        match limit {
            Some(l) => {
                let _: () = conn.set(key_limit(provider), l).await?;
            }
            None => {
                let _: () = conn.del(key_limit(provider)).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_limit_means_never_exhausted() {
        let snapshot = UsageSnapshot {
            consumption_today: 1_000_000,
            limit: None,
            ..Default::default()
        };
        assert!(!snapshot.is_exhausted());
        assert_eq!(snapshot.remaining_today(), None);
    }

    #[test]
    fn reaching_the_limit_exhausts_the_budget() {
        let snapshot = UsageSnapshot {
            consumption_today: 100,
            limit: Some(100),
            ..Default::default()
        };
        assert!(snapshot.is_exhausted());
        assert_eq!(snapshot.remaining_today(), Some(0));
    }

    #[test]
    fn remaining_never_underflows_past_the_limit() {
        let snapshot = UsageSnapshot {
            consumption_today: 150,
            limit: Some(100),
            ..Default::default()
        };
        assert_eq!(snapshot.remaining_today(), Some(0));
    }

    #[test]
    fn usage_below_the_limit_is_not_exhausted() {
        let snapshot = UsageSnapshot {
            consumption_today: 99,
            limit: Some(100),
            ..Default::default()
        };
        assert!(!snapshot.is_exhausted());
        assert_eq!(snapshot.remaining_today(), Some(1));
    }

    #[test]
    fn keys_separate_providers_and_granularities() {
        let archive = ArchiveId::from("abc");
        let page = PageNumber(1);

        assert_ne!(
            key_page("deepseek", &archive, page),
            key_page("anthropic", &archive, page)
        );
        assert_ne!(
            key_page("deepseek", &archive, page),
            key_archive("deepseek", &archive)
        );
        assert!(key_day("deepseek", &today()).contains(&today()));
    }

    #[test]
    fn week_key_uses_iso_week_format() {
        let week = current_week();
        assert!(week.contains("-W"), "got {week}");
    }
}
