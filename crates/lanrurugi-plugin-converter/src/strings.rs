//! Perl string-literal handling shared by the body converter: finding where a comment starts
//! (skipping over string contents so a `#` inside a string isn't mistaken for one), and
//! converting a single Perl string literal's content into the equivalent JS string/template
//! literal — the one place actual Perl→JS *semantic* translation happens (as opposed to the
//! mostly-syntactic substitutions in `body.rs`), since Perl's `"...$var..."` interpolation has no
//! direct JS syntax equivalent other than a template literal.

/// Splits a line into `(code, comment)` where `comment` is the text after a `#` that isn't inside
/// a string literal (or `None` if there's no such `#`).
pub fn split_code_and_comment(line: &str) -> (String, Option<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut in_string: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => in_string = Some(c),
            '#' => {
                let code: String = chars[..i].iter().collect();
                let comment: String = chars[i + 1..].iter().collect();
                return (code, Some(comment));
            }
            _ => {}
        }
        i += 1;
    }
    (line.to_string(), None)
}

/// One Perl string literal found in a line, with its quote style preserved so the caller can
/// decide how to render it.
#[derive(Debug, Clone)]
pub struct FoundString {
    pub quote: char,
    pub content: String,
}

/// Scans `line` for string literals, replacing each with a `\u{E000}` + index + `\u{E001}`
/// placeholder (private-use Unicode codepoints, never legitimately present in Perl source) so
/// the rest of the line can be safely regex-substituted without touching string contents, then
/// returns the placeholder'd line alongside the extracted strings for later restoration.
pub fn protect_strings(line: &str) -> (String, Vec<FoundString>) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut found = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            let quote = c;
            let mut content = String::new();
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    content.push(chars[i]);
                    content.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                content.push(chars[i]);
                i += 1;
            }
            let idx = found.len();
            found.push(FoundString { quote, content });
            out.push('\u{E000}');
            out.push_str(&idx.to_string());
            out.push('\u{E001}');
            continue;
        }
        out.push(c);
        i += 1;
    }

    (out, found)
}

/// Reverses [`protect_strings`], rendering each placeholder back into a JS string/template
/// literal via [`convert_string_content`].
pub fn restore_strings(line: &str, found: &[FoundString]) -> String {
    let mut out = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\u{E000}' {
            let mut j = i + 1;
            let mut digits = String::new();
            while j < chars.len() && chars[j] != '\u{E001}' {
                digits.push(chars[j]);
                j += 1;
            }
            if let Ok(idx) = digits.parse::<usize>() {
                if let Some(s) = found.get(idx) {
                    out.push_str(&convert_string_content(s));
                }
            }
            i = j + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Converts one Perl string literal to a JS string or template literal.
///
/// Single-quoted Perl strings never interpolate, so they map straight to an equivalent-content
/// JS string. Double-quoted ones do interpolate `$var`/`@arr`/`${expr}` — if any interpolation
/// marker is present the whole literal becomes a JS template literal with `$var` → `${var}` and
/// `@arr` → `${arr.join(", ")}` (Perl's list interpolation join — this assumes the default `$"`
/// separator, a single space in real Perl, approximated here as `", "` since that's what every
/// tag-list plugin in the actual corpus wants; genuinely space-joined output needs a manual
/// tweak, called out via the `likely_needs_review` flag callers can check for `@` markers).
pub fn convert_string_content(found: &FoundString) -> String {
    // No re-escaping here: `found.content` already carries whatever escape sequences the
    // original Perl source used verbatim (see `protect_strings`, which preserves a backslash and
    // the character after it as-is rather than interpreting it) — and Perl/JS agree on what
    // `\\`, `\"`, `\n`, `\t` etc. mean, so passing them through unchanged is already correct.
    // Escaping again here would double an existing backslash, turning e.g. a real `\n` (newline
    // escape) into the literal two characters `\n` (backslash then the letter n).
    if found.quote == '\'' {
        return format!("'{}'", found.content);
    }

    let interpolates = found.content.contains('$') || found.content.contains('@');
    if !interpolates {
        return format!("\"{}\"", found.content);
    }

    let mut out = String::from("`");
    let chars: Vec<char> = found.content.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '$' if chars.get(i + 1) == Some(&'{') => {
                // `${...}` complex interpolation already matches JS template-literal syntax
                // verbatim (see module docs) — copy through unchanged.
                out.push('$');
                out.push('{');
                i += 2;
                while i < chars.len() && chars[i] != '}' {
                    out.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    out.push('}');
                    i += 1;
                }
            }
            '$' => {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j > start {
                    let name: String = chars[start..j].iter().collect();
                    out.push_str("${");
                    out.push_str(&name);
                    out.push('}');
                    i = j;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            '@' => {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j > start {
                    let name: String = chars[start..j].iter().collect();
                    out.push_str("${");
                    out.push_str(&name);
                    out.push_str(".join(\", \")}");
                    i = j;
                } else {
                    out.push('@');
                    i += 1;
                }
            }
            '`' => {
                out.push('\\');
                out.push('`');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.push('`');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_code_and_comment_ignores_hash_inside_string() {
        let (code, comment) = split_code_and_comment(r#"my $x = "a # b"; # real comment"#);
        assert_eq!(code, r#"my $x = "a # b"; "#);
        assert_eq!(comment, Some(" real comment".to_string()));
    }

    #[test]
    fn split_code_and_comment_returns_none_when_no_comment() {
        let (code, comment) = split_code_and_comment("my $x = 5;");
        assert_eq!(code, "my $x = 5;");
        assert_eq!(comment, None);
    }

    #[test]
    fn protect_and_restore_roundtrips_a_plain_string() {
        let line = r#"my $x = "hello";"#;
        let (protected, found) = protect_strings(line);
        assert!(protected.contains('\u{E000}'));
        let restored = restore_strings(&protected, &found);
        assert_eq!(restored, r#"my $x = "hello";"#);
    }

    #[test]
    fn single_quoted_strings_never_interpolate() {
        let found = FoundString {
            quote: '\'',
            content: "$literal not interpolated".to_string(),
        };
        assert_eq!(
            convert_string_content(&found),
            "'$literal not interpolated'"
        );
    }

    #[test]
    fn double_quoted_scalar_interpolation_becomes_template_literal() {
        let found = FoundString {
            quote: '"',
            content: "Processing file: $file_path".to_string(),
        };
        assert_eq!(
            convert_string_content(&found),
            "`Processing file: ${file_path}`"
        );
    }

    #[test]
    fn double_quoted_array_interpolation_joins() {
        let found = FoundString {
            quote: '"',
            content: "Tags: @tags".to_string(),
        };
        assert_eq!(
            convert_string_content(&found),
            "`Tags: ${tags.join(\", \")}`"
        );
    }

    #[test]
    fn non_interpolating_double_quoted_string_stays_a_plain_string() {
        let found = FoundString {
            quote: '"',
            content: "no interpolation here".to_string(),
        };
        assert_eq!(convert_string_content(&found), "\"no interpolation here\"");
    }

    #[test]
    fn complex_dollar_brace_interpolation_passes_through() {
        let found = FoundString {
            quote: '"',
            content: "Copying tags from archive \"${lrr_gid}\"".to_string(),
        };
        assert_eq!(
            convert_string_content(&found),
            "`Copying tags from archive \"${lrr_gid}\"`"
        );
    }
}
