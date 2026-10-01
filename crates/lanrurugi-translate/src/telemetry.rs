//! Lightweight counters making SC-003 (font-match rate) and SC-004 (look-ahead readiness rate)
//! actually measurable over real usage (T063).
//!
//! Both success criteria are stated as "at least X% of ...", which a single qualitative quickstart
//! run can't demonstrate. These counters are in-process and reset on restart — deliberately: they
//! exist to answer "is this working in practice", not to be a billing or audit record (the Activity
//! log covers auditability, and the Usage Budget covers spend).

use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide counters. `Relaxed` ordering throughout: these are statistics, and paying for
/// stronger ordering on a hot path to make a ratio marginally more precise isn't a good trade.
#[derive(Debug, Default)]
pub struct TranslationTelemetry {
    /// Blocks rendered in a font from the volume's locked golden set.
    font_matched: AtomicU64,
    /// Blocks rendered via meltdown re-classification or the fallback font instead.
    font_unmatched: AtomicU64,
    /// Pages the reader reached that were already translated.
    lookahead_hit: AtomicU64,
    /// Pages the reader reached before look-ahead finished (FR-012's loading state).
    lookahead_miss: AtomicU64,
}

impl TranslationTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_font_match(&self, matched: bool) {
        let counter = if matched {
            &self.font_matched
        } else {
            &self.font_unmatched
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_lookahead(&self, was_ready: bool) {
        let counter = if was_ready {
            &self.lookahead_hit
        } else {
            &self.lookahead_miss
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// SC-003: share of blocks drawn in a golden-set font. `None` until there is any data, so an
    /// idle instance reports "no measurement" rather than a misleading 0% or 100%.
    pub fn font_match_rate(&self) -> Option<f64> {
        ratio(
            self.font_matched.load(Ordering::Relaxed),
            self.font_unmatched.load(Ordering::Relaxed),
        )
    }

    /// SC-004: share of reached pages that were ready in time.
    pub fn lookahead_ready_rate(&self) -> Option<f64> {
        ratio(
            self.lookahead_hit.load(Ordering::Relaxed),
            self.lookahead_miss.load(Ordering::Relaxed),
        )
    }

    /// Emits both rates to the log — called periodically so the numbers show up in real
    /// deployments without needing a metrics stack.
    pub fn log_summary(&self) {
        match (self.font_match_rate(), self.lookahead_ready_rate()) {
            (None, None) => {}
            (font, lookahead) => tracing::info!(
                font_match_rate = ?font,
                lookahead_ready_rate = ?lookahead,
                "translation telemetry"
            ),
        }
    }
}

fn ratio(hits: u64, misses: u64) -> Option<f64> {
    let total = hits + misses;
    (total > 0).then(|| hits as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_unset_before_any_data() {
        let t = TranslationTelemetry::new();
        assert_eq!(t.font_match_rate(), None);
        assert_eq!(t.lookahead_ready_rate(), None);
    }

    #[test]
    fn font_match_rate_reflects_recorded_outcomes() {
        let t = TranslationTelemetry::new();
        for _ in 0..9 {
            t.record_font_match(true);
        }
        t.record_font_match(false);
        assert_eq!(t.font_match_rate(), Some(0.9));
    }

    #[test]
    fn lookahead_rate_reflects_recorded_outcomes() {
        let t = TranslationTelemetry::new();
        for _ in 0..19 {
            t.record_lookahead(true);
        }
        t.record_lookahead(false);
        assert_eq!(t.lookahead_ready_rate(), Some(0.95));
    }

    #[test]
    fn the_two_metrics_are_independent() {
        let t = TranslationTelemetry::new();
        t.record_font_match(true);
        assert_eq!(t.font_match_rate(), Some(1.0));
        assert_eq!(t.lookahead_ready_rate(), None);
    }
}
