//! Search query grammar parser (research.md §8), verified against
//! `~/LANraragi/lib/LANraragi/Model/Search.pm::compute_search_filter`.
//!
//! Legacy syntax, all preserved here:
//! - Comma-separated tags, each an independent token, ANDed together.
//! - `-` prefix on a tag negates it (must be absent).
//! - `"..."` (or a trailing `$`) marks an exact-tag match instead of a fuzzy substring match.
//! - `?`/`_` become single-character glob wildcards; `*`/`%` become multi-character wildcards.
//! - Tags are lowercased and trimmed.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tag: String,
    pub isneg: bool,
    pub isexact: bool,
}

/// Parses a raw filter string into search tokens. Legacy parses right-to-left (building each
/// token by popping characters off the end); the result is order-independent (tokens are ANDed),
/// so this left-to-right reimplementation produces an equivalent token set.
pub fn compute_search_filter(filter: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = filter.chars().peekable();

    while chars.peek().is_some() {
        while chars.peek() == Some(&',') || chars.peek() == Some(&' ') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let isneg = if chars.peek() == Some(&'-') {
            chars.next();
            true
        } else {
            false
        };

        let (raw, isexact) = if chars.peek() == Some(&'"') {
            chars.next();
            let mut s = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                s.push(c);
            }
            // Optional trailing `$` after the closing quote is accepted but redundant.
            if chars.peek() == Some(&'$') {
                chars.next();
            }
            (s, true)
        } else {
            let mut s = String::new();
            for c in chars.by_ref() {
                if c == ',' {
                    break;
                }
                s.push(c);
            }
            if let Some(stripped) = s.strip_suffix('$') {
                (stripped.to_string(), true)
            } else {
                (s, false)
            }
        };

        let tag = normalize(&raw);
        if !tag.is_empty() {
            tokens.push(Token {
                tag,
                isneg,
                isexact,
            });
        }
    }

    tokens
}

fn normalize(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        match c {
            '_' => out.push('?'),
            '%' => out.push('*'),
            other => out.push(other),
        }
    }
    out.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(tag: &str, isneg: bool, isexact: bool) -> Token {
        Token {
            tag: tag.to_string(),
            isneg,
            isexact,
        }
    }

    #[test]
    fn parses_simple_comma_separated_tags() {
        let tokens = compute_search_filter("artist:jane,adventure");
        assert_eq!(
            tokens,
            vec![
                tok("artist:jane", false, false),
                tok("adventure", false, false)
            ]
        );
    }

    #[test]
    fn negation_prefix() {
        let tokens = compute_search_filter("-artist:jane");
        assert_eq!(tokens, vec![tok("artist:jane", true, false)]);
    }

    #[test]
    fn exact_match_via_quotes() {
        let tokens = compute_search_filter("\"artist:jane\"");
        assert_eq!(tokens, vec![tok("artist:jane", false, true)]);
    }

    #[test]
    fn exact_match_via_trailing_dollar() {
        let tokens = compute_search_filter("artist:jane$");
        assert_eq!(tokens, vec![tok("artist:jane", false, true)]);
    }

    #[test]
    fn wildcard_normalization() {
        let tokens = compute_search_filter("art_st:j%ne");
        assert_eq!(tokens, vec![tok("art?st:j*ne", false, false)]);
    }

    #[test]
    fn lowercases_and_trims() {
        let tokens = compute_search_filter(" Artist:JANE , Adventure ");
        assert_eq!(
            tokens,
            vec![
                tok("artist:jane", false, false),
                tok("adventure", false, false)
            ]
        );
    }

    #[test]
    fn empty_filter_yields_no_tokens() {
        assert_eq!(compute_search_filter(""), Vec::new());
        assert_eq!(compute_search_filter("   "), Vec::new());
    }
}
