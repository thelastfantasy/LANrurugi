//! Search query grammar: parses a filter string into an [`Expr`] tree, originally verified against
//! `~/LANraragi/lib/LANraragi/Model/Search.pm::compute_search_filter` for the flat subset it shares
//! with legacy.
//!
//! Legacy syntax (preserved here — see the one deliberate deviation below):
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
//!   same token. On the *title* side the same token means "this phrase appears anywhere in the
//!   title" (see `engine.rs::token_matches` — deliberately looser than legacy's whole-title
//!   equality, which made quoting useless for the title-phrase lookup it looks like it serves).
//! - `?`/`_` become single-character glob wildcards; `*`/`%` become multi-character wildcards.
//! - Tags are lowercased and trimmed.
//! - Inside a quoted value, `\"` is a literal quote and `\\` a literal backslash — a tag value
//!   that itself contains a `"` (a real, if rare, possibility in a scraped artist/circle name) has
//!   no other way to be searched at all otherwise: unescaped, the first `"` inside the value would
//!   be read as the *closing* quote, silently truncating the value and leaving the remainder to be
//!   parsed as an unrelated bare token.
//!
//! Beyond legacy, this module also parses a real boolean structure — `|` for OR, `(`/`)` and
//! `{`/`}` for grouping, with `-` negating whatever atom (term or group) follows it, and implicit
//! AND binding tighter than `|`. Two consequences worth stating plainly, both deliberate and
//! recorded in `specs/001-lanrurugi-full-rewrite/spec.md`'s Assumptions:
//!
//! 1. `|`, `(`, `)`, `{`, `}` are **structural outside quotes**, so a tag value containing one of
//!    those characters must now be quoted (`group:"serious graphics (ice)"`). Legacy treated them
//!    as ordinary text.
//! 2. `OR`/`AND` are deliberately **not** keywords: `or` is a perfectly plausible thing to search
//!    for (it is a two-letter substring of a great many tags), and silently turning it into an
//!    operator would change results for a query that means something today.

/// Which half of an archive's searchable text a token is allowed to match. Defaults to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Tags and title — legacy's own behavior for a bare token.
    Any,
    /// Title only (`title:` / `filename:` prefixes) — the one thing legacy's syntax had no way to
    /// express at all, since a bare token always searched both.
    Title,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tag: String,
    pub isexact: bool,
    pub field: Field,
}

/// A parsed query: a boolean combination of token leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// Every child must match. An empty `And` matches everything (the empty query).
    And(Vec<Expr>),
    /// At least one child must match.
    Or(Vec<Expr>),
    /// The child must *not* match.
    Not(Box<Expr>),
    Term(Token),
}

/// One lexed piece of the input. Kept separate from [`Expr`] so grouping/precedence live in one
/// place ([`parse_query`]) and the (fiddly, quote-aware) character scanning lives in another.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Term(Token),
    /// A leading `-`: negates the atom that follows.
    Minus,
    Pipe,
    Open,
    Close,
}

/// The single entry point: parses a filter string into an expression tree.
pub fn parse_query(filter: &str) -> Expr {
    let pieces = lex(filter);
    let mut parser = Parser { pieces, pos: 0 };
    match parser.parse_or() {
        Some(expr) => expr,
        // Nothing parseable (`""`, `"  "`, `","`, a lone `-`) — a query with no conditions, which
        // matches everything, exactly as the empty filter did before.
        None => Expr::And(Vec::new()),
    }
}

/// Parses an *ordered* filter string into its terms, ignoring — and rejecting — the boolean syntax.
/// Only used by callers that must build a single ANDed predicate from a legacy-shaped string; see
/// its own engine call sites. Returns `None` when the input uses boolean structure, so a caller can
/// never silently drop an `|`/group and search for something narrower than what was asked.
pub fn parse_flat_terms(filter: &str) -> Option<Vec<Token>> {
    fn flatten(expr: Expr, out: &mut Vec<Token>) -> bool {
        match expr {
            Expr::Term(token) => {
                out.push(token);
                true
            }
            Expr::And(children) => children.into_iter().all(|child| flatten(child, out)),
            Expr::Or(_) | Expr::Not(_) => false,
        }
    }
    let mut out = Vec::new();
    flatten(parse_query(filter), &mut out).then_some(out)
}

struct Parser {
    pieces: Vec<Piece>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Piece> {
        self.pieces.get(self.pos)
    }

    fn next(&mut self) -> Option<Piece> {
        let piece = self.pieces.get(self.pos).cloned();
        if piece.is_some() {
            self.pos += 1;
        }
        piece
    }

    /// `a | b | c` — lowest precedence.
    fn parse_or(&mut self) -> Option<Expr> {
        let first = self.parse_and()?;
        let mut branches = vec![first];
        while matches!(self.peek(), Some(Piece::Pipe)) {
            self.next();
            // A dangling `|` (e.g. `a |`) just ends the chain rather than erroring — this parser
            // degrades gracefully on malformed input everywhere else, and a half-typed query is the
            // normal case while someone is still typing into the search box.
            match self.parse_and() {
                Some(branch) => branches.push(branch),
                None => break,
            }
        }
        if branches.len() == 1 {
            Some(branches.pop().expect("checked len above"))
        } else {
            Some(Expr::Or(branches))
        }
    }

    /// Juxtaposition: `a b c` is `And[a, b, c]`.
    fn parse_and(&mut self) -> Option<Expr> {
        let mut children = Vec::new();
        while let Some(child) = self.parse_unary() {
            children.push(child);
        }
        match children.len() {
            0 => None,
            1 => Some(children.pop().expect("checked len above")),
            _ => Some(Expr::And(children)),
        }
    }

    /// A leading `-`, then an atom.
    fn parse_unary(&mut self) -> Option<Expr> {
        let negated = matches!(self.peek(), Some(Piece::Minus));
        if negated {
            self.next();
        }
        let atom = self.parse_atom()?;
        Some(if negated {
            Expr::Not(Box::new(atom))
        } else {
            atom
        })
    }

    fn parse_atom(&mut self) -> Option<Expr> {
        // `peek` before consuming: a piece this arm doesn't own (`|` ends the AND chain, a `Close`
        // belongs to the enclosing group, a dangling `Minus` has nothing after it) must be left for
        // its own caller to see. Consuming it here silently ate the `|` between two branches, which
        // collapsed every `a | b` into just `a`.
        match self.peek()? {
            Piece::Term(_) => match self.next() {
                Some(Piece::Term(token)) => Some(Expr::Term(token)),
                _ => unreachable!("peeked a Term above"),
            },
            Piece::Open => {
                self.next();
                let inner = self.parse_or();
                // Consume the matching `Close` if the input actually had one; a missing one (an
                // unterminated group) still yields whatever was parsed, same graceful degradation
                // as an unterminated quote.
                if matches!(self.peek(), Some(Piece::Close)) {
                    self.next();
                }
                inner
            }
            Piece::Close | Piece::Pipe | Piece::Minus => None,
        }
    }
}

/// Splits the input into [`Piece`]s, mirroring the character scanning legacy's
/// `compute_search_filter` used (quotes, `\"`/`\\` escapes, `_`/`%` glob rewriting, lowercasing)
/// while additionally recognizing the boolean structure.
fn lex(filter: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut chars = filter.chars().peekable();

    loop {
        // Delimiters are separators, not pieces: juxtaposition is the AND.
        while matches!(chars.peek(), Some(',') | Some(' ')) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };

        match first {
            '|' => {
                chars.next();
                pieces.push(Piece::Pipe);
                continue;
            }
            '(' | '{' => {
                chars.next();
                pieces.push(Piece::Open);
                continue;
            }
            ')' | '}' => {
                chars.next();
                pieces.push(Piece::Close);
                continue;
            }
            _ => {}
        }

        // Negation is consumed here and the atom is read *immediately* after, in this same pass —
        // that is what keeps `--a` meaning "not the tag `-a`" rather than "not not a". Once the
        // dash is consumed this code is already "inside" the atom, so a following `-` is ordinary
        // text; legacy's single-pass scanner behaved identically. A dash with nothing searchable
        // after it (end of input, a delimiter, or another operator) is dropped to nothing, matching
        // legacy's "empty token is skipped" outcome.
        let mut negated = false;
        if chars.peek() == Some(&'-') {
            chars.next();
            negated = true;
            if matches!(
                chars.peek(),
                None | Some(',') | Some(' ') | Some('|') | Some(')') | Some('}')
            ) {
                continue;
            }
        }

        if matches!(chars.peek(), Some('(') | Some('{')) {
            chars.next();
            if negated {
                pieces.push(Piece::Minus);
            }
            pieces.push(Piece::Open);
            continue;
        }

        let (raw, isexact) = read_term(&mut chars);
        let (field, raw) = split_field_prefix(&raw);
        let tag = normalize(raw);
        if !tag.is_empty() {
            if negated {
                pieces.push(Piece::Minus);
            }
            pieces.push(Piece::Term(Token {
                tag,
                isexact,
                field,
            }));
        }
    }

    pieces
}

/// Reads one term's raw text (no field-prefix stripping, no normalization), mirroring legacy's
/// quoting rules. Stops at a delimiter or at a structural character — both only at the top level,
/// never inside a quoted segment.
fn read_term(chars: &mut std::iter::Peekable<std::str::Chars>) -> (String, bool) {
    if chars.peek() == Some(&'"') {
        chars.next();
        let body = read_quoted_body(chars);
        // Optional trailing `$` after the closing quote is accepted but redundant.
        if chars.peek() == Some(&'$') {
            chars.next();
        }
        return (body, true);
    }

    let mut s = String::new();
    let mut isexact = false;
    loop {
        match chars.peek() {
            None | Some(',') | Some(' ') | Some('|') | Some('(') | Some(')') | Some('{')
            | Some('}') => break,
            // `namespace:"value with spaces"` — matches e-hentai's own real search-box syntax
            // (`female:"double+penetration$"`), where only the *value* half is quoted, not the whole
            // `namespace:value` pair. The colon itself stays outside the quotes in the raw input but
            // ends up as a normal character in `s` either way, so the resulting token is identical
            // either way (`female:anal intercourse` whether written as `female:"anal intercourse"`
            // or the whole-token `"female:anal intercourse"` form the branch above already handles)
            // — this is purely an *additional accepted spelling*, not a new distinct semantic.
            Some(':') => {
                s.push(':');
                chars.next();
                if chars.peek() == Some(&'"') {
                    chars.next();
                    s.push_str(&read_quoted_body(chars));
                    isexact = true;
                    // Optional trailing `$` after the closing quote is accepted but redundant, same
                    // as the whole-token quote branch above.
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
}

/// Recognizes the `title:` (or `filename:`, same thing — the stored `title` is what legacy calls
/// both) field prefix, returning the remaining text. Any other namespace is left alone, so a real
/// tagged namespace can't be shadowed by accident.
fn split_field_prefix(raw: &str) -> (Field, &str) {
    let lower = raw.to_ascii_lowercase();
    for prefix in ["title:", "filename:"] {
        if lower.starts_with(prefix) {
            return (Field::Title, &raw[prefix.len()..]);
        }
    }
    (Field::Any, raw)
}

/// Reads a quoted value's body from just after the opening `"` up to (and consuming) the closing
/// `"`, honoring `\"` and `\\` as escapes — see this module's own docs for why. An unterminated
/// quote (input ends before a closing `"` is found) yields whatever was read so far rather than
/// erroring, matching every other malformed-input case in this parser (e.g. a trailing bare `-`),
/// which all degrade gracefully instead of rejecting the whole query.
fn read_quoted_body(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.peek() {
                Some('"') | Some('\\') => s.push(chars.next().expect("peeked Some above")),
                _ => s.push('\\'),
            },
            other => s.push(other),
        }
    }
    s
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

    fn tok(tag: &str, isexact: bool) -> Expr {
        Expr::Term(Token {
            tag: tag.to_string(),
            isexact,
            field: Field::Any,
        })
    }

    fn title(tag: &str, isexact: bool) -> Expr {
        Expr::Term(Token {
            tag: tag.to_string(),
            isexact,
            field: Field::Title,
        })
    }

    fn and(children: Vec<Expr>) -> Expr {
        Expr::And(children)
    }

    fn not(inner: Expr) -> Expr {
        Expr::Not(Box::new(inner))
    }

    #[test]
    fn parses_simple_comma_separated_tags() {
        assert_eq!(
            parse_query("artist:jane,adventure"),
            and(vec![tok("artist:jane", false), tok("adventure", false)])
        );
    }

    #[test]
    fn negation_prefix() {
        assert_eq!(parse_query("-artist:jane"), not(tok("artist:jane", false)));
    }

    #[test]
    fn exact_match_via_quotes() {
        assert_eq!(parse_query("\"artist:jane\""), tok("artist:jane", true));
    }

    #[test]
    fn exact_match_via_trailing_dollar() {
        assert_eq!(parse_query("artist:jane$"), tok("artist:jane", true));
    }

    #[test]
    fn escaped_quote_survives_inside_a_whole_token_quote() {
        // A literal `"` inside a tag value (e.g. a real artist name) — unescaped, the first `"`
        // would be read as the closing quote, truncating the value and leaving `bar"` behind to be
        // parsed as an unrelated bare token. `\"` protects it.
        assert_eq!(
            parse_query(r#""artist:foo\"bar""#),
            tok(r#"artist:foo"bar"#, true)
        );
    }

    #[test]
    fn escaped_quote_survives_inside_a_namespace_value_quote() {
        assert_eq!(
            parse_query(r#"artist:"foo\"bar""#),
            tok(r#"artist:foo"bar"#, true)
        );
    }

    #[test]
    fn escaped_backslash_stays_a_single_backslash() {
        assert_eq!(
            parse_query(r#"artist:"foo\\bar""#),
            tok(r#"artist:foo\bar"#, true)
        );
    }

    #[test]
    fn without_escaping_an_embedded_quote_truncates_and_leaves_a_stray_bare_token() {
        // Documents the pre-fix (still-current-if-unescaped) failure mode this fix addresses: a
        // raw, unescaped `"` inside the value silently splits into two unrelated tokens instead of
        // one — this is exactly why escaping exists, not a behavior to remove.
        assert_eq!(
            parse_query(r#"artist:"foo"bar""#),
            and(vec![tok("artist:foo", true), tok(r#"bar""#, false)])
        );
    }

    #[test]
    fn wildcard_normalization() {
        assert_eq!(parse_query("art_st:j%ne"), tok("art?st:j*ne", false));
    }

    #[test]
    fn lowercases_and_trims() {
        assert_eq!(
            parse_query(" Artist:JANE , Adventure "),
            and(vec![tok("artist:jane", false), tok("adventure", false)])
        );
    }

    #[test]
    fn empty_filter_yields_no_conditions() {
        assert_eq!(parse_query(""), and(Vec::new()));
        assert_eq!(parse_query("   "), and(Vec::new()));
    }

    // Issue #59: a plain space between two distinct `namespace:value` terms, typed the way a user
    // naturally would (no comma), used to silently collapse into one nonsense token that matched
    // nothing — verified live via `female:huge breasts female:milf` returning 0 results despite
    // each half independently matching 1. Space is now a real delimiter, same as comma.
    #[test]
    fn space_separates_tokens_like_comma() {
        assert_eq!(
            parse_query("female:milf language:chinese"),
            and(vec![
                tok("female:milf", false),
                tok("language:chinese", false)
            ])
        );
    }

    #[test]
    fn bare_keywords_space_separated() {
        // Synthetic (non-real-title) CJK text — this test only cares that a bare space between
        // two multi-byte tokens splits them, not about any specific real archive's content.
        assert_eq!(
            parse_query("さくら まぼろしの物語"),
            and(vec![tok("さくら", false), tok("まぼろしの物語", false)])
        );
    }

    // The other half of the fix: a multi-word tag *value* (still a real, common shape —
    // `female:huge breasts` is one tag, not two) must still be searchable as a single token now
    // that a bare space would otherwise split it. Quoting the *whole* token still works (top-level
    // quote branch, unchanged) and still ANDs correctly with a following unquoted token.
    #[test]
    fn quoted_whole_token_preserves_internal_space_and_still_ands_with_the_next_token() {
        assert_eq!(
            parse_query("\"female:huge breasts\" female:milf"),
            and(vec![
                tok("female:huge breasts", true),
                tok("female:milf", false)
            ])
        );
    }

    // e-hentai's own real search-box syntax quotes only the *value* half, colon outside the
    // quotes (`female:"double+penetration$"`) — now also accepted, and produces the identical
    // token either way (not a different, narrower kind of match).
    #[test]
    fn quoted_value_only_form_matches_quoted_whole_token_form_exactly() {
        assert_eq!(
            parse_query("female:\"anal intercourse\""),
            parse_query("\"female:anal intercourse\""),
        );
        assert_eq!(
            parse_query("female:\"anal intercourse\""),
            tok("female:anal intercourse", true)
        );
    }

    // Negation (`-` prefix) is parsed once per token, before either the quoted or unquoted branch
    // — unaffected by the space-delimiter/mid-token-quote additions above, but worth locking down
    // explicitly now that a token can be built two different ways.
    #[test]
    fn negation_combines_with_space_separated_tokens() {
        assert_eq!(
            parse_query("language:chinese -female:milf"),
            and(vec![
                tok("language:chinese", false),
                not(tok("female:milf", false))
            ])
        );
    }

    #[test]
    fn negation_combines_with_the_value_only_quote_form() {
        assert_eq!(
            parse_query("-female:\"huge breasts\""),
            not(tok("female:huge breasts", true))
        );
    }

    #[test]
    fn negation_combines_with_a_bare_space_separated_keyword() {
        // Synthetic CJK text, same reasoning as `bare_keywords_space_separated` above.
        assert_eq!(
            parse_query("-さくら まぼろしの物語"),
            and(vec![
                not(tok("さくら", false)),
                tok("まぼろしの物語", false)
            ])
        );
    }

    // ── Boolean structure (additive over legacy) ────────────────────────────────────────────────

    #[test]
    fn pipe_is_or() {
        assert_eq!(
            parse_query("artist:jane | artist:bob"),
            Expr::Or(vec![tok("artist:jane", false), tok("artist:bob", false)])
        );
    }

    #[test]
    fn implicit_and_binds_tighter_than_or() {
        // `a b | c` is `(a AND b) OR c`, not `a AND (b OR c)`.
        assert_eq!(
            parse_query("a b | c"),
            Expr::Or(vec![
                and(vec![tok("a", false), tok("b", false)]),
                tok("c", false),
            ])
        );
    }

    #[test]
    fn parens_and_braces_group_the_same_way() {
        let expected = Expr::Or(vec![
            tok("a", false),
            and(vec![tok("b", false), tok("c", false)]),
        ]);
        assert_eq!(parse_query("a | (b c)"), expected);
        assert_eq!(parse_query("a | {b c}"), expected);
    }

    #[test]
    fn nospace_around_or_and_parens_still_parses() {
        assert_eq!(
            parse_query("(a|b)c"),
            and(vec![
                Expr::Or(vec![tok("a", false), tok("b", false)]),
                tok("c", false),
            ])
        );
    }

    #[test]
    fn negation_applies_to_a_whole_group() {
        assert_eq!(
            parse_query("-(a | b)"),
            not(Expr::Or(vec![tok("a", false), tok("b", false)]))
        );
    }

    #[test]
    fn quoted_structural_characters_stay_literal() {
        // The documented escape hatch: quoting protects `|`/parens so a tag value that really
        // contains them (e.g. a circle name with parenthesized text) stays searchable.
        assert_eq!(
            parse_query(r#""serious graphics (ice)""#),
            tok("serious graphics (ice)", true)
        );
        assert_eq!(parse_query(r#"group:"a | b""#), tok("group:a | b", true));
    }

    #[test]
    fn dangling_operators_do_not_reject_the_query() {
        // Half-typed input is the normal case in a search box: everything here must degrade to
        // "the conditions that are actually present", never to an error.
        assert_eq!(parse_query("a |"), tok("a", false));
        assert_eq!(parse_query("(a"), tok("a", false));
        assert_eq!(parse_query("a -"), tok("a", false));
        assert_eq!(parse_query("-"), and(Vec::new()));
    }

    #[test]
    fn double_minus_stays_a_literal_leading_dash() {
        // Legacy consumed exactly one `-` as negation and then treated the next as ordinary text;
        // `--a` therefore searched for the tag `-a`. Preserved rather than "fixed", so this parser
        // can't silently change what an existing query means.
        assert_eq!(parse_query("--a"), not(tok("-a", false)));
    }

    #[test]
    fn title_prefix_scopes_a_token_to_the_title_half() {
        assert_eq!(parse_query("title:tari"), title("tari", false));
        assert_eq!(parse_query("filename:tari$"), title("tari", true));
        assert_eq!(parse_query("title:\"tari tari\""), title("tari tari", true));
        // Only the title half, and negation composes with it like any other atom.
        assert_eq!(parse_query("-title:tari"), not(title("tari", false)));
        // A namespace that merely *starts* with those letters is untouched.
        assert_eq!(parse_query("titles:tari"), tok("titles:tari", false));
    }

    #[test]
    fn flat_terms_refuses_boolean_structure_instead_of_dropping_it() {
        // `search_exists`-style callers need a plain ANDed term list; anything with real boolean
        // structure must come back `None` rather than silently losing the `|`/`-`/group.
        assert_eq!(
            parse_flat_terms("a -b"),
            None,
            "negation is not a flat term"
        );
        assert_eq!(parse_flat_terms("a | b"), None);
        assert_eq!(
            parse_flat_terms("(a)"),
            Some(vec![Token {
                tag: "a".into(),
                isexact: false,
                field: Field::Any
            }])
        );
    }
}
