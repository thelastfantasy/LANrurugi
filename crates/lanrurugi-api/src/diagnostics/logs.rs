//! The timestamped log reader the diagnostics views and the bundle builder share
//! (specs/018-error-telemetry-feedback FR-012/FR-014).
//!
//! The server's log lines start with an RFC3339 timestamp; a stack trace or a multi-line value does
//! not. Continuation lines are therefore attached to the entry above them — filtering or slicing by
//! time must never split an error from its trace.

use std::path::Path;

/// One timestamped entry: its instant, its source line number, and every line it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: i64,
    pub line: usize,
    pub text: String,
    pub level: Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Info,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// Parses one log file into entries, in file order.
pub fn parse(contents: &str) -> Vec<LogEntry> {
    let mut entries: Vec<LogEntry> = Vec::new();
    for (index, raw) in contents.lines().enumerate() {
        let line = index + 1;
        match parse_timestamp(raw) {
            Some(at) => {
                let level = parse_level(raw);
                entries.push(LogEntry {
                    at,
                    line,
                    text: raw.to_string(),
                    level,
                });
            }
            None => {
                // A continuation belongs to the entry above it; a file that starts mid-trace keeps
                // the orphan rather than dropping it.
                match entries.last_mut() {
                    Some(last) => {
                        last.text.push('\n');
                        last.text.push_str(raw);
                    }
                    None => entries.push(LogEntry {
                        at: 0,
                        line,
                        text: raw.to_string(),
                        level: Level::Info,
                    }),
                }
            }
        }
    }
    entries
}

/// Reads and parses one category's file. A missing file is an empty log, not an error: an instance
/// may run without file logging.
pub async fn read(dir: &Path, category: &str) -> Vec<LogEntry> {
    let path = dir.join(format!("{category}.log"));
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => parse(&contents),
        Err(_) => Vec::new(),
    }
}

/// The entries that overlap `from..=to`, with every owned continuation line included.
pub fn slice(entries: &[LogEntry], from: i64, to: i64) -> Vec<&LogEntry> {
    entries
        .iter()
        .filter(|entry| entry.at == 0 || (entry.at >= from && entry.at <= to))
        .collect()
}

/// The entries around a request id, used by an issue's log excerpt.
pub fn around_request<'a>(
    entries: &'a [LogEntry],
    request_id: &str,
    window: usize,
) -> Vec<&'a LogEntry> {
    let Some(hit) = entries.iter().position(|e| e.text.contains(request_id)) else {
        return Vec::new();
    };
    let start = hit.saturating_sub(window);
    let end = (hit + window + 1).min(entries.len());
    entries[start..end].iter().collect()
}

/// `2026-10-28T03:08:21.123456Z` (tracing's default) or with a numeric offset.
fn parse_timestamp(line: &str) -> Option<i64> {
    let candidate = line.split_whitespace().next()?;
    if candidate.len() < 19 {
        return None;
    }
    let date = &candidate[..10];
    let time = &candidate[11..19];
    if !date.contains('-') || !time.contains(':') {
        return None;
    }
    let days = days_from_civil(
        date[..4].parse().ok()?,
        date[5..7].parse().ok()?,
        date[8..10].parse().ok()?,
    );
    let secs = time[..2].parse::<i64>().ok()? * 3600
        + time[3..5].parse::<i64>().ok()? * 60
        + time[6..8].parse::<i64>().ok()?;
    let millis = candidate
        .find('.')
        .and_then(|dot| candidate.get(dot + 1..dot + 4))
        .and_then(|fraction| fraction.parse::<i64>().ok())
        .unwrap_or(0);
    Some(days * 86_400_000 + secs * 1000 + millis)
}

fn parse_level(line: &str) -> Level {
    let upper = line.to_ascii_uppercase();
    if upper.contains(" ERROR ") || upper.starts_with("ERROR") || upper.contains("] ERROR") {
        Level::Error
    } else if upper.contains(" WARN ") || upper.starts_with("WARN") || upper.contains("] WARN") {
        Level::Warning
    } else {
        Level::Info
    }
}

/// Days since the epoch for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
2026-10-28T03:08:21.100000Z  INFO lanrurugi_server: starting
2026-10-28T03:08:22.200000Z  WARN lanrurugi_api::share: peer refused
2026-10-28T03:08:23.300000Z ERROR lanrurugi_api::share: import failed
    at share_net::import (src/share_net/import.rs:42:7)
2026-10-28T03:08:24.400000Z  INFO lanrurugi_server: done
";

    #[test]
    fn continuation_lines_stay_with_their_entry() {
        let entries = parse(SAMPLE);
        assert_eq!(entries.len(), 4);
        let failed = entries
            .iter()
            .find(|entry| entry.text.contains("import failed"))
            .expect("the error entry");
        assert!(failed.text.contains("share_net::import"), "{}", failed.text);
        assert_eq!(failed.level, Level::Error);
    }

    #[test]
    fn entries_carry_a_real_instant() {
        let entries = parse(SAMPLE);
        let first = entries.first().expect("an entry");
        // 2026-10-28T03:08:21.100Z
        assert_eq!(first.at, 1_793_156_901_100);
    }

    #[test]
    fn slicing_keeps_whole_entries() {
        let entries = parse(SAMPLE);
        let picked = slice(&entries, entries[1].at, entries[2].at);
        assert_eq!(picked.len(), 2);
        assert!(picked[1].text.contains("at share_net::import"));
    }

    #[test]
    fn an_excerpt_looks_around_a_request_id() {
        let entries = parse(SAMPLE);
        assert!(around_request(&entries, "share_net::import", 1).len() >= 2);
        assert!(around_request(&entries, "no-such-id", 1).is_empty());
    }
}
