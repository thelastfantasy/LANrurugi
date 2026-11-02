//! Release markers (FR-002): title tokens that describe how a work was released — translation,
//! edition, censorship, format, page count — rather than what it is. Unrelated works share them
//! constantly, so they never count as shared title words. The vocabulary is shared with the
//! recommendation embedding's title normalizer; extend it in `lanrurugi_recommend::title`.

use std::collections::HashSet;

/// Whether one title token is a release marker. The vocabulary itself lives next to the title
/// normalizer that also consumes it (`lanrurugi_recommend::title`) — one list, so a word added for
/// the recommendation path (an edition note, a translation group) is immediately a non-signal for
/// grouping too, instead of the two drifting apart.
pub(crate) fn is_marker(token: &str) -> bool {
    lanrurugi_recommend::title::is_release_token(token)
}

/// The token set with every release marker removed.
pub(crate) fn strip(tokens: HashSet<&str>) -> HashSet<&str> {
    tokens.into_iter().filter(|t| !is_marker(t)).collect()
}

/// The title with marker tokens (as split by the grouping's own tokenizer) dropped, for what the
/// reviewer sees. Keeps the original order and spacing of what remains.
pub(crate) fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if !token.trim().is_empty() && !is_marker(token) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(token.trim());
        }
        token.clear();
    };
    for c in title.chars() {
        if matches!(
            c,
            '[' | ']' | '【' | '】' | '(' | ')' | '（' | '）' | '_' | '|' | '｜'
        ) {
            flush(&mut token, &mut out);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_markers_are_removed_and_creator_handles_kept() {
        let tokens: HashSet<&str> = [
            "Creator One",
            "Story",
            "中国翻訳",
            "DL版",
            "無修正",
            "某某汉化组",
            "24P",
            "English",
        ]
        .into_iter()
        .collect();
        let kept = strip(tokens);
        let expected: HashSet<&str> = ["Creator One", "Story"].into_iter().collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn numbers_that_are_not_page_counts_survive() {
        for t in ["2", "Vol.2", "第2話", "1999", "24pieces"] {
            assert!(!is_marker(t), "{t}");
        }
        for t in ["24p", "120pages", "1.2GB"] {
            assert!(is_marker(t), "{t}");
        }
    }

    #[test]
    fn clean_title_drops_bracketed_markers_only() {
        assert_eq!(
            clean_title("[Creator One] Story A [中国翻訳] [DL版]"),
            "Creator One Story A"
        );
    }
}
