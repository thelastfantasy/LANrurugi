//! Upload caps for sharing and the BitTorrent-style alternative speed schedule (FR-041–FR-044,
//! research §14).
//!
//! Two independent groups: uploads for this instance's own shares, and uploads made while helping
//! deliver other instances' shares. Each has a byte rate and a cap on importers served at once, in
//! a normal and an alternative set; a schedule or a manual switch picks the set. An alternative
//! own-share rate of 0 pauses own-share uploads (imports and file downloads) but never reading.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Duration as ChronoDuration, TimeZone, Timelike, Utc};
use serde::Serialize;

use crate::download_manager::rate_limit::RateLimiterMap;
use crate::share_settings::{parse_hhmm, AltSchedule, ShareSettings};
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Own,
    Helping,
}

impl Group {
    fn key(self) -> &'static str {
        match self {
            Self::Own => "share-upload-own",
            Self::Helping => "share-upload-helping",
        }
    }
}

/// The caps one group is held to right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Bytes per second; `None` is unlimited, `Some(0)` paused.
    pub rate: Option<u64>,
    pub max_peers: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModeStatus {
    /// `normal` or `alternative`.
    pub upload_mode: &'static str,
    /// `schedule`, `manual` or `none`.
    pub upload_mode_source: &'static str,
    /// Unix seconds of the next scheduled switch, if any.
    pub next_switch_at: Option<i64>,
}

/// Whether the schedule puts `at` inside an alternative window. A window whose end is not after
/// its start runs past midnight; the day filter applies to the day the window starts.
pub fn schedule_active<Tz: TimeZone>(schedule: &AltSchedule, at: &DateTime<Tz>) -> bool {
    if !schedule.enabled {
        return false;
    }
    let minute = at.hour() * 60 + at.minute();
    let today = at.weekday().number_from_monday() as u8;
    let yesterday = if today == 1 { 7 } else { today - 1 };
    let day_ok = |d: u8| schedule.days.is_empty() || schedule.days.contains(&d);
    schedule.windows.iter().any(|w| {
        let (Some(from), Some(to)) = (parse_hhmm(&w.from), parse_hhmm(&w.to)) else {
            return false;
        };
        if from < to {
            day_ok(today) && minute >= from && minute < to
        } else {
            (day_ok(today) && minute >= from) || (day_ok(yesterday) && minute < to)
        }
    })
}

/// The first minute after `now` at which the schedule's state flips, searched up to eight days.
pub fn next_switch<Tz: TimeZone>(
    schedule: &AltSchedule,
    now: &DateTime<Tz>,
) -> Option<DateTime<Tz>> {
    if !schedule.enabled || schedule.windows.is_empty() {
        return None;
    }
    let current = schedule_active(schedule, now);
    let start = now.clone() - ChronoDuration::seconds(now.second() as i64);
    (1..=8 * 24 * 60)
        .map(|m| start.clone() + ChronoDuration::minutes(m))
        .find(|t| schedule_active(schedule, t) != current)
}

fn cap(settings: &ShareSettings, name: &str) -> Option<u64> {
    settings.number(name)
}

/// The mode and caps in force at `now` (in the instance's time zone).
pub fn evaluate<Tz: TimeZone>(
    settings: &ShareSettings,
    now: &DateTime<Tz>,
) -> (ModeStatus, Caps, Caps) {
    let schedule = settings.alt_schedule();
    let manual = settings.flag("share_alt_manual");
    let scheduled = schedule_active(&schedule, now);
    let alternative = manual || scheduled;
    let status = ModeStatus {
        upload_mode: if alternative { "alternative" } else { "normal" },
        upload_mode_source: if manual {
            "manual"
        } else if scheduled {
            "schedule"
        } else {
            "none"
        },
        next_switch_at: if manual {
            None
        } else {
            next_switch(&schedule, now).map(|t| t.timestamp())
        },
    };
    let normal_own = Caps {
        rate: cap(settings, "share_upload_limit"),
        max_peers: cap(settings, "share_max_peers"),
    };
    let normal_help = Caps {
        rate: cap(settings, "share_helper_upload_limit"),
        max_peers: cap(settings, "share_helper_max_peers"),
    };
    if !alternative {
        return (status, normal_own, normal_help);
    }
    // An empty alternative field keeps the normal value.
    let alt = |name: &str, normal: Option<u64>| {
        if settings.text(name).trim().is_empty() {
            normal
        } else {
            cap(settings, name)
        }
    };
    let own = Caps {
        rate: alt("share_alt_upload_limit", normal_own.rate),
        max_peers: alt("share_alt_max_peers", normal_own.max_peers),
    };
    let help = Caps {
        rate: alt("share_alt_helper_upload_limit", normal_help.rate),
        max_peers: alt("share_alt_helper_max_peers", normal_help.max_peers),
    };
    (status, own, help)
}

async fn timezone(state: &AppState) -> chrono_tz::Tz {
    use deadpool_redis::redis::AsyncCommands;
    let name: Option<String> = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget(lanrurugi_storage::keys::CONFIG_KEY, "timezone")
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    name.and_then(|n| n.parse().ok()).unwrap_or(chrono_tz::UTC)
}

/// The current mode and caps.
pub async fn current(state: &AppState) -> (ModeStatus, Caps, Caps) {
    let settings = crate::share_settings::load(state).await;
    let tz = timezone(state).await;
    evaluate(&settings, &Utc::now().with_timezone(&tz))
}

/// Why an upload cannot go ahead right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Paused by the schedule or the manual switch; `until` is unix seconds when known.
    Paused { until: Option<i64> },
    /// Already serving the maximum number of importers.
    Busy,
}

/// Importers seen recently, per group, to enforce the concurrent-importer cap.
static ACTIVE: OnceLock<Mutex<HashMap<(u8, String), Instant>>> = OnceLock::new();
const ACTIVE_WINDOW: Duration = Duration::from_secs(30);

fn limiters() -> &'static RateLimiterMap {
    static MAP: OnceLock<RateLimiterMap> = OnceLock::new();
    MAP.get_or_init(RateLimiterMap::default)
}

/// Admits one upload to `peer` under the group's current caps.
pub async fn admit(state: &AppState, group: Group, peer: &str) -> Result<Caps, Held> {
    let (status, own, help) = current(state).await;
    let caps = match group {
        Group::Own => own,
        Group::Helping => help,
    };
    if caps.rate == Some(0) {
        return Err(Held::Paused {
            until: status.next_switch_at,
        });
    }
    if let Some(max) = caps.max_peers {
        let mut active = ACTIVE
            .get_or_init(Default::default)
            .lock()
            .expect("active lock");
        let tag = group as u8;
        active.retain(|_, seen| seen.elapsed() < ACTIVE_WINDOW);
        let key = (tag, peer.to_string());
        if !active.contains_key(&key)
            && active.keys().filter(|(g, _)| *g == tag).count() as u64 >= max
        {
            return Err(Held::Busy);
        }
        active.insert(key, Instant::now());
    }
    Ok(caps)
}

/// Waits until `len` more bytes may be sent under the group's rate.
pub async fn throttle(group: Group, caps: Caps, len: u64) {
    const SLICE: u64 = 64 * 1024;
    let mut left = len;
    while left > 0 {
        let step = left.min(SLICE);
        limiters().throttle(group.key(), caps.rate, step).await;
        left -= step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share_settings::ScheduleWindow;
    use chrono::TimeZone;

    fn schedule(windows: &[(&str, &str)], days: &[u8]) -> AltSchedule {
        AltSchedule {
            enabled: true,
            windows: windows
                .iter()
                .map(|(f, t)| ScheduleWindow {
                    from: f.to_string(),
                    to: t.to_string(),
                })
                .collect(),
            days: days.to_vec(),
        }
    }

    fn at(day: u32, h: u32, m: u32) -> DateTime<Utc> {
        // 2026-10-19 is a Monday.
        Utc.with_ymd_and_hms(2026, 10, 19 + day, h, m, 0).unwrap()
    }

    #[test]
    fn windows_within_a_day() {
        let s = schedule(&[("01:00", "08:00")], &[]);
        assert!(!schedule_active(&s, &at(0, 0, 59)));
        assert!(schedule_active(&s, &at(0, 1, 0)));
        assert!(schedule_active(&s, &at(0, 7, 59)));
        assert!(!schedule_active(&s, &at(0, 8, 0)));
    }

    #[test]
    fn windows_crossing_midnight_follow_their_start_day() {
        // Friday 22:00 → Saturday 06:00 only.
        let s = schedule(&[("22:00", "06:00")], &[5]);
        assert!(schedule_active(&s, &at(4, 23, 0)));
        assert!(schedule_active(&s, &at(5, 5, 59)));
        assert!(!schedule_active(&s, &at(5, 23, 0)));
        assert!(!schedule_active(&s, &at(3, 23, 0)));
    }

    #[test]
    fn disabled_schedules_never_apply() {
        let mut s = schedule(&[("00:00", "23:59")], &[]);
        s.enabled = false;
        assert!(!schedule_active(&s, &at(0, 12, 0)));
        assert_eq!(next_switch(&s, &at(0, 12, 0)), None);
    }

    #[test]
    fn the_next_switch_is_the_next_boundary() {
        let s = schedule(&[("01:00", "08:00")], &[]);
        assert_eq!(next_switch(&s, &at(0, 0, 30)), Some(at(0, 1, 0)));
        assert_eq!(next_switch(&s, &at(0, 3, 0)), Some(at(0, 8, 0)));
        assert_eq!(next_switch(&s, &at(0, 23, 0)), Some(at(1, 1, 0)));
    }
}
