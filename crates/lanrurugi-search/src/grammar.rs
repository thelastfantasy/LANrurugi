//! Search query grammar parser (research.md §8), originally verified against
//! `~/LANraragi/lib/LANraragi/Model/Search.pm::compute_search_filter`.
//!
//! Legacy syntax (mostly preserved here — see the one deliberate deviation below):
//! - Comma- **or space**-separated tags, each an independent token, ANDed together. Legacy only
//!   treats comma as a real delimiter (a bare space inside an unquoted token is preserved as part
//!   of that token's own text, matching how most tag values here are naturally multi-word, e.g.
//!   `female:huge breasts`) — but that also means legacy has no way to AND together two *separate*
//!   `namespace:value` terms without a comma, which real users don't reach for (issue #59: `female:
//!   huge breasts female:milf` typed with a plain space, the natural way, silently became one
//!   single nonsense token instead of two ANDed ones). Adopts e-hentai's own real search-box
//!   convention instead (`f_search=female:"double+penetration$"+female:"ryona"` — verified against
//!   a real e-hentai search): space is a real delimiter just like comma, and a multi-word tag
//!   *value* must be quoted to protect its internal spaces from being split. Every call site that
//!   *generates* a `namespace:value` search string (tag-click links, autocomplete insertion) quotes
//!   a multi-word value automatically, so this only becomes the user's own problem for a raw,
//!   hand-typed, unquoted multi-word query — which used to work and now doesn't; a real, deliberate
//!   parity break, not a silent one, and called out in-product wherever a predicate/search field's
//!   own help text explains the syntax.
//! - `-` prefix on a tag negates it (must be absent).
//! - `"..."` (or a trailing `$`) marks an exact-tag match instead of a fuzzy substring match — the
//!   quotes can wrap either the *whole* token (`"female:anal intercourse"`) or, matching
//!   e-hentai's own literal syntax, just the value half of a namespaced tag
//!   (`female:"anal intercourse"`, colon outside the quotes) — both spellings produce the exact
//!   same token.
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
            let mut isexact = false;
            loop {
                match chars.peek() {
                    None | Some(',') | Some(' ') => break,
                    // `namespace:"value with spaces"` — matches e-hentai's own real search-box
                    // syntax (`female:"double+penetration$"`), where only the *value* half is
                    // quoted, not the whole `namespace:value` pair. The colon itself stays outside
                    // the quotes in the raw input but ends up as a normal character in `s` either
                    // way, so the resulting token is identical either way (`female:anal
                    // intercourse` whether written as `female:"anal intercourse"` or the
                    // whole-token `"female:anal intercourse"` form the top-level quote branch
                    // above already handles) — this is purely an *additional accepted spelling*,
                    // not a new distinct semantic.
                    Some(':') => {
                        s.push(':');
                        chars.next();
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            for c in chars.by_ref() {
                                if c == '"' {
                                    break;
                                }
                                s.push(c);
                            }
                            isexact = true;
                            // Optional trailing `$` after the closing quote is accepted but
                            // redundant, same as the top-level quote branch above.
                            if chars.peek() == Some(&'$') {
                                chars.next();
                            }
                            break;
                        }
                    }
                    Some(&c) => {
                        s.push(c);
                        chars.next();
                    }
                }
            }
            if isexact {
                (s, true)
            } else if let Some(stripped) = s.strip_suffix('$') {
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

    // Issue #59: a plain space between two distinct `namespace:value` terms, typed the way a user
    // naturally would (no comma), used to silently collapse into one nonsense token that matched
    // nothing — verified live via `female:huge breasts female:milf` returning 0 results despite
    // each half independently matching 1. Space is now a real delimiter, same as comma.
    #[test]
    fn space_separates_tokens_like_comma() {
        let tokens = compute_search_filter("female:milf language:chinese");
        assert_eq!(
            tokens,
            vec![
                tok("female:milf", false, false),
                tok("language:chinese", false, false)
            ]
        );
    }

    #[test]
    fn bare_keywords_space_separated() {
        // Synthetic (non-real-title) CJK text — this test only cares that a bare space between
        // two multi-byte tokens splits them, not about any specific real archive's content.
        let tokens = compute_search_filter("さくら まぼろしの物語");
        assert_eq!(
            tokens,
            vec![
                tok("さくら", false, false),
                tok("まぼろしの物語", false, false)
            ]
        );
    }

    // The other half of the fix: a multi-word tag *value* (still a real, common shape —
    // `female:huge breasts` is one tag, not two) must still be searchable as a single token now
    // that a bare space would otherwise split it. Quoting the *whole* token still works (top-level
    // quote branch, unchanged) and still ANDs correctly with a following unquoted token.
    #[test]
    fn quoted_whole_token_preserves_internal_space_and_still_ands_with_the_next_token() {
        let tokens = compute_search_filter("\"female:huge breasts\" female:milf");
        assert_eq!(
            tokens,
            vec![
                tok("female:huge breasts", false, true),
                tok("female:milf", false, false)
            ]
        );
    }

    // e-hentai's own real search-box syntax quotes only the *value* half, colon outside the
    // quotes (`female:"double+penetration$"`) — now also accepted, and produces the identical
    // token either way (not a different, narrower kind of match).
    #[test]
    fn quoted_value_only_form_matches_quoted_whole_token_form_exactly() {
        assert_eq!(
            compute_search_filter("female:\"anal intercourse\""),
            compute_search_filter("\"female:anal intercourse\""),
        );
        assert_eq!(
            compute_search_filter("female:\"anal intercourse\""),
            vec![tok("female:anal intercourse", false, true)]
        );
    }

    // Negation (`-` prefix) is parsed once per token, before either the quoted or unquoted branch
    // — unaffected by the space-delimiter/mid-token-quote additions above, but worth locking down
    // explicitly now that a token can be built two different ways.
    #[test]
    fn negation_combines_with_space_separated_tokens() {
        let tokens = compute_search_filter("language:chinese -female:milf");
        assert_eq!(
            tokens,
            vec![
                tok("language:chinese", false, false),
                tok("female:milf", true, false)
            ]
        );
    }

    #[test]
    fn negation_combines_with_the_value_only_quote_form() {
        let tokens = compute_search_filter("-female:\"huge breasts\"");
        assert_eq!(tokens, vec![tok("female:huge breasts", true, true)]);
    }

    #[test]
    fn negation_combines_with_a_bare_space_separated_keyword() {
        // Synthetic CJK text, same reasoning as `bare_keywords_space_separated` above.
        let tokens = compute_search_filter("-さくら まぼろしの物語");
        assert_eq!(
            tokens,
            vec![
                tok("さくら", true, false),
                tok("まぼろしの物語", false, false)
            ]
        );
    }
}
