//! Threshold alerts (spec FR-013): the three conditions worth interrupting a bug report over.
//!
//! Deliberately not a general alerting system. Each kind fires at most once per
//! [`DEDUPE_WINDOW`] — an OOM kill stays "an OOM kill happened" for ten minutes, and a panel
//! ticking once a second must not turn one event into six hundred.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::resources::model::ResourceSnapshot;

/// One condition, deduplicated by `kind`.
pub const DEDUPE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Memory PSI `full avg10` above this is "the machine is actually stalling", not "busy".
pub const MEMORY_PRESSURE_PERCENT: f64 = 10.0;
/// File descriptors at 80% of the limit leave no room for the next accepted connection.
pub const FD_PRESSURE_RATIO: f64 = 0.8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdAlert {
    pub kind: String,
    pub at: i64,
    pub message: String,
    pub detail: serde_json::Value,
}

/// Stateful part: the previous OOM-kill counter (the kernel's counter is cumulative, so "an OOM
/// kill happened" means it *moved*), plus the last time each kind fired.
#[derive(Default)]
pub struct Alerts {
    previous_oom_kills: Mutex<Option<u64>>,
    last_fired: Mutex<HashMap<String, i64>>,
}

impl Alerts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn global() -> &'static Alerts {
        static ALERTS: std::sync::OnceLock<Alerts> = std::sync::OnceLock::new();
        ALERTS.get_or_init(Alerts::new)
    }

    /// `snapshot` is the observation; `now` is its `at`. Returns only the alerts that are newly due.
    pub fn evaluate(&self, snapshot: &ResourceSnapshot, now: i64) -> Vec<ThresholdAlert> {
        let mut fired = Vec::new();

        if let Some(cgroup) = snapshot.cgroup.as_ref() {
            if let Some(kills) = cgroup.events.oom_kill {
                let mut previous = self
                    .previous_oom_kills
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let moved = previous.is_some_and(|before| kills > before);
                *previous = Some(kills);
                // The first reading is a baseline, not an event: the container may have been killed
                // in long before this panel was ever opened.
                if moved {
                    fired.push(ThresholdAlert {
                        kind: "oom_kill".to_string(),
                        at: now,
                        message: format!("cgroup OOM killer ran {} time(s) in total", kills),
                        detail: serde_json::json!({ "oom_kill_total": kills }),
                    });
                }
            }

            if let Some(full10) = cgroup.psi_memory.full10
                && full10 > MEMORY_PRESSURE_PERCENT
            {
                fired.push(ThresholdAlert {
                    kind: "memory_pressure".to_string(),
                    at: now,
                    message: format!("memory pressure full avg10 {full10:.2}%"),
                    detail: serde_json::json!({ "psi_memory_full10": full10 }),
                });
            }
        }

        if let (Some(count), Some(limit)) = (snapshot.server.fd_count, snapshot.server.fd_limit)
            && limit > 0
        {
            let ratio = count as f64 / limit as f64;
            if ratio > FD_PRESSURE_RATIO {
                fired.push(ThresholdAlert {
                    kind: "fd_pressure".to_string(),
                    at: now,
                    message: format!("{count} of {limit} file descriptors open"),
                    detail: serde_json::json!({ "fd_count": count, "fd_limit": limit }),
                });
            }
        }

        let mut last = self.last_fired.lock().unwrap_or_else(|e| e.into_inner());
        fired.retain(|alert| match last.get(&alert.kind) {
            Some(previous) if now - previous < DEDUPE_WINDOW_MS => false,
            _ => {
                last.insert(alert.kind.clone(), now);
                true
            }
        });
        fired
    }
}

/// Logs one alert and hands it to the tracker as a warning event (FR-013). Called from the sampler,
/// so an alert exists only while somebody is watching — which is the same trade-off the whole panel
/// makes.
pub fn report(alert: &ThresholdAlert) {
    tracing::warn!(kind = %alert.kind, "{}", alert.message);
    crate::diagnostics::report::reporter().capture(
        lanrurugi_storage::diagnostics::Level::Warning,
        &alert.message,
        crate::diagnostics::report::Attrs::new()
            .tag("resources_alert", alert.kind.clone())
            // One stable issue per threshold: the message carries today's number, which must not
            // split "PSI is under pressure" into an issue per reading.
            .tag("fingerprint", format!("resources.{}", alert.kind))
            .extra("detail", alert.detail.clone()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::model::*;

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            at: 0,
            monotonic_ms: 0,
            window_ms: 0,
            version: "t".into(),
            release: "t".into(),
            platform: "t/t".into(),
            build: Default::default(),
            server: Default::default(),
            cgroup: None,
            cgroup_unavailable: None,
            runtime: Default::default(),
            plugins: Default::default(),
            processes: Default::default(),
            models: Default::default(),
            queues: Default::default(),
            redis: None,
            redis_unavailable: None,
            network: Default::default(),
            caches: Default::default(),
            library: Default::default(),
            storage: Default::default(),
            gpu: None,
            gpu_unavailable: None,
            watch: Default::default(),
            truncations: Vec::new(),
        }
    }

    fn with_oom_kill(kills: u64) -> ResourceSnapshot {
        let mut snapshot = snapshot();
        snapshot.cgroup = Some(CgroupSection {
            events: MemoryEvents {
                oom_kill: Some(kills),
                ..Default::default()
            },
            ..Default::default()
        });
        snapshot
    }

    #[test]
    fn a_healthy_snapshot_raises_nothing() {
        let alerts = Alerts::new();
        assert!(alerts.evaluate(&snapshot(), 0).is_empty());
    }

    #[test]
    fn the_first_oom_kill_counter_is_a_baseline_not_an_event() {
        let alerts = Alerts::new();
        assert!(alerts.evaluate(&with_oom_kill(3), 0).is_empty());
        let fired = alerts.evaluate(&with_oom_kill(4), 1);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].kind, "oom_kill");
    }

    #[test]
    fn the_same_kind_does_not_fire_twice_inside_the_dedupe_window() {
        let alerts = Alerts::new();
        alerts.evaluate(&with_oom_kill(0), 0);
        assert_eq!(alerts.evaluate(&with_oom_kill(1), 1).len(), 1);
        assert_eq!(alerts.evaluate(&with_oom_kill(2), 2).len(), 0);
        // Ten minutes later the counter moving again is news once more.
        assert_eq!(
            alerts
                .evaluate(&with_oom_kill(3), 1 + DEDUPE_WINDOW_MS)
                .len(),
            1
        );
    }

    #[test]
    fn memory_pressure_fires_only_past_the_threshold() {
        let alerts = Alerts::new();
        let mut quiet = snapshot();
        quiet.cgroup = Some(CgroupSection {
            psi_memory: Psi {
                full10: Some(9.9),
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(alerts.evaluate(&quiet, 0).is_empty());

        let mut stalled = snapshot();
        stalled.cgroup = Some(CgroupSection {
            psi_memory: Psi {
                full10: Some(12.5),
                ..Default::default()
            },
            ..Default::default()
        });
        let fired = alerts.evaluate(&stalled, 1);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].kind, "memory_pressure");
    }

    #[test]
    fn descriptor_pressure_fires_near_the_limit_and_never_divides_by_zero() {
        let alerts = Alerts::new();
        let mut observation = snapshot();
        observation.server.fd_count = Some(10);
        observation.server.fd_limit = Some(0);
        assert!(alerts.evaluate(&observation, 0).is_empty());

        observation.server.fd_count = Some(81);
        observation.server.fd_limit = Some(100);
        let fired = alerts.evaluate(&observation, 1);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].kind, "fd_pressure");
    }
}
