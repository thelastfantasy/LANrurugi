//! Grouping (specs/018-error-telemetry-feedback FR-003 / research.md §4).
//!
//! Same input, same issue, forever: the fingerprint is a hash of the error's type, its top user
//! frame and a normalized message. Normalization is what keeps "failed after 12 retries" and
//! "failed after 13 retries" in one issue.

use std::sync::LazyLock;

use lanrurugi_storage::diagnostics::{DiagnosticEvent, Fingerprint, Level};
use regex::Regex;

static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-fA-F]{8,}\b").unwrap());
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b")
        .unwrap()
});
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]{0,200}""#).unwrap());
static NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4,}\b").unwrap());
static PATHS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:/[\w.@+-]+){2,}|[A-Za-z]:\\[^\s]+").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// The message with the parts that vary between occurrences replaced by placeholders.
pub fn normalize(message: &str) -> String {
    let mut out = message.lines().next().unwrap_or("").to_string();
    out = UUID.replace_all(&out, "<uuid>").into_owned();
    out = HEX.replace_all(&out, "<hex>").into_owned();
    out = QUOTED.replace_all(&out, "<str>").into_owned();
    out = NUMBERS.replace_all(&out, "<n>").into_owned();
    out = PATHS.replace_all(&out, "<path>").into_owned();
    SPACES.replace_all(&out, " ").trim().to_string()
}

/// The top frame that is ours rather than a library's: the first `file:line` that does not look
/// like a vendored dependency or an anonymous eval.
pub fn top_frame(stack: Option<&str>) -> Option<String> {
    let stack = stack?;
    for line in stack.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("node_modules")
            || lower.contains("/vendor/")
            || lower.contains("(native)")
            || lower.contains("eval at")
        {
            continue;
        }
        if let Some(frame) = line.split('(').nth(1) {
            return Some(frame.trim_end_matches(')').to_string());
        }
        if line.contains(':') && !line.starts_with("at ") {
            return Some(line.to_string());
        }
    }
    None
}

/// The grouping key for an event. An event that carried its own fingerprint keeps it, so a report
/// groups the same way here and on a tracker.
pub fn fingerprint(event: &DiagnosticEvent) -> Fingerprint {
    if let Some(supplied) = event.tags.get("fingerprint") {
        if !supplied.trim().is_empty() {
            return Fingerprint(supplied.trim().to_string());
        }
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(event.source.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(event.message.lines().next().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(normalize(&event.message).as_bytes());
    hasher.update(b"\0");
    if let Some(frame) = top_frame(event.stack.as_deref()) {
        hasher.update(frame.as_bytes());
    }
    let digest = hasher.finalize().to_hex().to_string();
    Fingerprint(digest[..32].to_string())
}

/// The most frequent level among a set, so an issue's badge reflects its worst occurrence.
pub fn worst(levels: impl IntoIterator<Item = Level>) -> Level {
    levels
        .into_iter()
        .max_by_key(|level| level.rank())
        .unwrap_or(Level::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::diagnostics::{DiagnosticEventId, Source};

    fn event(message: &str, stack: Option<&str>) -> DiagnosticEvent {
        DiagnosticEvent {
            id: DiagnosticEventId::generate(),
            fingerprint: Fingerprint("pending".into()),
            at: 0,
            client_at: None,
            level: Level::Error,
            source: Source::Server,
            message: message.to_string(),
            stack: stack.map(str::to_string),
            release: None,
            platform: None,
            request_id: None,
            tags: Default::default(),
            breadcrumbs: Vec::new(),
            envelope: Vec::new(),
            bytes: 0,
        }
    }

    #[test]
    fn volatile_parts_of_a_message_normalize_away() {
        let a = normalize("failed for archive 4f2a1b9c8d7e6f50 at /library/a.zip");
        let b = normalize("failed for archive 9c1b3d5e7f2a4b60 at /library/b.zip");
        assert_eq!(a, b, "different ids must group together: {a} vs {b}");
        assert!(a.contains("<hex>"), "the id should be a placeholder: {a}");
    }

    #[test]
    fn small_numbers_are_signal_not_noise() {
        // "2 retries" and "9 retries" are different bugs; only long, generated-looking numbers are
        // collapsed.
        assert_ne!(
            normalize("gave up after 2 retries"),
            normalize("gave up after 9 retries")
        );
    }

    #[test]
    fn the_same_failure_shares_a_fingerprint() {
        let a = event(
            "cannot decode archive: bad header",
            Some("at scan (src/scan.rs:42:7)"),
        );
        let b = event(
            "cannot decode archive: bad header",
            Some("at scan (src/scan.rs:42:7)"),
        );
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn a_different_location_is_a_different_issue() {
        let a = event("boom", Some("at one (src/one.rs:1:1)"));
        let b = event("boom", Some("at two (src/two.rs:1:1)"));
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn a_supplied_fingerprint_wins() {
        let mut e = event("boom", None);
        e.tags.insert("fingerprint".into(), "custom-group".into());
        assert_eq!(fingerprint(&e).as_str(), "custom-group");
    }

    #[test]
    fn the_top_frame_skips_library_frames() {
        let stack = "at render (node_modules/react/index.js:10:2)\nat scan (src/scan.rs:42:7)";
        assert_eq!(top_frame(Some(stack)).as_deref(), Some("src/scan.rs:42:7"));
    }
}
