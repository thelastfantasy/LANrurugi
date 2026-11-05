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
