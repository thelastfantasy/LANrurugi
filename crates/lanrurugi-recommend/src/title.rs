//! What a raw archive title *means*, versus how the work was packaged.
//!
//! The embedding path, the LLM rerank prompt and Tankoubon grouping all need the same split: a
//! title carries a creator block (`[作者/社团]`), the work itself (often with a `～副標題～`), and
//! any number of **release** blocks (`[中国翻訳]`, `【完全版】`, `[DL版]`) that describe an edition
//! rather than the work. Release blocks are noise for "is this the same series" — unrelated works
//! share them constantly — while a subtitle is the opposite: it is what tells two volumes of the
//! same series apart, so it must survive verbatim.
//!
//! Two levels, deliberately separate:
//!
//! - **Character folding** ([`fold_width`]) only rewrites *how* a character is encoded: NFKC for
//!   full/half-width, then the three wave dashes (`～` U+FF5E, `〜` U+301C, `〰` U+3030) onto one
//!   form. It never deletes anything, so a `～…～` subtitle keeps its delimiters and its words.
//! - **Block stripping** ([`split_title`]) removes whole bracketed blocks whose *text* is release
//!   metadata, decided by vocabulary ([`is_release_token`]) rather than by bracket shape: `【誤】`
//!   style brackets are just as likely to hold a subtitle or a publisher as an edition note.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Bumped whenever the rules here change what they output for an unchanged input. A persisted
/// embedding was computed from this module's output, so a change must force a re-embed — the
/// reuse checks in `recommend_precompute` compare *raw* titles, which a rule change never
/// touches (`recommend_precompute`'s own normalizer-version check is what acts on this).
pub const RULE_REVISION: u32 = 1;

/// Substrings that make a block (or a grouping token) purely release metadata. Substring — not
/// whole-token — matching is safe here because every entry is a compound that only occurs in a
/// release context; the bare-word cases live in [`RELEASE_EXACT`].
const RELEASE_SUBSTRINGS: &[&str] = &[
    // Translation / scanlation.
    "翻訳",
    "翻譯",
    "翻译",
    "汉化",
    "漢化",
    "嵌字",
    "scanlat",
    "英訳",
    "机翻",
    "機翻",
    "中国語",
    "中文",
    // Censorship.
    "無修正",
    "无修正",
    "無修",
    "无修",
    "修正",
    // Edition / distribution.
    "dl版",
    "単行本",
    "分冊",
    "合本",
    "完全版",
    "愛蔵版",
    "新装版",
    "特装版",
    "限定版",
    "特典",
    "小冊子",
    "リーフレット",
    "電子版",
    "电子版",
    "デジタル",
    "digital",
    "成年コミック",
    "同人誌",
    "同人志",
    "アンソロジー",
    // Colour / quality.
    "フルカラー",
    "カラー化",
    "高画質",
];

/// Bare words that are release markers only as a whole block/token (`[DL]`, `[English]`) — as a
/// substring they would match ordinary title words.
const RELEASE_EXACT: &[&str] = &[
    "dl",
    "hd",
    "digital",
    "english",
    "chinese",
    "korean",
    "translated",
    "uncensored",
    "decensored",
    "colorized",
    "full color",
    "カラー",
    "英語",
    "中文",
    "中国語",
    "anthology",
];

/// A `～subtitle～` pair, a bare bracket block, and the three wave-dash spellings all fold here.
/// NFKC already maps `～` (U+FF5E) onto ASCII `~`; U+301C and U+3030 are not compatibility
/// characters and survive it, so the unification has to happen *after* NFKC (mapping the
/// full-width form before it would have been undone by NFKC either way).
static HANDLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(@[^)]+\)$").unwrap());
static FRAME_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-f\d+-\d+_n\d+$").unwrap());

/// The parts of one title, separated by what they say about the work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleParts {
    /// The leading `[作者/社团]`-style block, removed from [`Self::content`]: series membership
    /// is decided on the work's own title, and a same-creator boost is a separate ranking
    /// concern. Not currently fed to the LLM prompt (the heuristic can mislabel a leading source
    /// tag like `[Patreon]` as a creator, and the content text is what the model needs).
    pub creators: Vec<String>,
    /// The work itself: release blocks removed, subtitles and everything else intact.
    pub content: String,
    /// Release/edition blocks in their original order, for the LLM prompt and diagnostics.
    pub release: Vec<String>,
}

/// Whether one grouping token or one bracketed block is release metadata rather than content.
pub fn is_release_token(token: &str) -> bool {
    let lower = token.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    RELEASE_SUBSTRINGS.iter().any(|m| lower.contains(m))
        || RELEASE_EXACT.iter().any(|m| lower == *m)
        || is_airing_date(&lower)
        || is_page_or_size_marker(&lower)
}

/// `【260529】` — one exporter's airing-date tag. Deliberately only 6+ digits: `【2】` is a volume
/// number and must stay in the title, where it is exactly what tells two volumes apart.
fn is_airing_date(lower: &str) -> bool {
    lower.len() >= 6 && lower.chars().all(|c| c.is_ascii_digit())
}

/// "24p", "120 pages", "1.2gb" — counts, not titles.
fn is_page_or_size_marker(lower: &str) -> bool {
    let digits = lower.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &lower[digits.len()..];
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && matches!(unit, "p" | "pages" | "page" | "mb" | "gb" | "kb")
}

/// NFKC width folding plus wave-dash unification. Pure rewrite: nothing is removed.
pub fn fold_width(input: &str) -> String {
    input
        .nfkc()
        .map(|c| match c {
            '\u{301C}' | '\u{3030}' => '~',
            _ => c,
        })
        .collect()
}

/// The embedding/prompt text for a raw title — [`TitleParts::content`].
pub fn normalize_title(title: &str) -> String {
    split_title(title).content
}

/// Splits a raw title into creator / content / release parts.
pub fn split_title(raw: &str) -> TitleParts {
    let folded = fold_width(raw.trim());
    // One exporter's own prefix (`jellyfin-frames-`) plus its frame-id suffix carry no meaning.
    let folded = folded
        .strip_prefix("jellyfin-frames-")
        .unwrap_or(&folded)
        .to_string();

    let mut parts = TitleParts::default();
    let mut content = String::new();
    // Whether any real text (or kept block) has been emitted yet — a leading block that is not
    // release metadata is the conventional `[作者/社团]` prefix only while nothing precedes it.
    let mut seen_content = false;
    let mut creator_taken = false;

    for seg in segment(&folded) {
        match seg {
            Seg::Text(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                seen_content = true;
                append(&mut content, &text);
            }
            Seg::Block(inner) => {
                if is_release_token(&inner) {
                    let block = collapse(&inner);
                    if !block.is_empty() {
                        parts.release.push(block);
                    }
                } else if !seen_content && !creator_taken {
                    parts.creators.push(collapse(&inner));
                    creator_taken = true;
                } else {
                    seen_content = true;
                    append(&mut content, &inner);
                }
            }
        }
    }

    let content = HANDLE_RE.replace(&content, "");
    let content = FRAME_ID_RE.replace(&content, "");
    parts.content = collapse(&content);
    parts
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append(out: &mut String, piece: &str) {
    let piece = piece.trim();
    if piece.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(piece);
}

enum Seg {
    Text(String),
    Block(String),
}

/// Splits on `[...]` and `【...】` only. Parentheses stay inside their surrounding text: half-width
/// `(2)` is a volume marker worth keeping, and the `(しゃょー)` romanisation inside a creator block
/// is part of that block. An unclosed opening bracket degrades to plain text.
fn segment(folded: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < folded.len() {
        let c = folded[i..].chars().next().unwrap();
        let close = match c {
            '[' => Some(']'),
            '【' => Some('】'),
            _ => None,
        };
        if let Some(close) = close {
            let after_open = i + c.len_utf8();
            if let Some(rel) = folded[after_open..].find(close) {
                let inner_end = after_open + rel;
                if !buf.is_empty() {
                    out.push(Seg::Text(std::mem::take(&mut buf)));
                }
                out.push(Seg::Block(folded[after_open..inner_end].to_string()));
                i = inner_end + close.len_utf8();
                continue;
            }
        }
        buf.push(c);
        i += c.len_utf8();
    }
    if !buf.is_empty() {
        out.push(Seg::Text(buf));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_survive_and_creator_prefix_does_not() {
        // The `～…～` subtitle is content, not a release marker: it must come through untouched.
        let parts =
            split_title("[陸の孤島亭 (しゃょー)] 桜春女学院の男優3 [中国翻訳] [無修正] [DL版]");
        assert_eq!(parts.content, "桜春女学院の男優3");
        assert_eq!(parts.creators, vec!["陸の孤島亭 (しゃょー)"]);
        assert_eq!(parts.release, vec!["中国翻訳", "無修正", "DL版"]);

        // The content is the *folded* form (`～` → `~`), which is what gets embedded and shown to
        // the rerank model; the delimiters and the subtitle's words are all still there.
        let parts = split_title("[寒天] サキュバシスターズ ～聖職姉妹の背徳～");
        assert_eq!(parts.content, "サキュバシスターズ ~聖職姉妹の背徳~");
        assert_eq!(parts.creators, vec!["寒天"]);
        assert!(parts.release.is_empty());
    }

    #[test]
    fn wave_dash_spellings_fold_onto_one_form() {
        let a = normalize_title("サキュバシスターズ ～聖職姉妹の背徳～");
        let b = normalize_title("サキュバシスターズ 〜聖職姉妹の背徳〜");
        let c = normalize_title("サキュバシスターズ ~聖職姉妹の背徳~");
        assert_eq!(a, b);
        assert_eq!(a, c);

        // An in-word elongation is a different word, not a delimiter: it must not be deleted,
        // only unified.
        assert_eq!(
            normalize_title("み〜んなオナホにしてあげる"),
            "み~んなオナホにしてあげる"
        );
    }

    #[test]
    fn full_width_bracket_blocks_are_classified_not_blindly_stripped() {
        // Edition/quality notes go; a publisher block or a promotional word stays.
        assert_eq!(split_title("蝶子 【完全版】 [DL版]").content, "蝶子");
        assert_eq!(
            split_title("【緊急】現役SCちゃんの日常がヤバすぎる").content,
            "現役SCちゃんの日常がヤバすぎる"
        );
        let jelly = split_title(
            "jellyfin-frames-【260529】【ピンクパイナップル】レイカは華麗な僕の女王 第4巻-f14969-15202_n234",
        );
        assert_eq!(jelly.content, "レイカは華麗な僕の女王 第4巻");
        assert!(jelly.release.contains(&"260529".to_string()));
    }

    #[test]
    fn full_width_punctuation_folds_to_half_width() {
        assert_eq!(
            normalize_title("サグラレ堕メ－快楽堕ち少女－【フルカラー】"),
            "サグラレ堕メ-快楽堕ち少女-"
        );
        // Full-width space collapses like any other whitespace.
        assert_eq!(normalize_title("ギャル　せん2"), "ギャル せん2");
    }

    #[test]
    fn volume_numbers_in_full_width_brackets_are_kept() {
        // `【2】` is a volume, not an airing date: dropping it would merge two volumes into one
        // identical embedding.
        assert_eq!(split_title("作品【2】").content, "作品 2");
        assert_ne!(normalize_title("作品【2】"), normalize_title("作品【3】"));
        // Six digits is the exporter's date tag, where dropping it is the point.
        assert_eq!(split_title("作品【260529】").content, "作品");
    }

    #[test]
    fn volume_markers_in_parentheses_are_not_mistaken_for_brackets() {
        assert_eq!(
            split_title("ギャルとガリ勉～入り浸り勉強会～【R-単行本版／限定特典つき】(2)").content,
            "ギャルとガリ勉~入り浸り勉強会~ (2)"
        );
    }

    #[test]
    fn release_vocabulary_covers_grouping_tokens() {
        for token in [
            "中国翻訳",
            "DL版",
            "無修正",
            "English",
            "translated",
            "24p",
            "完全版",
            "限定特典つき",
        ] {
            assert!(
                is_release_token(token),
                "{token} should be a release marker"
            );
        }
        for token in ["桜春女学院の男優3", "ギャルせん2", "背徳", "緊急"] {
            assert!(
                !is_release_token(token),
                "{token} should not be a release marker"
            );
        }
    }
}
