//! `POST /plugin-wizard/lookup` (FR-001/FR-002/FR-003) — reports, per plugin type, whether an
//! installed plugin already covers the given domain. Uses `plugins::discover_namespaces`/
//! `plugins::find_plugin_by_domain` (both `pub(crate)`) — `find_plugin_by_domain` matches via each
//! candidate's `domain_match` (falling back to a loose `url_pattern`-as-domain-regex only when a
//! plugin never declares `domain_match`), NOT `find_matching_plugin`'s own precise-URL-trigger
//! matching. Using the latter here was a real, confirmed bug (2026-08-26): a wizard-generated
//! plugin whose `url_pattern` is intentionally *narrower* than its domain (e.g. `nhentai\.net/g/`,
//! requiring a real gallery path — correct for deciding when to actually fire a download) silently
//! failed to match a bare domain lookup like `"nhentai.net"`, making this endpoint report that
//! domain as uncovered even though a real plugin for it already existed.
//!
//! `login` coverage is resolved two ways, not just direct domain matching: a login plugin
//! is often relationally implied by a metadata/download plugin that already matched this domain
//! and declares `login_from: "<namespace>"` pointing back at it (e.g. `metadata/nhentai.ts`
//! declaring `login_from: "nhapiauth"`) — that link is the authoritative, always-present source of
//! "which login plugin does this domain's auth flow use", so it's checked first. Direct
//! domain matching against login candidates is still tried too (all four real login
//! plugins under `plugins/login/` do declare a domain-level `url_pattern`, mirroring their
//! metadata/download siblings) as a fallback for a login plugin with no metadata/download sibling
//! in this install at all — but relying on that alone was a real, previously-undetected bug
//! (verified live 2026-08-24: all four shipped login plugins had no `url_pattern` before this fix,
//! so `/plugin-wizard/lookup` always reported "login not covered" for e.g. `nhentai.net` even
//! though `plugins/login/nhentai.ts` already existed, and the wizard would generate a redundant
//! duplicate).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::plugins::{discover_namespaces, find_plugin_by_domain};
use crate::AppState;

const PLUGIN_TYPES: &[&str] = &["login", "metadata", "download"];

#[derive(Deserialize)]
pub(super) struct LookupRequest {
    domain: String,
}

#[derive(Serialize)]
struct TypeCoverage {
    covered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// The plugin's own self-declared `pluginInfo().namespace` — distinct from the file-path
    /// `namespace` above (same distinction `save.rs::SaveResponse.declared_namespace` documents).
    /// A *different* plugin's `login_from` field, and this wizard's own `loginAssociation`, must
    /// reference this value, never the file-path one — added specifically so the frontend can
    /// auto-associate an existing covered login plugin the moment the user answers "depends on
    /// login: yes", without needing a separate manual "associate" click first (a real reported gap,
    /// 2026-08-25: answering that question only set `dependsOnLogin`, never `loginAssociation`, so
    /// the generated code never actually declared `login_from` at all).
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_code: Option<String>,
}

impl TypeCoverage {
    fn missing() -> Self {
        Self {
            covered: false,
            namespace: None,
            declared_namespace: None,
            source_code: None,
        }
    }
}

pub(super) async fn lookup(
    State(state): State<AppState>,
    Json(req): Json<LookupRequest>,
) -> Response {
    let all_namespaces = discover_namespaces(&state.plugins_dir).await;

    // Bucket every installed plugin's namespace by its own declared `kind` — mirrors
    // `plugins.rs::list_plugins`'s own per-type grouping, just building this feature's own
    // three-key JSON shape instead of a flat list.
    let mut by_kind: std::collections::HashMap<&str, Vec<String>> = Default::default();
    for ns in &all_namespaces {
        if let Ok(info) = state.plugins.plugin_info(ns).await {
            if let Some(kind) = PLUGIN_TYPES.iter().find(|&&k| k == info.kind) {
                by_kind.entry(kind).or_default().push(ns.clone());
            }
        }
    }

    async fn coverage_for(
        state: &AppState,
        ns: &str,
        info: &lanrurugi_plugin::protocol::PluginInfo,
    ) -> TypeCoverage {
        let source_code = tokio::fs::read_to_string(state.plugins_dir.join(format!("{ns}.ts")))
            .await
            .unwrap_or_default();
        TypeCoverage {
            covered: true,
            namespace: Some(ns.to_string()),
            declared_namespace: Some(info.namespace.clone()),
            source_code: Some(source_code),
        }
    }

    // metadata/download first — login's relational fallback below needs their match results
    // (specifically, whichever one's `login_from` link) already computed.
    let mut matched_login_from: Option<String> = None;
    let mut coverage = serde_json::Map::new();
    for &kind in &["metadata", "download"] {
        let candidates = by_kind.get(kind).cloned().unwrap_or_default();
        let entry = match find_plugin_by_domain(&state, &candidates, &req.domain).await {
            Some((ns, info)) => {
                if matched_login_from.is_none() {
                    matched_login_from = info.login_from.clone();
                }
                coverage_for(&state, &ns, &info).await
            }
            None => TypeCoverage::missing(),
        };
        coverage.insert(
            kind.to_string(),
            serde_json::to_value(entry).unwrap_or(json!(null)),
        );
    }

    let login_candidates = by_kind.get("login").cloned().unwrap_or_default();
    let login_entry = match find_plugin_by_domain(&state, &login_candidates, &req.domain).await {
        Some((ns, info)) => coverage_for(&state, &ns, &info).await,
        None => match matched_login_from {
            Some(ns) if login_candidates.contains(&ns) => {
                match state.plugins.plugin_info(&ns).await {
                    Ok(info) => coverage_for(&state, &ns, &info).await,
                    Err(_) => TypeCoverage::missing(),
                }
            }
            _ => TypeCoverage::missing(),
        },
    };
    coverage.insert(
        "login".to_string(),
        serde_json::to_value(login_entry).unwrap_or(json!(null)),
    );

    (StatusCode::OK, Json(serde_json::Value::Object(coverage))).into_response()
}
