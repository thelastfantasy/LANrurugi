//! Scanner behaviour over fixture sources.
//!
//! The scanner is the one place where a silent mistake is invisible downstream: every gate is built
//! on its output, so a route it fails to see simply does not exist as far as the rest of the
//! pipeline is concerned. Each case below pins one of the shapes it has to get right — several of
//! them are shapes that would have been got wrong by a text scan, and one of them
//! (`plugins.rs`'s test-local routers, reproduced here) is a real file in this repository.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lanrurugi_api_docgen::scan::{self, HttpMethod};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway source tree. No `tempfile` dependency: the workspace does not have one, and one
/// process id plus a counter is enough to keep parallel tests apart.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(sources: &[(&str, &str)]) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "lanrurugi-docgen-scan-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&root);
        let api_src = root.join("crates/lanrurugi-api/src");
        std::fs::create_dir_all(&api_src).expect("create fixture dir");
        for (name, body) in sources {
            let path = api_src.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create fixture subdir");
            }
            std::fs::write(&path, body).expect("write fixture source");
        }
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn api_src(&self) -> PathBuf {
        self.root.join("crates/lanrurugi-api/src")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn scan_sources(sources: &[(&str, &str)]) -> anyhow::Result<scan::ScanOutput> {
    let fixture = Fixture::new(sources);
    let result = scan::scan(&fixture.api_src(), &[], fixture.root());
    // The fixture is dropped here, but the returned value owns everything it needs.
    result
}

fn scan_error(sources: &[(&str, &str)]) -> String {
    match scan_sources(sources) {
        Ok(out) => panic!("expected a scan failure, got {} route(s)", out.routes.len()),
        Err(error) => format!("{error:#}"),
    }
}

#[test]
fn literal_path_with_one_method() {
    let out = scan_sources(&[(
        "thing.rs",
        r#"
use axum::Router;
use axum::routing::get;

pub fn router() -> Router<()> {
    Router::new().route("/thing", get(list_things))
}

async fn list_things() {}
"#,
    )])
    .expect("scan");

    assert_eq!(out.routes.len(), 1);
    let route = &out.routes[0];
    assert_eq!(route.absolute_path, "/api/thing");
    assert_eq!(route.declared_path, "/thing");
    assert_eq!(route.method, HttpMethod::Get);
    assert_eq!(route.handler.as_deref(), Some("list_things"));
    assert_eq!(route.module, "thing");
    assert_eq!(route.factory, "thing::router");
}

#[test]
fn multi_line_multi_method_chain_registers_every_method() {
    let out = scan_sources(&[(
        "archives.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route(
        "/archives",
        get(list_archives).delete(batch_delete_archives),
    )
}
"#,
    )])
    .expect("scan");

    let mut found: Vec<(String, Option<String>)> = out
        .routes
        .iter()
        .map(|r| (r.method.as_str().to_string(), r.handler.clone()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            (
                "delete".to_string(),
                Some("batch_delete_archives".to_string())
            ),
            ("get".to_string(), Some("list_archives".to_string())),
        ]
    );
}

#[test]
fn crate_qualified_method_forms_are_recognised() {
    let out = scan_sources(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new()
        .route("/a", axum::routing::post(create_thing))
        .route("/b", axum::routing::get(read_thing).put(update_thing))
        .route("/c", put(replace_thing))
}
"#,
    )])
    .expect("scan");

    let mut pairs: Vec<String> = out
        .routes
        .iter()
        .map(|r| format!("{} {}", r.method.as_str(), r.absolute_path))
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            "get /api/b".to_string(),
            "post /api/a".to_string(),
            "put /api/b".to_string(),
            "put /api/c".to_string(),
        ]
    );
}

#[test]
fn route_text_inside_comments_is_ignored() {
    // A real shape: `authz.rs` and `plugins.rs` both explain route registration in prose, and a
    // text scan turns those explanations into phantom endpoints.
    let out = scan_sources(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    // Never the bare path a `router()` function's own `.route(...)` call declares.
    /// `.route("/ghost", get(ghost))` — and neither is this.
    Router::new().route("/real", get(real))
}
"#,
    )])
    .expect("scan");

    assert_eq!(out.routes.len(), 1);
    assert_eq!(out.routes[0].absolute_path, "/api/real");
}

#[test]
fn test_modules_and_test_functions_are_skipped() {
    // Reproduces the real shape of `crates/lanrurugi-api/src/plugins.rs`: a `#[cfg(test)]` module
    // builds a throwaway Router whose paths are computed at runtime. Scanning it would either
    // publish nonsense routes or fail the whole run, so skipping test code is load-bearing.
    let out = scan_sources(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/real", get(real))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_a_fake_file() {
        let name = "x.zip";
        let _router = axum::Router::new().route(
            &format!("/{name}"),
            get(|| async { "hello" }),
        );
    }
}
"#,
    )])
    .expect("scan");

    assert_eq!(out.routes.len(), 1);
    assert_eq!(out.routes[0].absolute_path, "/api/real");
}

#[test]
fn runtime_computed_path_is_rejected_with_a_location() {
    let message = scan_error(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    let prefix = "/dynamic";
    Router::new().route(prefix, get(handler))
}
"#,
    )]);

    assert!(
        message.contains("thing.rs:") && message.contains("not a string literal"),
        "message should name the file and line and say why: {message}"
    );
}

#[test]
fn method_router_without_a_known_verb_is_rejected() {
    let message = scan_error(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/thing", some_layer(handler))
}
"#,
    )]);

    assert!(
        message.contains("some_layer"),
        "message should name the unrecognised call: {message}"
    );
}

#[test]
fn any_is_rejected_rather_than_guessed() {
    let message = scan_error(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/thing", any(handler))
}
"#,
    )]);

    assert!(
        message.contains("any()"),
        "message should explain what `any()` does: {message}"
    );
}

#[test]
fn two_registrations_of_one_method_and_path_are_rejected() {
    let message = scan_error(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new()
        .route("/thing", get(first))
        .route("/thing", get(second))
}
"#,
    )]);

    assert!(
        message.contains("unreachable") && message.contains("/api/thing"),
        "message should name the duplicated route: {message}"
    );
}

#[test]
fn handler_doc_comments_are_captured() {
    let out = scan_sources(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/thing", get(list_things))
}

/// Every tracked thing, newest first.
///
/// Second paragraph.
async fn list_things() {}
"#,
    )])
    .expect("scan");

    let lookup = out.fn_lookup();
    let info = lookup
        .resolve(&out.routes[0].file, "list_things")
        .expect("handler resolved");
    assert_eq!(
        info.doc.as_deref(),
        Some("Every tracked thing, newest first.\n\nSecond paragraph.")
    );
}

#[test]
fn nested_module_paths_are_reflected_in_the_module_and_factory() {
    let out = scan_sources(&[(
        "group/mod.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/inside", get(handler))
}
"#,
    )])
    .expect("scan");

    assert_eq!(out.routes[0].module, "group");
    assert_eq!(out.routes[0].factory, "group::router");
}

#[test]
fn unrecognised_nest_prefix_fails_the_mount_check() {
    let fixture = Fixture::new(&[(
        "thing.rs",
        r#"
pub fn router() -> Router<()> {
    Router::new().route("/thing", get(handler))
}
"#,
    )]);
    let app_src = fixture.root().join("app.rs");
    std::fs::write(
        &app_src,
        r#"
fn build() -> Router {
    Router::new().nest("/api/v2", inner()).nest_service("/other", service())
}
"#,
    )
    .expect("write app.rs");

    let out = scan::scan(
        &fixture.api_src(),
        std::slice::from_ref(&app_src),
        fixture.root(),
    )
    .expect("scan");
    let error = scan::resolve_mounts(&out).expect_err("mount check should fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("/api/v2") && message.contains("/other"),
        "message should name both unknown prefixes: {message}"
    );
}
