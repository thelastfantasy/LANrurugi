//! Role-conditional access notes: the part of this API's behaviour OpenAPI cannot express.
//!
//! Several endpoints answer differently depending on *who* is asking — `GET /api/activity` shows a
//! session every entry but a guest token only its own; a `guest_visitor` sees a search restricted to
//! guest-visible categories, and gets a 404 (not a 403) for an archive outside them. OpenAPI has no
//! construct for that: a path and method identify one operation, and there is no per-role condition
//! on parameters or responses. `oneOf` would say "one of these shapes", not "this shape for this
//! role".
//!
//! So the notes live in a field of their own rather than in `description`:
//!
//! - They are **appended as a new key** (`x-lanrurugi-access-notes`), never merged into an existing
//!   one. `merge` is whole-operation replacement precisely because field-level merging produces a
//!   result nobody can predict from reading either input; adding a distinct key respects that rule
//!   instead of carving an exception into it.
//! - They therefore reach every consumer: the JSON, the docs site's callout, and — because the
//!   Swagger page sets `showExtensions` — the explorer's Extensions table.
//!
//! Writing them in the sidecar was the obvious alternative and is wrong: a sidecar entry replaces
//! the *whole* operation, so adding one sentence to `/api/search` would mean copying upstream's
//! description into this repository, after which the two drift.
//!
//! Keyed by `"METHOD /path"` rather than by a real OpenAPI path item, so a note can never be
//! mistaken for the fragment format that *does* replace operations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::scan::{HttpMethod, RouteRecord};

/// The document key the notes are emitted under.
pub const FIELD: &str = "x-lanrurugi-access-notes";

/// Marker opening the copy appended to `description`.
///
/// The notes are emitted twice on purpose, in two forms for two readers: as prose in `description`,
/// which is where every renderer already shows operation text — so Swagger UI puts it under the
/// endpoint title instead of in the Extensions table at the very bottom of a 1,500px-tall block —
/// and as a structured field for anything that wants to branch on it.
///
/// The HTML comment is what makes that duplication safe: the documentation site strips from this
/// marker to the end before rendering, because it shows the notes as its own localised callout and
/// would otherwise print the English copy directly above that callout. An HTML comment renders as
/// nothing, so a consumer that does not strip it is unaffected.
pub const DESCRIPTION_MARKER: &str = "<!-- lanrurugi:access-notes -->";

/// The prose form: a bold lead-in and a list, appended to whatever `description` already says.
///
/// The blank lines are load-bearing. With the marker directly above the heading, Swagger UI's
/// markdown renderer treats the block as raw text: the heading came out with its asterisks visible
/// and outside any tag, and only the list happened to survive. One blank line after the marker and
/// one after the heading keeps each construct its own block.
pub fn description_block(notes: &[String]) -> String {
    let mut out = String::from(DESCRIPTION_MARKER);
    out.push_str("\n\n**Access differs by role**\n");
    for note in notes {
        out.push_str("\n- ");
        out.push_str(note);
    }
    out
}

#[derive(Debug, Default)]
pub struct AccessNotes {
    by_route: BTreeMap<(String, HttpMethod), Vec<String>>,
}

impl AccessNotes {
    pub fn get(&self, path: &str, method: HttpMethod) -> Option<&Vec<String>> {
        self.by_route.get(&(path.to_string(), method))
    }

    /// How many routes carry notes. Reported rather than asserted, so "the file was actually read"
    /// is a number a reader can check.
    pub fn route_count(&self) -> usize {
        self.by_route.len()
    }

    pub fn note_count(&self) -> usize {
        self.by_route.values().map(Vec::len).sum()
    }
}

pub fn load(path: &Path, routes: &[RouteRecord]) -> Result<AccessNotes> {
    // A missing file is not an error: the notes are optional detail, and a checkout that has none
    // should still build. A file that exists and is wrong is a hard failure, like every other gate.
    if !path.exists() {
        return Ok(AccessNotes::default());
    }

    let display = path.display().to_string();
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {display}"))?;
    let parsed: Value =
        serde_saphyr::from_str(&text).with_context(|| format!("parsing {display}"))?;
    let Some(map) = parsed.as_object() else {
        bail!("{display}: expected a mapping of `METHOD /path` keys to notes");
    };

    let route_set: BTreeSet<(String, HttpMethod)> = routes
        .iter()
        .map(|r| (r.absolute_path.clone(), r.method))
        .collect();

    let mut notes = AccessNotes::default();
    let mut problems: Vec<String> = Vec::new();

    for (key, value) in map {
        let Some((method_name, route)) = key.split_once(' ') else {
            problems.push(format!(
                "{display}: `{key}` is not `METHOD /path` (for example `GET /api/activity`)."
            ));
            continue;
        };
        let Some(method) = HttpMethod::parse(&method_name.to_ascii_lowercase()) else {
            problems.push(format!(
                "{display}: `{key}` names method `{method_name}`, which is not one of \
                 get/post/put/delete/patch/head/options."
            ));
            continue;
        };
        if !route_set.contains(&(route.to_string(), method)) {
            problems.push(format!(
                "{display}: `{} {}` does not match any route the server registers. Either the route \
                 was removed or renamed (update or delete this entry), or the path is missing its \
                 `/api` prefix.",
                method.as_str().to_uppercase(),
                route
            ));
            continue;
        }

        let entries: Vec<String> = match value {
            Value::String(single) => vec![single.clone()],
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item.as_str() {
                        Some(text) => out.push(text.to_string()),
                        None => problems.push(format!(
                            "{display}: `{key}` has a non-string entry; every note must be a string."
                        )),
                    }
                }
                out
            }
            _ => {
                problems.push(format!(
                    "{display}: `{key}` must be a string or a list of strings, not {}.",
                    match value {
                        Value::Null => "null",
                        Value::Bool(_) => "a boolean",
                        Value::Number(_) => "a number",
                        Value::Object(_) => "a mapping",
                        _ => "that",
                    }
                ));
                continue;
            }
        };

        if entries.iter().all(|note| note.trim().is_empty()) {
            problems.push(format!("{display}: `{key}` has no non-empty note."));
            continue;
        }
        if notes
            .by_route
            .insert((route.to_string(), method), entries)
            .is_some()
        {
            problems.push(format!("{display}: `{key}` is listed more than once."));
        }
    }

    if !problems.is_empty() {
        problems.sort();
        bail!(
            "{display}: {} problem(s):\n  - {}",
            problems.len(),
            problems.join("\n  - ")
        );
    }

    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(path: &str, method: HttpMethod) -> RouteRecord {
        RouteRecord {
            method,
            absolute_path: path.to_string(),
            declared_path: path.to_string(),
            factory: "thing::router".to_string(),
            handler: Some("handler".to_string()),
            module: "thing".to_string(),
            file: "crates/lanrurugi-api/src/thing.rs".to_string(),
            line: 1,
        }
    }

    fn write(body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lanrurugi-access-notes-{}-{:?}.yaml",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn a_note_attaches_to_its_route() {
        let path = write("\"GET /api/activity\":\n  - session sees everything.\n");
        let notes = load(&path, &[route("/api/activity", HttpMethod::Get)]).expect("load");
        assert_eq!(
            notes.get("/api/activity", HttpMethod::Get),
            Some(&vec!["session sees everything.".to_string()])
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_single_string_is_accepted_as_one_note() {
        let path = write("\"GET /api/thing\": one line.\n");
        let notes = load(&path, &[route("/api/thing", HttpMethod::Get)]).expect("load");
        assert_eq!(notes.note_count(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_note_for_a_route_that_does_not_exist_is_refused() {
        let path = write("\"GET /api/gone\":\n  - stale.\n");
        let error = load(&path, &[route("/api/thing", HttpMethod::Get)])
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match any route"), "{error}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_key_that_is_not_method_and_path_is_refused() {
        let path = write("/api/thing:\n  - missing the method.\n");
        let error = load(&path, &[route("/api/thing", HttpMethod::Get)])
            .unwrap_err()
            .to_string();
        assert!(error.contains("is not `METHOD /path`"), "{error}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let missing = std::env::temp_dir().join("lanrurugi-access-notes-absent.yaml");
        let _ = std::fs::remove_file(&missing);
        let notes = load(&missing, &[]).expect("a missing notes file is fine");
        assert_eq!(notes.route_count(), 0);
    }
}
