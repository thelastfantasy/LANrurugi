//! The one list of release markers (FR-002): title tokens that describe how a work was released —
//! translation, edition, censorship, format, page count — rather than what it is. Unrelated works
//! share them constantly, so they never count as shared title words. Extend the lists here when
//! the evaluation finds another one.

use std::collections::HashSet;

/// Whole tokens that are always release markers (compared case-insensitively).
const MARKERS: &[&str] = &[
    // Translation.
    "中国翻訳",
    "中国翻譯",
    "中國翻譯",
    "中国語",
    "中文",
    "中国翻译",
    "英訳",
    "英語",
    "english",
    "chinese",
    "korean",
    "韓国翻訳",
    "translated",
    "翻訳",
    "機械翻訳",
    "机翻",
    "機翻",
    // Edition / distribution.
    "dl版",
    "dl",
    "digital",
    "電子版",
    "电子版",
    "同人誌",
    "同人志",
    "成年コミック",
    // Censorship.
    "無修正",
    "无修正",
    "無修",
    "无修",
    "uncensored",
    "decensored",
    "修正",
    // Colour / quality.
    "カラー化",
    "colorized",
    "full color",
    "フルカラー",
    "高画質",
    "hd",
];

/// Substrings that make a whole token a marker: a translation group's name ("某某汉化组") says who
/// translated it, not what it is.
const MARKER_SUBSTRINGS: &[&str] = &["汉化", "漢化", "翻訳", "翻譯", "翻译", "嵌字", "scanlat"];

/// Whether one title token is a release marker.
pub(crate) fn is_marker(token: &str) -> bool {
    let lower = token.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if MARKERS.contains(&lower.as_str()) {
        return true;
    }
    if MARKER_SUBSTRINGS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    is_page_or_size_marker(&lower)
}

/// "24p", "32P", "120 pages", "1.2gb" and the like.
fn is_page_or_size_marker(lower: &str) -> bool {
    let digits = lower.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &lower[digits.len()..];
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && matches!(unit, "p" | "pages" | "page" | "mb" | "gb" | "kb")
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
