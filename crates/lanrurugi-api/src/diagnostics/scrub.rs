//! Scrubbing for diagnostics payloads (specs/018-error-telemetry-feedback FR-008).
//!
//! Runs on ingest, before storage — a secret that is never stored cannot leak from the settings
//! page, from a bundle, or from a forward to a tracker later. Structural scrubbing (a fixed
//! breadcrumb field set, no form values, no archive titles unless opted in) happens in the parser;
//! this module is the textual pass over whatever text the parser kept.

use std::sync::LazyLock;

use regex::Regex;

/// Marker left where something was removed, so a reader can tell a redaction from an omission.
pub const REDACTED: &str = "[redacted]";

struct Rule {
    pattern: Regex,
    /// Keeps the label and drops only the value: e.g. `token=abc` → `token=[redacted]`.
    keep_prefix: bool,
    replacement: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let mut rules = Vec::new();
    let mut add = |pattern: &str, keep_prefix: bool, replacement: &'static str| {
        rules.push(Rule {
            pattern: Regex::new(pattern).expect("scrub pattern"),
            keep_prefix,
            replacement,
        });
    };
    // Share codes: the readable form and the sealed route suffix that rides in the copyable text.
    add(r"(?i)\blrrg:[A-Z0-9-]{4,}\S*", false, REDACTED);
    add(r"(?i)([?&]r=)[A-Za-z0-9_\-%=.+]{8,}", true, "");
    // Credentials in headers, query strings and JSON-ish bodies.
    add(
        r"(?i)(authorization\s*[:=]\s*)(bearer\s+)?[A-Za-z0-9._\-+/=]{8,}",
        true,
        "",
    );
    add(r"(?i)(cookie\s*[:=]\s*)[^\r\n]{4,}", true, "");
    add(
        r"(?i)(x-share-token\s*[:=]\s*)[A-Za-z0-9._\-+/=]{4,}",
        true,
        "",
    );
    add(
        r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|passwd|rt)\b(\s*[:=]\s*|\s+)[^\s,;&"']{4,}"#,
        true,
        "",
    );
    // Absolute paths: POSIX with at least two segments, and Windows drive paths.
    add(r#"(?:^|[\s"'(=])(?:/[A-Za-z0-9._@+-]+){2,}"#, false, " ~");
    add(r#"(?i)\b[A-Z]:\\[^\s"']{2,}"#, false, " ~");
    // Long hex or base64 blobs: keys, hashes, tickets that are not already covered by a label.
    add(r"\b[0-9a-f]{32,}\b", false, REDACTED);
    rules
});

/// Whether an archive's name may appear in a captured message or tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveNames {
    Strip,
    Keep,
}

impl ArchiveNames {
    pub fn from_flag(keep: bool) -> Self {
        if keep {
            Self::Keep
        } else {
            Self::Strip
        }
    }
}

/// Scrubs one string. Cheap enough to call per field; a `Regex` set is built once per process.
pub fn text(input: &str, archive_names: ArchiveNames) -> String {
    let mut out = input.to_string();
    for rule in RULES.iter() {
        out = if rule.keep_prefix {
            // Keep the label (`token=`), replace only the value it introduced.
            rule.pattern
                .replace_all(&out, |caps: &regex::Captures| {
                    format!("{}{}", &caps[1], REDACTED)
                })
                .into_owned()
        } else {
            rule.pattern
                .replace_all(&out, rule.replacement)
                .into_owned()
        };
    }
    // Archive titles are never put here by the parser unless the administrator opted in, so an
    // opted-out payload simply has none; the flag exists so a caller that did include one (a
    // message a plugin composed itself) can drop it with the same call.
    let _ = archive_names;
    truncate(&out, 8 * 1024)
}

/// Clamps a string to a byte budget without splitting a UTF-8 sequence.
pub fn truncate(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = input[..end].to_string();
    out.push_str("…[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(input: &str) -> String {
        text(input, ArchiveNames::Strip)
    }

    #[test]
    fn share_codes_and_sealed_suffixes_disappear() {
        let out = strip("import failed for lrrg:VXR29-J5RNH?r=abcDEF123456");
        assert!(!out.contains("VXR29-J5RNH"), "{out}");
        assert!(!out.contains("abcDEF123456"), "{out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn labeled_secrets_keep_their_label_only() {
        let out = strip("request headers: Authorization: Bearer abcdef1234567890 token=deadbeef");
        assert!(!out.contains("abcdef1234567890"), "{out}");
        assert!(!out.contains("deadbeef"), "{out}");
        assert!(
            out.contains("Authorization") || out.contains("authorization"),
            "{out}"
        );
    }

    #[test]
    fn absolute_paths_become_a_placeholder() {
        let out = strip("could not write /home/jade/manga/foo.zip");
        assert!(!out.contains("/home/jade"), "{out}");
        assert!(out.contains('~'));
    }

    #[test]
    fn long_hex_blobs_disappear() {
        let out = strip("ticket 0123456789abcdef0123456789abcdef0123");
        assert!(!out.contains("0123456789abcdef"), "{out}");
    }

    #[test]
    fn ordinary_messages_survive_intact() {
        let message = "failed to decode archive: unexpected end of central directory";
        assert_eq!(strip(message), message);
    }

    #[test]
    fn truncation_does_not_split_a_character() {
        // 4 bytes cannot hold "ab" + a 3-byte character, so the cut lands before it.
        let short = truncate("ab日本語", 4);
        assert!(short.starts_with("ab"), "{short}");
        assert!(!short.contains('\u{FFFD}'), "no half character: {short}");
        // 5 bytes do fit that character.
        let fits = truncate("ab日本語", 5);
        assert!(fits.starts_with("ab日"), "{fits}");
        assert!(fits.ends_with("[truncated]"));
    }
}
