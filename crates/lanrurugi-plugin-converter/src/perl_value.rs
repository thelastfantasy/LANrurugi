//! A small recursive-descent parser for the subset of Perl literal syntax actually used inside
//! `sub plugin_info { return ( key => value, ... ); }` blocks across every legacy plugin
//! (`~/LANraragi/lib/LANraragi/Plugin/*/*.pm`, verified by reading the full corpus): quoted
//! strings (single/double, including ones that span multiple physical lines — Perl allows literal
//! newlines inside a double-quoted string, and several plugins' `icon`/`description` fields do
//! exactly that), `[ ... ]` arrays, `{ ... }` hashes, and bareword hash keys before `=>`. This is
//! deliberately not a general Perl parser — Perl's real grammar requires a full interpreter to
//! parse in general (it's context-sensitive), but `plugin_info`'s body is a flat, static literal
//! in every real plugin, which this covers exactly.

#[derive(Debug, Clone, PartialEq)]
pub enum PerlValue {
    Str(String),
    Array(Vec<PerlValue>),
    Hash(Vec<(String, PerlValue)>),
}

impl PerlValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PerlValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PerlValue]> {
        match self {
            PerlValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_hash(&self) -> Option<&[(String, PerlValue)]> {
        match self {
            PerlValue::Hash(h) => Some(h),
            _ => None,
        }
    }
}

/// Looks up a key in a parsed hash's top-level fields.
pub fn hash_get<'a>(hash: &'a [(String, PerlValue)], key: &str) -> Option<&'a PerlValue> {
    hash.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

pub struct Scanner {
    chars: Vec<char>,
    pos: usize,
}

impl Scanner {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while matches!(self.peek(), Some(c) if c.is_whitespace()) {
                self.advance();
            }
            if self.peek() == Some('#') {
                while !matches!(self.peek(), Some('\n') | None) {
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    /// Finds the byte offset (char index) of the start of `sub NAME { ... }`'s body — specifically
    /// the `(` immediately following that sub's `return` keyword — for the given sub name. Returns
    /// `None` if no such sub exists in the source.
    pub fn find_return_paren_in_sub(src: &str, sub_name: &str) -> Option<usize> {
        let marker = format!("sub {sub_name}");
        let sub_start = src.find(&marker)?;
        let after_marker = &src[sub_start..];
        let return_offset = after_marker.find("return")?;
        let after_return = &after_marker[return_offset + "return".len()..];
        let paren_offset = after_return.find('(')?;
        Some(sub_start + return_offset + "return".len() + paren_offset + 1)
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn parse_string(&mut self) -> String {
        let quote = self.advance().expect("caller checked a quote is present");
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c == '\\' && quote == '"' {
                // Double-quoted strings: interpret the common escapes actually used in the
                // corpus.
                self.advance();
                match self.advance() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                }
                continue;
            }
            if c == '\\' && quote == '\'' {
                // Single-quoted Perl strings only ever treat `\\` and `\'` as escapes (producing
                // a literal `\` or `'`) — any other `\X` is left as a literal backslash followed
                // by `X` (verified: real `perlop` semantics for `q'...'`/`'...'`). Previously this
                // branch didn't exist at all, so a bare backslash fell through to the plain
                // "push and advance" case below — meaning the *next* char (the escaped `'` or
                // `\`) was then read as this string's own terminator on the following loop
                // iteration, ending the string early and leaving the scanner desynced partway
                // through the source (see `Metadata/MEMS.pm`'s `'Mayriad\'s ... Script.'`, which
                // triggered exactly this — a real, reproducible OOM: the desynced scanner then
                // fed `parse_hash_body`/`parse_list` a stray punctuation character neither
                // `parse_bareword` nor `parse_value`'s fallback can consume, and — before the
                // zero-progress guard added alongside this fix — that looped forever pushing
                // empty entries into an ever-growing `Vec` until the process was OOM-killed).
                match self.peek_at(1) {
                    Some('\'') | Some('\\') => {
                        self.advance();
                        out.push(self.advance().unwrap());
                    }
                    _ => {
                        out.push(c);
                        self.advance();
                    }
                }
                continue;
            }
            if c == quote {
                self.advance();
                break;
            }
            out.push(c);
            self.advance();
        }
        out
    }

    fn parse_bareword(&mut self) -> String {
        let mut out = String::new();
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            out.push(self.advance().unwrap());
        }
        out
    }

    /// Parses one value: a string, an array (`[...]`), a hash (`{...}`), or a bareword (treated
    /// as a string — legacy sometimes writes unquoted single-word values).
    pub fn parse_value(&mut self) -> PerlValue {
        self.skip_ws_and_comments();
        match self.peek() {
            Some('"') | Some('\'') => PerlValue::Str(self.parse_string()),
            Some('[') => {
                self.advance();
                PerlValue::Array(self.parse_list(']'))
            }
            Some('{') => {
                self.advance();
                PerlValue::Hash(self.parse_hash_body('}'))
            }
            _ => PerlValue::Str(self.parse_bareword()),
        }
    }

    /// Parses a comma-separated list of values until (and consuming) `closing`.
    fn parse_list(&mut self, closing: char) -> Vec<PerlValue> {
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == Some(closing) {
                self.advance();
                break;
            }
            if self.at_end() {
                break;
            }
            // Safety net: a character this scanner doesn't recognize at all (neither a quote,
            // bracket, alphanumeric bareword char, nor `closing`/`,`) would otherwise leave
            // `parse_value`'s bareword fallback returning `""` with zero advance, spinning this
            // loop forever while it keeps pushing empty values into `items` — an unbounded-memory
            // hang, not just a slow parse (this is exactly how a single mis-parsed escape
            // sequence upstream, in `parse_string`, once turned into a real OOM — see its own
            // docs). Detecting "no progress" here and forcibly skipping one character keeps this
            // parser's promise of always terminating even against input its restricted grammar
            // doesn't actually cover, rather than depending on every producer of a `Scanner` never
            // handing it anything unexpected.
            let pos_before = self.pos;
            items.push(self.parse_value());
            if self.pos == pos_before {
                self.advance();
            }
            self.skip_ws_and_comments();
            if self.peek() == Some(',') {
                self.advance();
            }
        }
        items
    }

    /// Parses `key => value, key => value, ...` until (and consuming) `closing`.
    pub fn parse_hash_body(&mut self, closing: char) -> Vec<(String, PerlValue)> {
        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == Some(closing) {
                self.advance();
                break;
            }
            if self.at_end() {
                break;
            }

            // Safety net: see the identical guard in `parse_list` — a character this scanner
            // doesn't recognize as a quote/bareword-char/`closing`/`,` would otherwise leave both
            // the key and value parses returning `""`/zero advance, spinning this loop forever
            // while it keeps pushing empty `("", "")` fields — an unbounded-memory hang, not just
            // a slow parse.
            let pos_before = self.pos;

            let key = if matches!(self.peek(), Some('"') | Some('\'')) {
                self.parse_string()
            } else {
                self.parse_bareword()
            };

            self.skip_ws_and_comments();
            // Consume `=>` or a plain `,`-preceding `:`-less separator (legacy always uses `=>`).
            if self.peek() == Some('=') && self.peek_at(1) == Some('>') {
                self.advance();
                self.advance();
            }

            let value = self.parse_value();
            fields.push((key, value));

            if self.pos == pos_before {
                self.advance();
            }

            self.skip_ws_and_comments();
            if self.peek() == Some(',') {
                self.advance();
            }
        }
        fields
    }
}

/// Extracts and parses `sub plugin_info { return ( ... ); }`'s literal hash from a full `.pm`
/// source file's text.
pub fn parse_plugin_info(source: &str) -> Option<Vec<(String, PerlValue)>> {
    let paren_char_idx = Scanner::find_return_paren_in_sub_charwise(source, "plugin_info")?;
    let mut scanner = Scanner::new(source);
    scanner.pos = paren_char_idx;
    Some(scanner.parse_hash_body(')'))
}

impl Scanner {
    /// Same as `find_return_paren_in_sub`, but returns a **char** index (not byte offset) into
    /// the source, since [`Scanner`] operates over `Vec<char>`.
    fn find_return_paren_in_sub_charwise(src: &str, sub_name: &str) -> Option<usize> {
        let byte_idx = Self::find_return_paren_in_sub(src, sub_name)?;
        Some(src[..byte_idx].chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_flat_hash_of_strings() {
        let src = r#"
            sub plugin_info {
                return (
                    name => "Tag Copier",
                    type => "metadata",
                    namespace => "copytags",
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(
            hash_get(&fields, "name").unwrap().as_str(),
            Some("Tag Copier")
        );
        assert_eq!(
            hash_get(&fields, "type").unwrap().as_str(),
            Some("metadata")
        );
        assert_eq!(
            hash_get(&fields, "namespace").unwrap().as_str(),
            Some("copytags")
        );
    }

    #[test]
    fn parses_an_array_of_parameter_hashes() {
        let src = r#"
            sub plugin_info {
                return (
                    name => "X",
                    parameters => [ { type => "bool", desc => "Assume english" }, { type => "bool", desc => "Add tag" } ]
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let params = hash_get(&fields, "parameters").unwrap().as_array().unwrap();
        assert_eq!(params.len(), 2);
        let first = params[0].as_hash().unwrap();
        assert_eq!(
            hash_get(first, "desc").unwrap().as_str(),
            Some("Assume english")
        );
    }

    #[test]
    fn parses_a_hash_shaped_parameters_field() {
        let src = r#"
            sub plugin_info {
                return (
                    name => "X",
                    parameters => {
                        'copy_date_added' => {
                            type => "bool",
                            desc => "Enable to also copy the date"
                        }
                    }
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let params = hash_get(&fields, "parameters").unwrap().as_hash().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "copy_date_added");
    }

    #[test]
    fn handles_multiline_double_quoted_strings() {
        let src = "sub plugin_info {\n    return (\n        description => \"Line one\nLine two\",\n    );\n}\n";
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(
            hash_get(&fields, "description").unwrap().as_str(),
            Some("Line one\nLine two")
        );
    }

    #[test]
    fn skips_comments_between_fields() {
        let src = r#"
            sub plugin_info {
                return (
                    #Standard metadata
                    name => "X",
                    #Another comment
                    type => "metadata",
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(hash_get(&fields, "name").unwrap().as_str(), Some("X"));
        assert_eq!(
            hash_get(&fields, "type").unwrap().as_str(),
            Some("metadata")
        );
    }

    #[test]
    fn handles_escaped_quotes_inside_double_quoted_strings() {
        let src = r#"sub plugin_info { return ( description => "Say \"hi\" to me" ); }"#;
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(
            hash_get(&fields, "description").unwrap().as_str(),
            Some("Say \"hi\" to me")
        );
    }

    #[test]
    fn returns_none_when_no_plugin_info_sub_exists() {
        let src = "sub get_tags { return (tags => 'x'); }";
        assert!(parse_plugin_info(src).is_none());
    }

    #[test]
    fn handles_escaped_apostrophes_inside_single_quoted_strings() {
        // Regression test: `Metadata/MEMS.pm`'s real `description` field
        // (`'Mayriad\'s EH Master Script.'`) used to desync this scanner entirely — the escaped
        // `'` was misread as the string's own closing quote, and the leftover fragment
        // (`s EH Master Script.'`) went on to desync `parse_hash_body` so badly it looped
        // forever pushing empty fields into an unbounded `Vec`, OOM-killing the process (see
        // `parse_string`'s own docs for the full chain). Covers both real Perl single-quote
        // escapes: `\'` → `'` and `\\` → `\`.
        let src = r"sub plugin_info { return ( description => 'Mayriad\'s EH Master Script.' ); }";
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(
            hash_get(&fields, "description").unwrap().as_str(),
            Some("Mayriad's EH Master Script.")
        );
    }

    #[test]
    fn single_quoted_backslash_escape_produces_a_literal_backslash() {
        let src = r"sub plugin_info { return ( name => 'a\\b' ); }";
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(hash_get(&fields, "name").unwrap().as_str(), Some(r"a\b"));
    }

    #[test]
    fn single_quoted_backslash_before_an_ordinary_char_stays_literal() {
        // Real Perl single-quote semantics: only `\\` and `\'` are escapes; `\n` inside `'...'`
        // is a literal backslash followed by `n`, not a newline (unlike in a double-quoted
        // string).
        let src = r"sub plugin_info { return ( name => 'a\nb' ); }";
        let fields = parse_plugin_info(src).unwrap();
        assert_eq!(hash_get(&fields, "name").unwrap().as_str(), Some(r"a\nb"));
    }

    #[test]
    fn parse_hash_body_terminates_on_an_unrecognized_stray_character() {
        // Simulates whatever a *future* scanner bug might still desync onto — a character this
        // restricted grammar has no case for at all (not a quote, bracket, alphanumeric bareword
        // char, `closing`, or `,`). Before the zero-progress guard, `parse_value`'s bareword
        // fallback would return `""` with zero advance here, spinning `parse_hash_body` forever
        // while it kept pushing empty fields — this test's own completion (returning at all,
        // rather than the test process hanging) is the actual assertion; the field-count bound
        // just guards against a future change accidentally reintroducing silent unbounded growth.
        let src = "sub plugin_info { return ( ! ! ! ) }";
        let fields = parse_plugin_info(src).unwrap();
        assert!(
            fields.len() < 100,
            "expected the scanner to bail out quickly, got {} fields",
            fields.len()
        );
    }
}
