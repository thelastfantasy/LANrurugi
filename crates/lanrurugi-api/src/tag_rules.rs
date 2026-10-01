//! Tag rewrite rules (`tagrules`/`tagruleson` Settings-page fields) — verified against legacy's
//! own real implementation (`~/LANraragi/lib/LANraragi/Utils/Tags.pm::tags_rules_to_array`/
//! `rewrite_tags`/`apply_rules`, invoked from `Model/Plugins.pm:292-296` right after a metadata
//! plugin returns its new tags, before they're merged into the archive's existing tags). This was
//! a real Phase 1 gap (issue #85): the Settings-page UI and Redis field existed, but nothing ever
//! read `tagrules` to actually rewrite anything.
//!
//! One rule per line (the frontend's own `<textarea>` — see `TagsThumbnailsSection.tsx`'s "Split
//! rules with linebreaks" copy). Six rule shapes, syntax and matching semantics copied field-for-
//! field from `tags_rules_to_array`:
//!
//! - `-tag` → [`Rule::Remove`] — drops a tag exactly matching `tag` (case-insensitive).
//! - `-namespace:*` → [`Rule::RemoveNamespace`] — drops every tag under `namespace:`.
//! - `~namespace` → [`Rule::StripNamespace`] — strips the `namespace:` prefix, keeps the rest.
//! - `namespace:* -> new-namespace:*` → [`Rule::ReplaceNamespace`].
//! - `tag -> new-tag` → [`Rule::Replace`] — legacy also allows a bare `tag` line with no `->` at
//!   all as a shorthand for `-tag` ("blacklist mode", `apply_rules`'s comment) — not implemented
//!   here since the frontend's own documented syntax never mentions it and no default rule uses
//!   it; every rule this parser doesn't recognize is just skipped rather than guessed at.
//! - `tag => new-tag` → [`Rule::HashReplace`] — same effect as `Replace`, but legacy calls out
//!   that it's resolved via a hashtable *after* every other rule type, for archives with large
//!   rule sets. Preserved here as a distinct variant + apply-order so a plugin author copying
//!   legacy rule sets verbatim gets identical output, even though a hashmap isn't otherwise
//!   necessary at this project's scale.
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Remove(String),
    RemoveNamespace(String),
    StripNamespace(String),
    ReplaceNamespace(String, String),
    Replace(String, String),
    HashReplace(String, String),
}

/// Parses the `tagrules` setting's raw text into an ordered rule list. Accepts both `\n`-separated
/// lines (this port's own `<textarea>` convention) and legacy's real Redis-stored `;`-separated
/// flat encoding (`Model/Config.pm::get_tagrules`'s own default value, inherited verbatim as this
/// field's default in `settings.rs::STRING_FIELDS`) — a line is split on `;` only when it contains
/// no `\n` of its own, so a `;` genuinely typed inside a real multi-line rule set (unlikely, but
/// not guaranteed absent) isn't mangled.
fn parse_rules(text: &str) -> Vec<Rule> {
    let normalized = if text.contains('\n') {
        text.to_string()
    } else {
        text.replace(';', "\n")
    };

    let mut rules = Vec::new();
    for line in normalized.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((raw_match, raw_value)) = line.split_once("=>") {
            let m = raw_match.trim();
            let v = raw_value.trim();
            if !m.is_empty() {
                rules.push(Rule::HashReplace(m.to_lowercase(), v.to_string()));
            }
            continue;
        }

        let (raw_match, raw_value) = match line.split_once("->") {
            Some((m, v)) => (m.trim(), v.trim()),
            None => (line, ""),
        };
        if raw_match.is_empty() {
            continue;
        }

        if raw_value.is_empty() && raw_match.starts_with('-') && raw_match.ends_with(":*") {
            let ns = &raw_match[1..raw_match.len() - 2];
            rules.push(Rule::RemoveNamespace(ns.to_lowercase()));
        } else if raw_value.is_empty() && raw_match.starts_with('-') {
            rules.push(Rule::Remove(raw_match[1..].to_lowercase()));
        } else if raw_value.is_empty() && raw_match.starts_with('~') {
            rules.push(Rule::StripNamespace(raw_match[1..].to_lowercase()));
        } else if raw_match.ends_with(":*") && raw_value.ends_with(":*") {
            let m = &raw_match[..raw_match.len() - 2];
            let v = &raw_value[..raw_value.len() - 2];
            rules.push(Rule::ReplaceNamespace(m.to_lowercase(), v.to_string()));
        } else if !raw_value.is_empty() {
            rules.push(Rule::Replace(
                raw_match.to_lowercase(),
                raw_value.to_string(),
            ));
        }
        // A bare line with no `->`/`=>` and no recognized prefix (legacy's "blacklist mode"
        // shorthand for `-tag`) is intentionally left unrecognized — see module docs.
    }
    rules
}

/// Applies `tagrules` text to a comma-separated tag list, matching legacy's `rewrite_tags` +
/// `apply_rules` order: every non-hash rule runs first, in file order, on each tag in turn; then
/// (only if the tag survived) the hash-replace table is consulted once. Returns the rewritten
/// comma-separated string, ready to merge into an archive's tags the same way un-rewritten plugin
/// output already is (`plugins.rs::run_enabled_metadata_plugins_on_archive`).
pub fn apply_tag_rules(tags_csv: &str, rules_text: &str) -> String {
    let rules = parse_rules(rules_text);
    if rules.is_empty() {
        return tags_csv.to_string();
    }

    let mut hash_replace: HashMap<String, String> = HashMap::new();
    let mut other_rules = Vec::new();
    for rule in rules {
        match rule {
            Rule::HashReplace(m, v) => {
                hash_replace.insert(m, v);
            }
            other => other_rules.push(other),
        }
    }

    let mut out = Vec::new();
    for tag in tags_csv.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if let Some(rewritten) = apply_rules_to_tag(tag, &other_rules, &hash_replace) {
            out.push(rewritten);
        }
    }
    out.join(",")
}

fn apply_rules_to_tag(
    tag: &str,
    rules: &[Rule],
    hash_replace: &HashMap<String, String>,
) -> Option<String> {
    let mut tag = tag.to_string();
    for rule in rules {
        match rule {
            Rule::Remove(m) => {
                if tag.to_lowercase() == *m {
                    return None;
                }
            }
            Rule::RemoveNamespace(ns) => {
                if let Some((prefix, _)) = tag.split_once(':') {
                    if prefix.to_lowercase() == *ns {
                        return None;
                    }
                }
            }
            Rule::StripNamespace(ns) => {
                if let Some((prefix, rest)) = tag.split_once(':') {
                    if prefix.to_lowercase() == *ns {
                        tag = rest.to_string();
                    }
                }
            }
            Rule::ReplaceNamespace(m, v) => {
                if let Some((prefix, rest)) = tag.split_once(':') {
                    if prefix.to_lowercase() == *m {
                        tag = format!("{v}:{rest}");
                    }
                }
            }
            Rule::Replace(m, v) => {
                if tag.to_lowercase() == *m {
                    tag = v.clone();
                }
            }
            Rule::HashReplace(..) => {
                unreachable!("hash_replace rules are split out before this loop")
            }
        }
    }
    if let Some(replacement) = hash_replace.get(&tag.to_lowercase()) {
        tag = replacement.clone();
    }
    Some(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_rule_drops_an_exact_tag_case_insensitively() {
        assert_eq!(apply_tag_rules("Foo,bar,baz", "-bar"), "Foo,baz");
    }

    #[test]
    fn remove_namespace_rule_drops_every_tag_under_that_namespace() {
        assert_eq!(
            apply_tag_rules("source:fakku,artist:jane,other", "-source:*"),
            "artist:jane,other"
        );
    }

    #[test]
    fn strip_namespace_rule_keeps_the_tag_without_its_prefix() {
        assert_eq!(
            apply_tag_rules("lang:english,other", "~lang"),
            "english,other"
        );
    }

    #[test]
    fn replace_namespace_rule_renames_the_namespace_prefix() {
        assert_eq!(
            apply_tag_rules("female:tall,other", "female:* -> girl:*"),
            "girl:tall,other"
        );
    }

    #[test]
    fn replace_rule_swaps_one_exact_tag_for_another() {
        assert_eq!(
            apply_tag_rules("oneshot,other", "oneshot -> one-shot"),
            "one-shot,other"
        );
    }

    #[test]
    fn hash_replace_rule_runs_after_every_other_rule_type() {
        // The tag is first renamed by a `Replace` rule to something the `HashReplace` rule then
        // also matches — confirms hash-replace really does run in a second pass, not interleaved.
        assert_eq!(apply_tag_rules("a", "a -> b\nb => c"), "c");
    }

    #[test]
    fn blank_lines_and_whitespace_are_ignored() {
        assert_eq!(apply_tag_rules("foo", "\n  \n-bar\n\n"), "foo");
    }

    #[test]
    fn semicolon_separated_text_is_treated_the_same_as_newline_separated() {
        // Legacy's own default `tagrules` value (`settings.rs::STRING_FIELDS`) is `;`-joined —
        // must parse identically to the same rules written one-per-line.
        assert_eq!(
            apply_tag_rules(
                "already uploaded,keep",
                "-already uploaded;-forbidden content"
            ),
            "keep"
        );
    }

    #[test]
    fn no_rules_returns_the_input_unchanged() {
        assert_eq!(apply_tag_rules("a,b,c", ""), "a,b,c");
    }

    #[test]
    fn unmatched_tags_pass_through_untouched() {
        assert_eq!(
            apply_tag_rules("unrelated", "-bar\nfoo -> baz"),
            "unrelated"
        );
    }
}
