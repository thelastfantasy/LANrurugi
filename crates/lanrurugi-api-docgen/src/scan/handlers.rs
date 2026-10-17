//! Handler documentation recovery.
//!
//! Handlers in this codebase carry `///` comments describing real behaviour — which query
//! parameters matter, which fields are omitted when absent, what a failure means. Those comments
//! are the single best source of endpoint descriptions available without inventing anything, and
//! they cost nothing to surface: the maintainer already writes them, and because the generated
//! document quotes them rather than paraphrasing, the two cannot drift apart.

use std::collections::HashMap;

use super::FnInfo;

/// Resolves a handler name to its definition, preferring a same-file match.
///
/// Same-file first is not a tie-breaker detail: each module defines both its router and its
/// handlers, so the local match is the correct one essentially always. The global fallback exists
/// for the handful of handlers reached through a re-export, and refuses to guess when a name is
/// genuinely ambiguous.
pub struct FnLookup<'a> {
    by_file_and_name: HashMap<(&'a str, &'a str), &'a FnInfo>,
    by_name: HashMap<&'a str, Vec<&'a FnInfo>>,
}

impl<'a> FnLookup<'a> {
    pub fn new(fns: &'a [FnInfo]) -> Self {
        let mut by_file_and_name = HashMap::new();
        let mut by_name: HashMap<&str, Vec<&FnInfo>> = HashMap::new();
        for f in fns {
            by_file_and_name.insert((f.file.as_str(), f.name.as_str()), f);
            by_name.entry(f.name.as_str()).or_default().push(f);
        }
        Self {
            by_file_and_name,
            by_name,
        }
    }

    pub fn resolve(&self, file: &str, handler: &str) -> Option<&'a FnInfo> {
        if let Some(found) = self.by_file_and_name.get(&(file, handler)) {
            return Some(found);
        }
        match self.by_name.get(handler) {
            Some(candidates) if candidates.len() == 1 => Some(candidates[0]),
            _ => None,
        }
    }
}

/// Extract a `///` doc comment from an item's attributes.
///
/// `#[doc = " ..."]` is the same thing spelled differently and is accepted too. Blank leading
/// lines and the single space `rustdoc` strips are removed so the text can go straight into an
/// OpenAPI description.
pub fn doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    let mut is_doc = false;
    let mut lines: Vec<String> = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        is_doc = true;
        let syn::Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = &nv.value
        else {
            continue;
        };
        lines.push(
            value
                .value()
                .strip_prefix(' ')
                .unwrap_or(&value.value())
                .to_string(),
        );
    }

    if !is_doc {
        return None;
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

/// Split a doc comment into a one-line summary and the remaining prose.
///
/// Doc comments in this codebase are hard-wrapped at about a hundred columns, so "the first line"
/// is usually not a sentence. The first *sentence boundary* on the first line is preferred, with a
/// minimum length so that a short lead-in like "`GET /jobs`." is not mistaken for one.
pub fn split_summary(doc: &str) -> (String, String) {
    let trimmed = doc.trim();
    let mut lines = trimmed.lines();
    let first_line = lines.next().unwrap_or("").trim().to_string();
    let remaining_lines = lines.collect::<Vec<_>>().join("\n");
    let remaining_lines = remaining_lines.trim().to_string();

    let summary = match first_line.find(". ") {
        Some(idx) if idx >= 20 => first_line[..=idx].trim().to_string(),
        _ => first_line.clone(),
    };
    let summary = summary.trim_end_matches('.').trim().to_string();

    let leftover = match first_line.find(summary.as_str()) {
        Some(idx) if !summary.is_empty() => first_line[idx + summary.len()..]
            .trim_start_matches(['.', ' '])
            .trim()
            .to_string(),
        _ => String::new(),
    };

    let rest = match (leftover.is_empty(), remaining_lines.is_empty()) {
        (true, true) => String::new(),
        (false, true) => leftover,
        (true, false) => remaining_lines,
        (false, false) => format!("{leftover}\n\n{remaining_lines}"),
    };
    (summary, rest)
}

/// `list_archives` -> `listArchives`, the shape OpenAPI tooling expects of an `operationId`.
pub fn operation_id_from_handler(handler: &str) -> String {
    let mut out = String::with_capacity(handler.len());
    let mut upper_next = false;
    for ch in handler.chars() {
        if ch == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "handler".to_string()
    } else {
        out
    }
}
