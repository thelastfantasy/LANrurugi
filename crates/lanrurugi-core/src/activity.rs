//! Interactive-traffic beacon shared between the HTTP layer and the background scanner.
//!
//! The scanner's first pass over a large library is CPU- and I/O-heavy enough to starve the HTTP
//! handlers it shares the process with: a 1.1 TB library made `/api/info` take ~60 s on a 12-core
//! machine. Nothing in the request path needs to know *what* the scanner is doing — only whether
//! somebody is using the instance right now — so this is a single timestamp, updated by a
//! middleware on every request and read by the scan loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_REQUEST_MS: AtomicU64 = AtomicU64::new(0);

/// Milliseconds since the Unix epoch, or `0` if the clock is before it.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called once per HTTP request by the server's outermost middleware.
pub fn note_request() {
    LAST_REQUEST_MS.store(now_ms(), Ordering::Relaxed);
}

/// When the last request was seen, in the same units as [`now_ms`]. `0` means "nothing yet".
pub fn last_request_ms() -> u64 {
    LAST_REQUEST_MS.load(Ordering::Relaxed)
}

/// Whether an interactive request arrived within `window_ms` of `now`.
pub fn request_within(now: u64, window_ms: u64) -> bool {
    let last = last_request_ms();
    last != 0 && now.saturating_sub(last) < window_ms
}

/// Kernel-reported stall time, as the percentage of the last 10 seconds in which at least one task
/// was stalled on `kind` (`"io"` or `"cpu"`). Linux-only (PSI, kernel ≥ 4.20); `None` elsewhere or
/// when the file is unreadable, which callers treat as "no pressure signal".
///
/// This is the honest answer to "is the machine busy": it is measured by the kernel across every
/// process and container, so a ZFS scrub, a backup or a neighbour's container shows up here, while
/// the request beacon above cannot see any of them.
pub fn pressure_avg10(kind: &str) -> Option<f64> {
    let text = std::fs::read_to_string(format!("/proc/pressure/{kind}")).ok()?;
    let some = text.lines().find(|line| line.starts_with("some"))?;
    let field = some.split_whitespace().find(|f| f.starts_with("avg10="))?;
    field.trim_start_matches("avg10=").parse().ok()
}

/// Whether the machine is under enough CPU or I/O pressure that a background scan should stand
/// aside. Thresholds are percentages of wall time stalled in the last 10 seconds: 10% I/O stall
/// already means something is waiting on the disks, and 25% CPU stall means the scheduler is
/// queueing work.
pub fn under_pressure() -> bool {
    const IO_STALL: f64 = 10.0;
    const CPU_STALL: f64 = 25.0;
    pressure_avg10("io").is_some_and(|v| v > IO_STALL)
        || pressure_avg10("cpu").is_some_and(|v| v > CPU_STALL)
}
