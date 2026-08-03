//! `plugins` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml`'s
//! `PluginInfo`/`OperationResponse`-derived schemas. Discovers installed plugins by recursively
//! scanning `AppState::plugins_dir` for `*.ts` files (one per namespace, organized by category —
//! `metadata/`, `login/`, `download/`, `script/`, plus a `custom/` tree for uploaded plugins) and
//! querying each one's `plugin_info` (constitution Principle IV) rather than requiring a separate
//! enable/registration step not yet modeled in Phase 1. A namespace is the file's path relative to
//! `plugins_dir`, without its `.ts` extension (`metadata/ehentai.ts` → `metadata/ehentai`) — see
//! `lanrurugi_plugin::pool::is_safe_namespace` for why multi-component namespaces are safe here.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_plugin::protocol::PluginInfo;
use lanrurugi_storage::id::ARCHIVE_ID_LEN;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::common::error;
use crate::download_manager::domain_rules::DomainRule;
use crate::download_manager::ingest::ingest_downloaded_file;
use crate::download_manager::settings::{merge, resolve_bundle_as_archive, resolve_domain_rules};
use crate::download_manager::stream::{download_one, DownloadRequest as StreamDownloadRequest};
use crate::download_manager::DownloadManager;
use crate::AppState;

/// The four categories a plugin's own `plugin_info().type` can declare — also the fixed set of
/// subdirectories under `plugins_dir` that ship in the repo (`plugins/metadata/`, `plugins/login/`,
/// `plugins/download/`, `plugins/script/`). `install_plugin` trusts *only* this list (not
/// user input) to pick the destination subdirectory for an uploaded plugin.
const PLUGIN_CATEGORIES: &[&str] = &["metadata", "login", "download", "script"];

/// Where every uploaded plugin lands, regardless of its declared category — mirrors legacy's own
/// `lib/LANraragi/Plugin/Sideloaded/` (verified: `Controller/Plugins.pm::process_upload`), which
/// also keeps user-supplied plugins physically separate from ones shipped in the repo, but nested
/// one level deeper by category (`custom/metadata/`, `custom/login/`, …) since namespaces here can
/// contain subdirectories (unlike legacy's flat `Sideloaded/`).
const CUSTOM_PLUGIN_DIR: &str = "custom";

fn plugin_method(kind: &str) -> &'static str {
    match kind {
        "metadata" => "exec_metadata",
        "login" => "exec_login",
        "download" => "exec_download",
        "script" => "exec_script",
        _ => "exec_metadata",
    }
}

/// Redis key a plugin's own configured custom values live under — legacy's `LRR_PLUGIN_<NS>` hash
/// (`~/LANraragi/lib/LANraragi/Utils/Plugins.pm:106-167`), on the same `config` logical DB legacy
/// itself uses (`Model/Config.pm::get_redis_config`) so a value configured through a legacy
/// instance sharing this Redis is read correctly here with zero migration (Principle I).
fn plugin_settings_key(namespace: &str) -> String {
    format!("LRR_PLUGIN_{}", namespace.to_uppercase())
}

/// A plugin's persisted custom-parameter values, JSON-encoded as a single array under the
/// `customargs` field — matches legacy's own `LRR_PLUGIN_<NS>` field name and encoding exactly
/// (`Controller/Plugins.pm::save_config`'s `encode_json(\@customargs)` / `Utils/Plugins.pm::
/// get_plugin_parameters`'s `decode_json($saved_config)`), positionally matching `info.parameters`
/// (`parameters[0]`'s value is `customargs[0]`, etc.) — not legacy's newer per-key HASH-style
/// storage (`to_named_params`), which no plugin in this corpus declares.
///
/// Missing/malformed stored data (nothing saved yet, or a legacy instance's differently-shaped
/// value) is treated as "no overrides" — `param_count` empty strings — rather than an error, since
/// every value here is optional free text a user may simply not have configured yet.
async fn get_plugin_customargs(
    state: &AppState,
    namespace: &str,
    param_count: usize,
) -> Vec<String> {
    let mut args = vec![String::new(); param_count];
    let Ok(mut conn) = state.redis.config.get().await else {
        return args;
    };
    let raw: Option<String> = conn
        .hget(plugin_settings_key(namespace), "customargs")
        .await
        .unwrap_or_default();
    if let Some(raw) = raw {
        if let Ok(saved) = serde_json::from_str::<Vec<String>>(&raw) {
            for (slot, value) in args.iter_mut().zip(saved) {
                *slot = value;
            }
        }
    }
    args
}

/// A plugin's persisted display-order priority within its own `type` group — lower sorts first,
/// same convention as Redis's own `ZADD` ordering (not legacy, which has no such concept at all;
/// this is a genuinely new, additive feature). Stored in the same `LRR_PLUGIN_<NS>` hash as
/// `customargs`/`enabled` under a new `priority` field. Absent for a plugin whose order was never
/// explicitly set (a fresh install, or one added after the last reorder) — `None` in that case,
/// which `list_plugins` sorts *after* every plugin with a real priority (falling back to
/// discovery order among themselves), so a newly installed plugin doesn't jump ahead of ones a
/// user already arranged.
async fn get_plugin_priority(state: &AppState, namespace: &str) -> Option<i64> {
    let mut conn = state.redis.config.get().await.ok()?;
    let raw: Option<String> = conn
        .hget(plugin_settings_key(namespace), "priority")
        .await
        .unwrap_or_default();
    raw?.parse().ok()
}

/// Runs `info`'s declared `login_from` plugin (if any) fresh and folds the resulting cookies into
/// `args["user_agent_cookies"]` — mirrors legacy's `exec_login_plugin`
/// (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:107-135`), which re-logs-in before *every*
/// metadata/download/script call rather than caching a session (there's no session-lifetime
/// concept to reuse here either, so neither host nor plugin needs to invalidate anything later).
/// `login`-type plugins are never login'd into themselves (`info.kind == "login"` short-circuits
/// immediately) and a login failure only logs a warning — the main plugin call still goes ahead,
/// just without any cookies, the same as legacy falling back to a blank `Mojo::UserAgent->new`
/// when its own login attempt didn't return a real user agent.
async fn with_login_cookies(state: &AppState, info: &PluginInfo, mut args: Value) -> Value {
    if info.kind == "login" {
        return args;
    }
    let Some(login_from) = &info.login_from else {
        return args;
    };
    let Some((login_ns, login_info)) = resolve_declared_namespace(state, login_from).await else {
        tracing::warn!(
            declared_login_from = %login_from,
            "no installed plugin declares this login_from namespace, continuing without a logged-in user agent"
        );
        return args;
    };
    let customargs = get_plugin_customargs(state, &login_ns, login_info.parameters.len()).await;
    let login_args = json!({ "customargs": customargs });
    match state
        .plugins
        .execute(&login_ns, "exec_login", login_args)
        .await
    {
        Ok(result) => {
            if let Some(cookies) = result.get("cookies") {
                args["user_agent_cookies"] = cookies.clone();
            }
        }
        Err(e) => {
            tracing::warn!(
                login_plugin = %login_ns,
                error = %e,
                "login plugin failed, continuing without a logged-in user agent"
            );
        }
    }
    args
}

/// Resolves `info.sidecar_files` (see its own docs) against the archive at `file_path`, adding
/// whatever it finds to `args["sidecar_files"]` as `{ filename: content }` before the plugin ever
/// runs — the plugin itself never gets real filesystem access for this (constitution Principle
/// IV: only these specific, host-mediated file contents, nothing broader). A file that's missing,
/// unreadable, or not valid UTF-8 is just skipped with a warning rather than failing the whole
/// call — matches legacy's own `is_file_in_archive` returning nothing found being a normal,
/// expected outcome for plugins that only *optionally* enrich metadata from a sidecar file.
fn with_sidecar_files(info: &PluginInfo, mut args: Value, file_path: Option<&str>) -> Value {
    if info.sidecar_files.is_empty() {
        return args;
    }
    let Some(file_path) = file_path else {
        return args;
    };
    let path = std::path::Path::new(file_path);
    let mut found = serde_json::Map::new();
    for wanted in &info.sidecar_files {
        let entry_name = match lanrurugi_scanner::archive_format::find_entry_by_suffix(path, wanted)
        {
            Ok(Some(name)) => name,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(wanted, error = %e, "failed to search archive for sidecar file");
                continue;
            }
        };
        match lanrurugi_scanner::archive_format::read_entry(path, &entry_name) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    found.insert(wanted.clone(), json!(text));
                }
                Err(e) => tracing::warn!(wanted, error = %e, "sidecar file wasn't valid UTF-8"),
            },
            Err(e) => {
                tracing::warn!(wanted, error = %e, "failed to read sidecar file from archive")
            }
        }
    }
    if !found.is_empty() {
        args["sidecar_files"] = Value::Object(found);
    }
    args
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugins/{type}", get(list_plugins))
        .route("/plugins/use", post(use_plugin_sync))
        .route("/plugins/queue", post(use_plugin_async))
        .route(
            // `namespace` moved from a path segment to a query parameter — axum's catch-all path
            // segments (`{*namespace}`, needed since a namespace may now contain `/` like
            // `metadata/ehentai`) are only allowed at the very end of a route, so a trailing
            // `/settings` after it isn't expressible as a path. Not a breaking change: this
            // endpoint has no legacy contract equivalent (additive-only) and isn't wired into the
            // frontend yet (see `Plugins.tsx`'s own comment on this).
            "/plugins/settings",
            get(get_plugin_settings).put(put_plugin_settings),
        )
        .route("/plugins/priority", post(put_plugin_priority))
        .route("/download_url", post(download_url))
        .route("/plugins/upload", post(upload_plugin))
        .route(
            // `namespace` as a query parameter, not a `{namespace}/options` path segment, for the
            // same reason `/plugins/settings` above is: a namespace may itself contain `/`
            // (`download/pixiv`), which a trailing path segment after it can't express with
            // axum's routing (catch-all segments are only allowed at the very end of a route).
            "/plugins/options",
            get(get_plugin_options)
                .put(put_plugin_options)
                .delete(delete_plugin_options),
        )
}

#[derive(Debug, Deserialize)]
pub struct PluginSettingsQuery {
    namespace: String,
}

/// `GET /plugins/settings?namespace=...` — the plugin's currently persisted `customargs` (padded/
/// truncated to exactly its own declared `parameters.len()` — a stale saved array longer/shorter
/// than the plugin's current declaration, e.g. after the plugin was upgraded to add/remove a
/// parameter, is silently reconciled to the current shape rather than surfaced as a mismatch,
/// matching legacy's own default-value-fill-then-overwrite behavior in `get_plugin_parameters`)
/// and `enabled` (legacy's "Run Automatically" toggle — `is_plugin_enabled`, `false` when never
/// set, same as legacy's own `hexists`-gated default).
async fn get_plugin_settings(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
) -> Response {
    let param_count = match state.plugins.plugin_info(&query.namespace).await {
        Ok(info) => info.parameters.len(),
        Err(e) => {
            return error(StatusCode::NOT_FOUND, "get_plugin_settings", e.to_string());
        }
    };
    let customargs = get_plugin_customargs(&state, &query.namespace, param_count).await;
    let enabled = get_plugin_enabled(&state, &query.namespace).await;
    axum::Json(json!({ "customargs": customargs, "enabled": enabled })).into_response()
}

async fn get_plugin_enabled(state: &AppState, namespace: &str) -> bool {
    let Ok(mut conn) = state.redis.config.get().await else {
        return false;
    };
    let value: Option<String> = conn
        .hget(plugin_settings_key(namespace), "enabled")
        .await
        .unwrap_or_default();
    value.as_deref() == Some("1")
}

#[derive(Debug, Deserialize)]
pub struct PutPluginSettingsBody {
    /// `None`/absent leaves the stored `customargs` untouched — a partial update (unlike legacy's
    /// own single-form-submits-everything page, this app saves the "Run Automatically" toggle and
    /// the parameter form independently, so a toggle flip must not require resending the other's
    /// current value just to avoid clobbering it).
    customargs: Option<Vec<String>>,
    enabled: Option<bool>,
}

async fn put_plugin_settings(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
    axum::Json(body): axum::Json<PutPluginSettingsBody>,
) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_plugin_settings",
                e.to_string(),
            )
        }
    };
    let key = plugin_settings_key(&query.namespace);
    let mut fields: Vec<(&str, String)> = Vec::new();
    if let Some(customargs) = &body.customargs {
        let encoded = match serde_json::to_string(customargs) {
            Ok(s) => s,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "put_plugin_settings",
                    e.to_string(),
                )
            }
        };
        fields.push(("customargs", encoded));
    }
    if let Some(enabled) = body.enabled {
        fields.push(("enabled", if enabled { "1" } else { "0" }.to_string()));
    }
    if fields.is_empty() {
        return axum::Json(json!({ "operation": "put_plugin_settings", "success": 1 }))
            .into_response();
    }
    let result: Result<(), _> = conn.hset_multiple(&key, &fields).await;
    match result {
        Ok(()) => {
            axum::Json(json!({ "operation": "put_plugin_settings", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "put_plugin_settings",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutPluginPriorityBody {
    /// This body's own `type` — matched against every included namespace's *actual* declared
    /// `plugin_info().type` before writing anything, so a stale/forged request naming a plugin of
    /// a different type than claimed can't silently reorder it into the wrong group.
    #[serde(rename = "type")]
    kind: String,
    /// The complete, newly-ordered namespace list for `type` (every plugin of that type the
    /// client currently has rendered, front-to-back) — not a partial move-this-one-item delta, to
    /// keep the write dead simple: each namespace's `priority` becomes its index in this array.
    order: Vec<String>,
}

/// `PUT /plugins/priority` — persists a drag-and-drop reorder of one plugin `type` group's display
/// order (additive, no legacy equivalent: legacy's `plugins.html.tt2` has no such affordance at
/// all). Rewrites every listed namespace's `priority` field in its own `LRR_PLUGIN_<NS>` hash
/// (same storage as `customargs`/`enabled`) to its position in `order`, so a subsequent
/// `GET /plugins/{type}` sorts accordingly. A namespace whose actual type doesn't match `body.type`
/// is skipped (not written, not erroring the whole request) — see `PutPluginPriorityBody::kind`'s
/// own docs for why this check exists at all.
async fn put_plugin_priority(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<PutPluginPriorityBody>,
) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_plugin_priority",
                e.to_string(),
            )
        }
    };
    let mut skipped = Vec::new();
    for (index, namespace) in body.order.iter().enumerate() {
        let matches_type = state
            .plugins
            .plugin_info(namespace)
            .await
            .map(|info| info.kind == body.kind)
            .unwrap_or(false);
        if !matches_type {
            skipped.push(namespace.clone());
            continue;
        }
        let key = plugin_settings_key(namespace);
        let result: Result<(), _> = conn.hset(&key, "priority", index.to_string()).await;
        if let Err(e) = result {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_plugin_priority",
                e.to_string(),
            );
        }
    }
    axum::Json(json!({ "operation": "put_plugin_priority", "success": 1, "skipped": skipped }))
        .into_response()
}

/// Fetches `namespace`'s declared `pluginOptions()` fresh from the plugin, mapping "plugin exports
/// no such function" (`Ok(None)`) and "namespace doesn't exist at all" (`Err`) onto the same `404`
/// (spec FR-015 / contracts/download-settings-api.md — the two cases are indistinguishable from
/// the caller's point of view: no settings interface either way).
async fn fetch_declared_options(
    state: &AppState,
    namespace: &str,
) -> Option<lanrurugi_plugin::protocol::PluginOptionsResult> {
    state.plugins.plugin_options(namespace).await.ok().flatten()
}

async fn get_plugin_options(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
) -> Response {
    let Some(declared) = fetch_declared_options(&state, &query.namespace).await else {
        return error(
            StatusCode::NOT_FOUND,
            "get_plugin_options",
            "plugin does not exist or declares no configurable options",
        );
    };
    let override_ = match state.plugin_options.get(&query.namespace).await {
        Ok(o) => o,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_plugin_options",
                e.to_string(),
            )
        }
    };
    let effective = merge(&query.namespace, &declared, override_.as_ref());
    axum::Json(effective).into_response()
}

#[derive(Debug, Deserialize)]
pub struct PutPluginOptionsBody {
    #[serde(default)]
    domain_rules: Option<Vec<lanrurugi_storage::plugin_options::DomainRuleOverride>>,
    #[serde(default)]
    bundle_as_archive: Option<bool>,
    #[serde(default)]
    overwrite_on_duplicate: Option<bool>,
}

/// FR-014: a concurrency/rate-limit value, if present at all, must be a positive integer — a `0`
/// or the field being present-but-absent-in-spirit isn't silently clamped/dropped, it's rejected
/// with a field-level message identifying exactly which rule and field failed.
fn validate_domain_rules(
    rules: &[lanrurugi_storage::plugin_options::DomainRuleOverride],
) -> Result<(), (String, String)> {
    for (i, rule) in rules.iter().enumerate() {
        if let Some(0) = rule.max_concurrent {
            return Err((
                "max_concurrent must be a positive integer".to_string(),
                format!("domain_rules[{i}].max_concurrent"),
            ));
        }
        if let Some(0) = rule.max_bytes_per_sec {
            return Err((
                "max_bytes_per_sec must be a positive integer".to_string(),
                format!("domain_rules[{i}].max_bytes_per_sec"),
            ));
        }
    }
    Ok(())
}

async fn put_plugin_options(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
    axum::Json(body): axum::Json<PutPluginOptionsBody>,
) -> Response {
    let Some(declared) = fetch_declared_options(&state, &query.namespace).await else {
        return error(
            StatusCode::NOT_FOUND,
            "put_plugin_options",
            "plugin does not exist or declares no configurable options",
        );
    };

    if let Some(rules) = &body.domain_rules {
        if let Err((message, field)) = validate_domain_rules(rules) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(json!({ "error": message, "field": field })),
            )
                .into_response();
        }
    }

    // Partial update (contract: "a field omitted from the request body is left at its current
    // effective value") — start from whatever's already stored, then overwrite only the fields
    // this request actually provided.
    let mut override_ = state
        .plugin_options
        .get(&query.namespace)
        .await
        .unwrap_or_default()
        .unwrap_or_default();
    if body.domain_rules.is_some() {
        override_.domain_rules = body.domain_rules;
    }
    if body.bundle_as_archive.is_some() {
        override_.bundle_as_archive = body.bundle_as_archive;
    }
    if body.overwrite_on_duplicate.is_some() {
        override_.overwrite_on_duplicate = body.overwrite_on_duplicate;
    }

    if let Err(e) = state
        .plugin_options
        .save(&query.namespace, &override_)
        .await
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "put_plugin_options",
            e.to_string(),
        );
    }

    let effective = merge(&query.namespace, &declared, Some(&override_));
    axum::Json(effective).into_response()
}

async fn delete_plugin_options(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
) -> Response {
    let Some(declared) = fetch_declared_options(&state, &query.namespace).await else {
        return error(
            StatusCode::NOT_FOUND,
            "delete_plugin_options",
            "plugin does not exist or declares no configurable options",
        );
    };
    if let Err(e) = state.plugin_options.delete(&query.namespace).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_plugin_options",
            e.to_string(),
        );
    }
    let effective = merge(&query.namespace, &declared, None);
    axum::Json(effective).into_response()
}

/// Resolves a plugin's *self-declared* namespace (e.g. `"ehlogin"`, `info.login_from`'s value,
/// hand-written straight into a plugin's own `pluginInfo()` — legacy source verbatim, since
/// legacy's flat plugin directory made the declared and file-path namespaces always identical) to
/// the actual file-discovery-path namespace (`"login/ehentai"`) every real host call
/// (`PluginPool::plugin_info`/`execute`) needs — same root cause `list_plugins` works around for
/// its own `namespace` response field, but `login_from` bakes the declared value directly into
/// plugin source, so it must be resolved fresh at call time instead. Scans every installed plugin
/// for one whose own `plugin_info().namespace` matches; `None` if none does (a plugin points at a
/// `login_from` that isn't actually installed).
async fn resolve_declared_namespace(
    state: &AppState,
    declared_namespace: &str,
) -> Option<(String, PluginInfo)> {
    for ns in discover_namespaces(&state.plugins_dir).await {
        if let Ok(info) = state.plugins.plugin_info(&ns).await {
            if info.namespace == declared_namespace {
                return Some((ns, info));
            }
        }
    }
    None
}

/// Recursively walks `plugins_dir` (iteratively, via an explicit directory queue — not `async fn`
/// self-recursion, which Rust can't size without boxing) collecting every `.ts` file's path
/// relative to `plugins_dir`, with the extension stripped and components joined by `/` — that
/// string is the namespace (`metadata/ehentai.ts` → `"metadata/ehentai"`, and a plugin still
/// directly under `plugins_dir` with no category subfolder is just `"foo"`, unchanged from
/// before). Depth is unbounded: `custom/` (uploaded plugins) may itself have subdirectories.
async fn discover_namespaces(plugins_dir: &std::path::Path) -> Vec<String> {
    let mut namespaces = Vec::new();
    let mut dirs_to_visit = vec![plugins_dir.to_path_buf()];

    while let Some(dir) = dirs_to_visit.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                dirs_to_visit.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("ts") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(plugins_dir) else {
                continue;
            };
            let without_ext = relative.with_extension("");
            let Some(namespace) = without_ext.to_str() else {
                continue;
            };
            // Windows paths would join with `\`; every deployment target here is Linux
            // (constitution's Debian-slim runtime), but normalize defensively anyway since the
            // namespace string is also what gets sent over HTTP and matched against routes.
            namespaces.push(namespace.replace(std::path::MAIN_SEPARATOR, "/"));
        }
    }
    namespaces
}

async fn list_plugins(State(state): State<AppState>, Path(kind): Path<String>) -> Response {
    let namespaces = discover_namespaces(&state.plugins_dir).await;
    // discovery order, per-type — the fallback ordering for any plugin with no explicit priority
    // (see `get_plugin_priority`'s own docs), and the tiebreak among several such plugins.
    let mut discovery_index: std::collections::HashMap<String, usize> = Default::default();
    let mut per_type_counter: std::collections::HashMap<String, usize> = Default::default();

    let mut plugins = Vec::new();
    for ns in namespaces {
        if let Ok(info) = state.plugins.plugin_info(&ns).await {
            if kind == "all" || info.kind == kind {
                let counter = per_type_counter.entry(info.kind.clone()).or_insert(0);
                discovery_index.insert(ns.clone(), *counter);
                *counter += 1;

                let priority = get_plugin_priority(&state, &ns).await;
                plugins.push(json!({
                    // The file-discovery-path namespace (`ns`, e.g. `download/pixiv`) — not the
                    // plugin's own self-declared `info.namespace` (e.g. `pixivdl`, legacy's
                    // `plugin_info()` `namespace` field). Every other endpoint that takes a
                    // `namespace`/`plugin` parameter (`/plugins/use`, `/plugins/options`,
                    // `/plugins/settings`) resolves it straight through `discover_namespaces`'s
                    // own path-based scheme (`PluginPool::plugin_info`/`execute` join it onto
                    // `plugins_dir` as `{namespace}.ts`), so returning the *declared* value here
                    // instead would silently 404 every one of those calls — a real, previously
                    // shipped bug this fixes, not a hypothetical one (confirmed via a live
                    // container: `POST /plugins/use?plugin=pixivdl` 404s, `plugin=download/pixiv`
                    // works). Unlike legacy (one flat plugin directory, so the two values were
                    // always identical there), this rewrite's category subfolders/`custom/` tree
                    // make them genuinely different strings.
                    "namespace": ns,
                    "type": info.kind,
                    "name": info.name,
                    "author": info.author,
                    "description": info.description,
                    "version": info.version,
                    "icon": info.icon,
                    "oneshot_arg": info.oneshot_arg,
                    // The plugin's own self-declared login_from value (e.g. "ehlogin") — display
                    // only here (legacy's "This plugin depends on the login plugin ..." note,
                    // `plugins.html.tt2` line 143-145); the real call-time resolution to a
                    // file-path namespace happens in `resolve_declared_namespace`, not here.
                    "login_from": info.login_from,
                    "url_pattern": info.url_pattern,
                    "priority": priority,
                    "parameters": info.parameters.iter().map(|p| json!({
                        "name": p.name,
                        "desc": p.description,
                        "type": p.param_type,
                    })).collect::<Vec<_>>(),
                }));
            }
        }
    }

    // Sort by (type, priority-or-discovery-order) so the response is already in the order the
    // Plugins page's own drag-to-reorder should display, and `findMatchingPlugin`-style callers
    // just take the first url_pattern match within a type rather than needing their own sort.
    plugins.sort_by(|a, b| {
        let type_a = a["type"].as_str().unwrap_or("");
        let type_b = b["type"].as_str().unwrap_or("");
        type_a.cmp(type_b).then_with(|| {
            let ns_a = a["namespace"].as_str().unwrap_or("");
            let ns_b = b["namespace"].as_str().unwrap_or("");
            let rank = |v: &Value, ns: &str| {
                v.as_i64()
                    .unwrap_or_else(|| i64::MAX / 2 + *discovery_index.get(ns).unwrap_or(&0) as i64)
            };
            rank(&a["priority"], ns_a).cmp(&rank(&b["priority"], ns_b))
        })
    });

    axum::Json(plugins).into_response()
}

#[derive(Debug, Deserialize)]
pub struct UsePluginParams {
    id: Option<String>,
    plugin: Option<String>,
    arg: Option<String>,
}

/// The small, deliberately narrow subset of `LRR_CONFIG` values a metadata plugin might need to
/// consult (currently just `plugins/metadata/dateadded.ts`'s `usedateadded`/`usedatemodified`) —
/// passed to every metadata/download/script call under `args["settings"]` rather than the plugin
/// making its own round trip back into `GET /settings`, and rather than handing over the *entire*
/// settings hash (which includes things like `apikey` a plugin has no business reading).
async fn get_plugin_relevant_settings(state: &AppState) -> Value {
    let Ok(mut conn) = state.redis.config.get().await else {
        return json!({});
    };
    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    json!({
        "usedateadded": fields.get("usedateadded").map(|v| v != "0").unwrap_or(true),
        "usedatemodified": fields.get("usedatemodified").map(|v| v != "0").unwrap_or(false),
    })
}

fn extract_archive_id(oneshot: &str) -> Option<String> {
    if oneshot.len() < ARCHIVE_ID_LEN {
        return None;
    }
    let lower = oneshot.to_lowercase();
    let hex_run: String = lower
        .chars()
        .skip_while(|c| !c.is_ascii_hexdigit())
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (hex_run.len() == ARCHIVE_ID_LEN).then_some(hex_run)
}

/// `plugins/metadata/copyarchivetags.ts`'s one real need: another archive's stored `tags` string,
/// looked up by an ID this plugin extracts from its own oneshot `arg` (not `params.id`, which is
/// the *current* archive) — resolved host-side since a Deno-sandboxed plugin has no direct storage
/// access (mirrors `LANraragi::Utils::Database::get_tags`, a plain by-ID field read).
async fn get_other_archive_tags(state: &AppState, plugin: &str, arg: Option<&str>) -> Value {
    if plugin != "metadata/copyarchivetags" {
        return Value::Null;
    }
    let Some(other_id) = arg.and_then(extract_archive_id) else {
        return Value::Null;
    };
    match state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(other_id))
        .await
    {
        Ok(Some(archive)) => json!(archive.tags),
        _ => Value::Null,
    }
}

/// Normalizes a URL the same way both `plugins/script/sourcefinder.ts` (via
/// `get_existing_archive_id_for_url` below) and legacy's `SourceFinder.pm::run_script` do before
/// comparing it against a stored `source:` tag — strips scheme, `www.`, any query string, and a
/// trailing slash.
pub(crate) fn trim_url(url: &str) -> String {
    let url = url.trim();
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let without_www = without_scheme
        .strip_prefix("www.")
        .unwrap_or(without_scheme);
    let without_query = without_www.split('?').next().unwrap_or(without_www);
    without_query.trim_end_matches('/').to_string()
}

/// `plugins/script/sourcefinder.ts`'s one real need: the ID of whichever archive (if any) has a
/// `source:` tag matching `url`, including the E-Hentai/ExHentai domain-alias special case
/// (`SourceFinder.pm::run_script`'s own two extra branches) — resolved host-side the same way
/// `get_other_archive_tags` resolves `copyarchivetags`' need, since a Deno-sandboxed plugin has no
/// direct storage access. Legacy maintains a dedicated `LRR_URLMAP` index for this; this scans
/// every archive's tags on demand instead (same simplification already used by `GET
/// /database/stats` — correct, just not index-accelerated, acceptable at personal-library scale).
async fn get_existing_archive_id_for_url(
    state: &AppState,
    plugin: &str,
    url: Option<&str>,
) -> Value {
    if plugin != "script/sourcefinder" {
        return Value::Null;
    }
    let Some(url) = url else {
        return Value::Null;
    };
    let trimmed = trim_url(url);
    if trimmed.is_empty() {
        return Value::Null;
    }

    let mut candidates = vec![trimmed.clone()];
    if let Some(rest) = trimmed.strip_prefix("exhentai.org/") {
        candidates.push(format!("e-hentai.org/{rest}"));
    } else if let Some(rest) = trimmed.strip_prefix("e-hentai.org/") {
        candidates.push(format!("exhentai.org/{rest}"));
    }

    let Ok(archives) = state.repos.archives.list_all().await else {
        return Value::Null;
    };
    for archive in &archives {
        for tag in archive.tags.split(',') {
            let Some(source) = tag.trim().strip_prefix("source:") else {
                continue;
            };
            if candidates.iter().any(|c| c == &trim_url(source)) {
                return json!(archive.id);
            }
        }
    }
    Value::Null
}

/// `plugins/script/nhentaisourceconverter.ts`'s one real need: every archive's `id`/`tags`, so it
/// can compute its own tag rewrite purely in JS and hand the result back rather than touching
/// storage directly — resolved host-side the same way the other two `script/*`-specific helpers
/// above are. Deliberately whole-library (not scoped to one archive, unlike every `metadata`-type
/// helper), matching what `nHentaiSourceConverter.pm::run_script` itself operates over.
async fn get_all_archive_tags_for_script(state: &AppState, plugin: &str) -> Value {
    if plugin != "script/nhentaisourceconverter" {
        return Value::Null;
    }
    match state.repos.archives.list_all().await {
        Ok(archives) => json!(archives
            .into_iter()
            .map(|a| json!({ "id": a.id, "tags": a.tags }))
            .collect::<Vec<_>>()),
        Err(_) => Value::Null,
    }
}

/// Applies `plugins/script/nhentaisourceconverter.ts`'s returned `updates` (see `ScriptResult` in
/// `plugin-sdk.ts`) back onto real archive records — the host's half of the read-compute-write
/// split described in that same SDK doc comment, since the plugin itself only ever returns its
/// *intended* rewrites, never writes storage directly.
async fn apply_script_tag_updates(state: &AppState, plugin: &str, data: &Value) {
    if plugin != "script/nhentaisourceconverter" {
        return;
    }
    let Some(updates) = data.get("updates").and_then(Value::as_array) else {
        return;
    };
    for update in updates {
        let (Some(id), Some(tags)) = (
            update.get("id").and_then(Value::as_str),
            update.get("tags").and_then(Value::as_str),
        ) else {
            continue;
        };
        if let Ok(Some(mut archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.to_string()))
            .await
        {
            archive.tags = tags.to_string();
            let _ = state.repos.archives.save(&archive).await;
        }
    }
}

/// `plugins/script/foldertocat.ts`'s two real needs, resolved host-side since a Deno-sandboxed
/// plugin has no direct storage access: the library's own archive-directory root (so it can walk
/// the real filesystem itself via `Deno.readDir` — see that file's own docs for why the host
/// doesn't pre-walk it instead), and a `path -> archive id` map (so the plugin can resolve the
/// files it finds back to real archive IDs before returning its computed category groupings).
async fn get_foldertocat_args(state: &AppState, plugin: &str) -> (Value, Value) {
    if plugin != "script/foldertocat" {
        return (Value::Null, Value::Null);
    }
    let library_path = json!(state.library.archive_dir.display().to_string());
    let archive_id_by_path = match state.repos.archives.list_all().await {
        Ok(archives) => {
            let map: serde_json::Map<String, Value> = archives
                .into_iter()
                .map(|a| (a.file, json!(a.id)))
                .collect();
            Value::Object(map)
        }
        Err(_) => Value::Null,
    };
    (library_path, archive_id_by_path)
}

/// Applies `plugins/script/foldertocat.ts`'s returned `categories_to_create`/
/// `delete_old_categories` (see `ScriptResult` in `plugin-sdk.ts`) — the host's half of the
/// read-compute-return-write-to-host split, mirroring
/// `lanrurugi-api::scripts::subfolders_to_categories`'s own category-creation logic exactly (same
/// `SET_<unix-seconds>` catid generation) so the two implementations produce identical results,
/// differing only in how the directory walk itself was performed.
async fn apply_foldertocat_categories(state: &AppState, plugin: &str, data: &Value) {
    if plugin != "script/foldertocat" {
        return;
    }
    if data.get("delete_old_categories").and_then(Value::as_bool) == Some(true) {
        if let Ok(categories) = state.repos.categories.list_all().await {
            for category in categories.iter().filter(|c| c.search.is_none()) {
                let _ = state.repos.categories.delete(&category.catid).await;
            }
        }
    }

    let Some(to_create) = data.get("categories_to_create").and_then(Value::as_array) else {
        return;
    };
    let mut next_candidate = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for entry in to_create {
        let (Some(name), Some(archive_ids)) = (
            entry.get("name").and_then(Value::as_str),
            entry.get("archive_ids").and_then(Value::as_array),
        ) else {
            continue;
        };
        let mut catid = lanrurugi_core::ids::CategoryId(format!("SET_{next_candidate}"));
        while state
            .repos
            .categories
            .get(&catid)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            next_candidate += 1;
            catid = lanrurugi_core::ids::CategoryId(format!("SET_{next_candidate}"));
        }
        next_candidate += 1;

        let category = lanrurugi_core::entities::Category {
            catid,
            name: name.to_string(),
            search: None,
            archives: archive_ids
                .iter()
                .filter_map(Value::as_str)
                .map(|s| lanrurugi_core::ids::ArchiveId(s.to_string()))
                .collect(),
            pinned: false,
        };
        let _ = state.repos.categories.save(&category).await;
    }
}

/// Result of [`run_enabled_metadata_plugins_on_archive`] — mirrors the 4-tuple legacy's own
/// `exec_enabled_plugins_on_file` returns (`Model/Plugins.pm`), used for the same "N plugins
/// used successfully, M failed, K tags added" style summary message every real ingestion path
/// (upload, download, scan) surfaces.
pub struct AutoPluginSummary {
    pub successes: u32,
    pub failures: u32,
    pub added_tags: u32,
    /// The last plugin-returned non-empty title, if any (later plugins in run order win, matching
    /// legacy's own `$newtitle` being reassigned on every plugin that returns one).
    pub new_title: Option<String>,
}

/// Legacy's real, load-bearing "自动运行"/"Run Automatically" mechanism (`Model::Plugins::
/// exec_enabled_plugins_on_file`, called by `Model::Upload.pm` right after every new upload/
/// download is cataloged, and by the watcher after every scanned file) — runs every metadata
/// plugin with `enabled: true` against a freshly-cataloged archive, in priority order, with the
/// filename-parsing plugin (`regexplugin`) always bumped to run first when present (legacy's own
/// `TODO: Make plugin exec order configurable` comment, preserved verbatim — it still isn't).
/// This closes a real Phase 1 gap: nothing in this port called anything like this at all, so
/// every enabled metadata plugin's "auto-run" checkbox was silently inert on every ingestion path
/// (confirmed live: two fully-downloaded E-Hentai archives never got a `source:` tag or any other
/// metadata a plugin would have added, because nothing ever invoked one).
///
/// Reuses the exact same argument shape [`use_plugin_sync`] builds for an id-only (no `arg`/
/// `oneshot_param`) call — that's already the args a metadata plugin needs (`archive_id`,
/// `file_path`, `customargs`, sidecar files, login cookies, ...); this only adds the plugin-
/// selection/ordering and the tag/title merge-and-persist step `use_plugin_sync` deliberately
/// leaves to its caller (its own callers include the interactive "Fetch Metadata" button, which
/// must NOT auto-persist before the user reviews the result).
pub async fn run_enabled_metadata_plugins_on_archive(
    state: &AppState,
    archive_id: &str,
) -> AutoPluginSummary {
    let mut summary = AutoPluginSummary {
        successes: 0,
        failures: 0,
        added_tags: 0,
        new_title: None,
    };

    let mut namespaces = Vec::new();
    for ns in discover_namespaces(&state.plugins_dir).await {
        let Ok(info) = state.plugins.plugin_info(&ns).await else {
            continue;
        };
        if info.kind != "metadata" {
            continue;
        }
        if !get_plugin_enabled(state, &ns).await {
            continue;
        }
        namespaces.push((ns, info));
    }
    // `regexplugin` (declared namespace, not necessarily `metadata/regexparse` — a custom-
    // installed clone could sit at a different file path) always runs first when enabled, exactly
    // matching legacy's own reordering.
    if let Some(regex_pos) = namespaces
        .iter()
        .position(|(_, info)| info.namespace == "regexplugin")
    {
        let regex_entry = namespaces.remove(regex_pos);
        namespaces.insert(0, regex_entry);
    }

    for (ns, info) in namespaces {
        // Fetched fresh on every plugin iteration (not hoisted above the loop) since an earlier
        // plugin in this same run may have just updated `title`/`tags` — the next plugin should
        // see that plugin's own output as its "existing" state, matching legacy's own sequential
        // `set_tags(..., append=1)` behavior across `exec_enabled_plugins_on_file`'s loop.
        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
            .await
            .ok()
            .flatten();
        let file_path = archive.as_ref().map(|a| a.file.clone());
        let file_modified_time = file_path.as_deref().and_then(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
        });
        let customargs = get_plugin_customargs(state, &ns, info.parameters.len()).await;
        let args = json!({
            "archive_id": archive_id,
            "arg": null,
            "oneshot_param": null,
            "customargs": customargs,
            "file_path": file_path,
            "file_modified_time": file_modified_time,
            "settings": get_plugin_relevant_settings(state).await,
            "existing_tags": archive.as_ref().map(|a| a.tags.clone()).unwrap_or_default(),
            "archive_title": archive.as_ref().map(|a| a.title.clone()).unwrap_or_default(),
        });
        let args = with_sidecar_files(&info, args, file_path.as_deref());
        let args = with_login_cookies(state, &info, args).await;

        let data = match state
            .plugins
            .execute(&ns, plugin_method(&info.kind), args)
            .await
        {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(%archive_id, plugin = %ns, error = %e, "auto-run metadata plugin failed");
                summary.failures += 1;
                continue;
            }
        };
        if let Some(err) = data.get("error").and_then(Value::as_str) {
            tracing::warn!(%archive_id, plugin = %ns, error = %err, "auto-run metadata plugin returned an error");
            summary.failures += 1;
            continue;
        }

        let Ok(Some(mut archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
            .await
        else {
            summary.failures += 1;
            continue;
        };
        let old_title = archive.title.clone();
        let old_tags = archive.tags.clone();

        // `set_tags($id, $newtags, 1)`'s real append+dedupe semantics (`Utils/Database.pm`) — new
        // tags are appended after the existing ones, then deduplicated (first occurrence wins),
        // not a blind overwrite.
        if let Some(new_tags) = data.get("tags").and_then(Value::as_str) {
            if !new_tags.trim().is_empty() {
                let mut seen = std::collections::HashSet::new();
                let mut merged = Vec::new();
                for t in old_tags
                    .split(',')
                    .chain(new_tags.split(','))
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    if seen.insert(t.to_string()) {
                        merged.push(t.to_string());
                    }
                }
                summary.added_tags +=
                    new_tags.split(',').filter(|t| !t.trim().is_empty()).count() as u32;
                archive.tags = merged.join(",");
            }
        }
        if let Some(title) = data.get("title").and_then(Value::as_str) {
            if !title.trim().is_empty() {
                archive.title = title.to_string();
                summary.new_title = Some(title.to_string());
            }
        }
        if let Some(summary_text) = data.get("summary").and_then(Value::as_str) {
            if !summary_text.trim().is_empty() {
                archive.summary = summary_text.to_string();
            }
        }

        match state.repos.archives.save(&archive).await {
            Ok(()) => {
                summary.successes += 1;
                if archive.title != old_title {
                    if let Err(e) = lanrurugi_search::indexer::update_title_index(
                        &state.redis.search,
                        archive_id,
                        &old_title,
                        &archive.title,
                    )
                    .await
                    {
                        tracing::warn!(%archive_id, error = %e, "failed to update title search index");
                    }
                }
                if archive.tags != old_tags {
                    if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
                        &state.redis.search,
                        archive_id,
                        &old_tags,
                        &archive.tags,
                    )
                    .await
                    {
                        tracing::warn!(%archive_id, error = %e, "failed to update tag search index");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(%archive_id, plugin = %ns, error = %e, "failed to persist auto-run plugin's metadata");
                summary.failures += 1;
            }
        }
    }

    summary
}

async fn use_plugin_sync(
    State(state): State<AppState>,
    Query(params): Query<UsePluginParams>,
) -> Response {
    let Some(plugin) = params.plugin else {
        return error(
            StatusCode::BAD_REQUEST,
            "use_plugin",
            "No plugin specified.",
        );
    };

    let info = match state.plugins.plugin_info(&plugin).await {
        Ok(i) => i,
        Err(e) => {
            return axum::Json(json!({
                "operation": "use_plugin",
                "success": 0,
                "error": e.to_string(),
            }))
            .into_response()
        }
    };

    // Metadata plugins that derive tags from the filename (e.g. "Filename Parsing") need the
    // archive's actual on-disk path, not just its ID — fetch it once here rather than requiring
    // every such plugin to make its own round trip back into the API for something the host
    // already knows. Also the source of `existing_tags`/`archive_title` below: several real
    // converted plugins (`chaika`/`ehentai`/`fakku`/`hitomi`/`mems`/`nhentai`/`pixiv`) fall back to
    // parsing a `source:`-style tag or an embedded ID out of the archive's own current tags/title
    // when no `arg`/oneshot URL was given — matching legacy's own `exec_metadata_plugin`
    // (`Model/Plugins.pm`), whose `%infohash` always includes both. Missing entirely before this
    // (confirmed live: every one of those plugins crashed with "Cannot read properties of
    // undefined" whenever called without an explicit `arg`, including every auto-run invocation).
    let queried_archive = match &params.id {
        Some(id) => state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let file_path = queried_archive.as_ref().map(|a| a.file.clone());

    // The file's own last-modified time (Unix seconds), resolved host-side the same way
    // `file_path` already is — so `plugins/metadata/dateadded.ts` (the one real consumer) doesn't
    // need its own filesystem read permission just for this one `stat` call.
    let file_modified_time = file_path.as_deref().and_then(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    });

    let customargs = get_plugin_customargs(&state, &plugin, info.parameters.len()).await;
    let other_archive_tags = get_other_archive_tags(&state, &plugin, params.arg.as_deref()).await;
    let existing_archive_id =
        get_existing_archive_id_for_url(&state, &plugin, params.arg.as_deref()).await;
    let archives_for_script = get_all_archive_tags_for_script(&state, &plugin).await;
    let (library_path, archive_id_by_path) = get_foldertocat_args(&state, &plugin).await;
    let args = json!({
        "archive_id": params.id,
        "arg": params.arg,
        "oneshot_param": params.arg,
        "customargs": customargs,
        "file_path": file_path,
        "file_modified_time": file_modified_time,
        "settings": get_plugin_relevant_settings(&state).await,
        "other_archive_tags": other_archive_tags,
        "existing_archive_id": existing_archive_id,
        "archives": archives_for_script,
        "library_path": library_path,
        "archive_id_by_path": archive_id_by_path,
        "existing_tags": queried_archive.as_ref().map(|a| a.tags.clone()).unwrap_or_default(),
        "archive_title": queried_archive.as_ref().map(|a| a.title.clone()).unwrap_or_default(),
    });
    let args = with_sidecar_files(&info, args, file_path.as_deref());
    let args = with_login_cookies(&state, &info, args).await;
    let method = plugin_method(&info.kind);

    match state.plugins.execute(&plugin, method, args).await {
        Ok(data) => {
            apply_script_tag_updates(&state, &plugin, &data).await;
            apply_foldertocat_categories(&state, &plugin, &data).await;
            axum::Json(json!({
                "operation": "use_plugin",
                "success": 1,
                "type": info.kind,
                "data": data,
            }))
            .into_response()
        }
        Err(e) => axum::Json(json!({
            "operation": "use_plugin",
            "success": 0,
            "type": info.kind,
            "error": e.to_string(),
        }))
        .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct QueuePluginParams {
    id: Option<String>,
    plugin: Option<String>,
    arg: Option<String>,
}

/// `POST /plugins/queue` — runs the plugin as a background job (legacy's async/Minion variant),
/// reusing the same `lanrurugi-core::jobs` abstraction as backup/rebuild-index/bench (T013).
async fn use_plugin_async(
    State(state): State<AppState>,
    Query(params): Query<QueuePluginParams>,
) -> Response {
    let Some(plugin) = params.plugin else {
        return error(
            StatusCode::BAD_REQUEST,
            "queue_plugin_exec",
            "No plugin specified.",
        );
    };

    let job_id = state.jobs.create("plugin_exec").await;
    let jobs = state.jobs.clone();
    let plugins = state.plugins.clone();
    let archives = state.repos.archives.clone();
    let job_id_for_task = job_id.clone();
    let archive_id = params.id.clone();
    let arg = params.arg.clone();
    let state_for_task = state.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let info = match plugins.plugin_info(&plugin).await {
            Ok(i) => i,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };
        let file_path = match &archive_id {
            Some(id) => archives
                .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
                .await
                .ok()
                .flatten()
                .map(|a| a.file),
            None => None,
        };
        let method = plugin_method(&info.kind);
        let customargs =
            get_plugin_customargs(&state_for_task, &plugin, info.parameters.len()).await;
        let existing_archive_id =
            get_existing_archive_id_for_url(&state_for_task, &plugin, arg.as_deref()).await;
        let archives_for_script = get_all_archive_tags_for_script(&state_for_task, &plugin).await;
        let (library_path, archive_id_by_path) =
            get_foldertocat_args(&state_for_task, &plugin).await;
        let args = json!({
            "archive_id": archive_id,
            "arg": arg,
            "oneshot_param": arg,
            "customargs": customargs,
            "file_path": file_path,
            "existing_archive_id": existing_archive_id,
            "archives": archives_for_script,
            "library_path": library_path,
            "archive_id_by_path": archive_id_by_path,
        });
        let args = with_sidecar_files(&info, args, file_path.as_deref());
        let args = with_login_cookies(&state_for_task, &info, args).await;
        match plugins.execute(&plugin, method, args).await {
            Ok(data) => {
                apply_script_tag_updates(&state_for_task, &plugin, &data).await;
                apply_foldertocat_categories(&state_for_task, &plugin, &data).await;
                jobs.finish(&job_id_for_task, data).await
            }
            Err(e) => jobs.fail(&job_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({
        "operation": "queue_plugin_exec",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct DownloadUrlParams {
    url: Option<String>,
    catid: Option<String>,
}

/// One `downloads[]` entry as `execDownload` returns it — mirrors
/// `contracts/plugin-download-protocol.md`'s wire shape field-for-field.
#[derive(Debug, Deserialize)]
struct DownloadRequestJson {
    url: String,
    method: Option<String>,
    #[serde(default)]
    headers: std::collections::HashMap<String, String>,
    filename_hint: Option<String>,
}

/// A plugin's own structured error (`plugin-sdk.ts`'s `PluginError`) — `error_code` is itself an
/// i18n lookup key (see that type's own docs), `data` its interpolation params.
#[derive(Debug, Clone, Deserialize)]
struct PluginErrorJson {
    error_code: String,
    #[serde(default)]
    data: Option<std::collections::HashMap<String, lanrurugi_core::queue_error::PluginErrorValue>>,
}

/// `execDownload`'s full return shape — see `contracts/plugin-download-protocol.md`'s extended
/// `DownloadResult`. Exactly one of `downloads`/`file_path`/`error` is expected to be present.
#[derive(Debug, Default, Deserialize)]
struct PluginDownloadResult {
    downloads: Option<Vec<DownloadRequestJson>>,
    file_path: Option<String>,
    error: Option<PluginErrorJson>,
}

/// `POST /download_url` — finds an enabled download-type plugin whose `url_regex` matches `url`
/// and queues it as a background job (verified shape: `~/LANraragi/tools/openapi.yaml`'s
/// `downloadUrl` operation). No `url_regex` field exists on `protocol::PluginInfo` yet (only
/// `contracts/plugin-protocol.md`'s minimal wire fields plus the display fields `list_plugins`
/// needs) — matching is deferred to whichever download plugin is installed to self-report via its
/// own `oneshot_arg`/description; for Phase 1 this dispatches to the **first** installed
/// download-type plugin found, which is sufficient for the single-download-plugin case
/// `quickstart.md` §4 exercises. Multi-plugin URL routing is a natural follow-up once more than
/// one download plugin ships.
///
/// `specs/005-download-plugin-progress`: the plugin itself no longer performs the real byte-level
/// HTTP transfer — its `exec_download` result is now one of `downloads[]` (one or more real
/// resource URLs, which this handler downloads itself via `download_manager`, reporting live
/// progress and respecting per-domain concurrency/rate-limit rules), `file_path` (pre-existing
/// fallback: the plugin already downloaded/wrote the file itself — unmanaged, no
/// progress/concurrency/rate-limit treatment), or `error`.
async fn download_url(
    State(state): State<AppState>,
    Query(params): Query<DownloadUrlParams>,
) -> Response {
    let Some(url) = params.url.clone().filter(|u| !u.is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "download_url", "No URL specified.");
    };

    let namespaces = discover_namespaces(&state.plugins_dir).await;
    let mut download_plugin = None;
    for ns in namespaces {
        if let Ok(info) = state.plugins.plugin_info(&ns).await {
            if info.kind == "download" {
                download_plugin = Some((ns, info));
                break;
            }
        }
    }

    let Some((plugin, info)) = download_plugin else {
        return error(
            StatusCode::BAD_REQUEST,
            "download_url",
            "No download plugin installed.",
        );
    };

    let job_id = start_download(
        state,
        plugin.clone(),
        info,
        url,
        params.catid.clone(),
        false,
        None,
    )
    .await;

    axum::Json(json!({
        "operation": "download_url",
        "url": params.url,
        "category": params.catid,
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

/// Launches one download (single- or multi-resource) as a background job, exactly as
/// `download_url` always has — extracted so the download-queue's own `start`/`start_all`/
/// `start_selected` endpoints can reuse the identical dispatch/execute/ingest sequence rather
/// than duplicating it. `overwrite` threads straight to [`run_managed_downloads`]/
/// `ingest_downloaded_file`. `queue_link`, when `Some((repo, item_id))`, keeps that download-queue
/// item's own `state`/`job_id`/`error` fields updated as the download progresses — this is what
/// makes an *in-progress* (not just not-yet-started) queued download's state survive a page
/// refresh or a different browser tab, since both poll `GET /download_queue` independently of
/// this job's own lifetime.
pub(crate) async fn start_download(
    state: AppState,
    plugin_namespace: String,
    info: PluginInfo,
    url: String,
    category: Option<String>,
    overwrite: bool,
    queue_link: Option<(
        Arc<lanrurugi_storage::download_queue::DownloadQueueRepository>,
        String,
    )>,
) -> String {
    let job_id = state.jobs.create("download_url").await;
    let jobs = state.jobs.clone();
    let plugins = state.plugins.clone();
    let job_id_for_task = job_id.clone();
    let state_for_task = state.clone();
    let plugin_namespace_for_task = plugin_namespace.clone();

    // Registered only when this download is tied to a persistent queue item — a queue row is the
    // only UI surface with a Stop button; a one-off `queue_link: None` download (if any caller
    // ever uses that path) has nothing to cancel it from. Keyed by queue-item ID, not job ID,
    // since `download_queue::stop_one` only ever knows the item ID the user clicked Stop on.
    let cancel = tokio_util::sync::CancellationToken::new();
    if let Some((_, item_id)) = &queue_link {
        state
            .download_cancellations
            .lock()
            .await
            .insert(item_id.clone(), cancel.clone());
    }
    let cancel_for_task = cancel.clone();
    let queue_link_for_cleanup = queue_link.clone();
    // A separate clone (not derived from `queue_link_for_cleanup` inside the inner task body)
    // specifically because that one is captured by the inner `async move` block below and must
    // still be usable, unmoved, in the cleanup code that runs *after* that block — `async move`
    // captures a referenced variable by move regardless of whether it's only ever borrowed inside.
    let queue_item_id_for_task = queue_link.as_ref().map(|(_, id)| id.clone());
    let state_for_cleanup = state.clone();

    tokio::spawn(async move {
        // The real task body is wrapped in its own async block (rather than being this whole
        // closure) purely so the `download_cancellations` entry inserted above always gets
        // removed afterward — every one of this body's several early `return`s is really "this
        // task is done now" and each one needs the same cleanup, and a `Drop`-based guard can't
        // do the `.await`ed `Mutex` removal this needs. Once the token is *inserted*, its only
        // other real-world visitor is `download_queue::stop_one`'s own lock+lookup+remove, so
        // there's a real (harmless) possible race where both this cleanup and a fresh stop
        // request try to remove the same now-finished entry — `HashMap::remove` on a
        // no-longer-present key is simply a no-op either way.
        (async move {
            jobs.mark_active(&job_id_for_task).await;
            if let Some((repo, item_id)) = &queue_link {
                update_queue_item_state(
                    repo,
                    item_id,
                    lanrurugi_storage::download_queue::DownloadQueueState::Downloading,
                    Some(job_id_for_task.clone()),
                    None,
                    None,
                )
                .await;
            }

            // Legacy's own `exec_download_plugin` bundles the target URL as `url`, not `arg`
            // (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:171-175`'s `%infohash` — `arg` is a
            // completely separate per-plugin *settings* value, e.g. E-Hentai's `forceresampled`
            // toggle, passed as legacy's own separate `%settings` positional arg to `provide_url`, and
            // as this repo's `hostArgs.customargs` — see `DownloadHostArgs`'s own docs). Every real
            // download plugin (`chaika.ts`/`ehentai.ts`/`pixiv.ts`) reads `hostArgs.url` for the URL
            // itself, for exactly this reason.
            //
            // Missing `customargs` here was a real, shipped bug (confirmed via a live "Cannot read
            // properties of undefined (reading '0')" error on a real E*Hentai download): unlike
            // `use_plugin_sync`/`use_plugin_async`, this path never called `get_plugin_customargs`, so
            // `ehentai.ts`'s `lrr_info.customargs[0]` (`forceresampled`) always threw on `undefined`.
            let customargs = get_plugin_customargs(
                &state_for_task,
                &plugin_namespace_for_task,
                info.parameters.len(),
            )
            .await;
            // Kept alive past `args`'s own move of `url` below — needed later for the real
            // `source:<url>` tag legacy's own `Utils::Minion::download_url` task adds (see
            // `ingest_downloaded_file`'s own docs), which must be the URL the *user* gave, not
            // whatever internal link the plugin's own `exec_download` result later resolves to.
            let source_url = url.clone();
            let args = json!({ "url": url, "category": category, "customargs": customargs });
            let args = with_login_cookies(&state_for_task, &info, args).await;
            let plugin_result = match plugins
                .execute(&plugin_namespace_for_task, "exec_download", args)
                .await
            {
                Ok(data) => data,
                Err(e) => {
                    let queue_error = queue_error_from_pool_error(&plugin_namespace_for_task, &e);
                    if let Some((repo, item_id)) = &queue_link {
                        update_queue_item_state(
                            repo,
                            item_id,
                            lanrurugi_storage::download_queue::DownloadQueueState::Error,
                            None,
                            Some(queue_error.clone()),
                            None,
                        )
                        .await;
                    }
                    jobs.fail(&job_id_for_task, e.to_string()).await;
                    return;
                }
            };

            let parsed: PluginDownloadResult = match serde_json::from_value(plugin_result.clone()) {
                Ok(p) => p,
                Err(e) => {
                    let queue_error =
                        lanrurugi_core::queue_error::QueueError::MalformedPluginResponse {
                            plugin: plugin_namespace_for_task.clone(),
                        };
                    if let Some((repo, item_id)) = &queue_link {
                        update_queue_item_state(
                            repo,
                            item_id,
                            lanrurugi_storage::download_queue::DownloadQueueState::Error,
                            None,
                            Some(queue_error.clone()),
                            None,
                        )
                        .await;
                    }
                    jobs.fail(
                        &job_id_for_task,
                        format!("plugin returned an unrecognized result shape: {e}"),
                    )
                    .await;
                    return;
                }
            };

            if let Some(err) = parsed.error {
                let queue_error = queue_error_from_plugin_error(&plugin_namespace_for_task, &err);
                if let Some((repo, item_id)) = &queue_link {
                    update_queue_item_state(
                        repo,
                        item_id,
                        lanrurugi_storage::download_queue::DownloadQueueState::Error,
                        None,
                        Some(queue_error.clone()),
                        None,
                    )
                    .await;
                }
                jobs.fail(&job_id_for_task, err.error_code).await;
                return;
            }

            if let Some(downloads) = parsed.downloads.filter(|d| !d.is_empty()) {
                match run_managed_downloads(
                    state_for_task.clone(),
                    plugin_namespace_for_task.clone(),
                    downloads,
                    job_id_for_task.clone(),
                    category.clone(),
                    overwrite,
                    &source_url,
                    &cancel_for_task,
                    queue_item_id_for_task.as_deref(),
                )
                .await
                {
                    Ok(ids) => {
                        if let Some((repo, item_id)) = &queue_link {
                            update_queue_item_state(
                                repo,
                                item_id,
                                lanrurugi_storage::download_queue::DownloadQueueState::Done,
                                None,
                                None,
                                Some(ids.clone()),
                            )
                            .await;
                        }
                        jobs.finish(&job_id_for_task, json!({ "archive_ids": ids }))
                            .await
                    }
                    // A user-requested Stop races with `download_one`'s own cancellation check: by the
                    // time `run_managed_downloads` returns `Err`, `cancel_for_task` may already be set
                    // even though the underlying `QueueError` (e.g. `Internal`, from the placeholder
                    // `DownloadError::Cancelled` conversion) looks like any other failure. Checking the
                    // token here — rather than growing `QueueError` a `Cancelled` variant, which would
                    // then have to be a persistable "error" that isn't really an error — is what lets a
                    // deliberate stop revert the item to `Cancelled` (restartable, same as `Queued`/
                    // `Error` — see `start_one`'s guard — but distinct so the frontend can keep
                    // showing "已取消" after a page refresh instead of that signal only living in
                    // transient mutation state) instead of `Error` (which `jobs.fail` + a stored
                    // `QueueError` would otherwise imply this was a real failure).
                    Err(_) if cancel_for_task.is_cancelled() => {
                        if let Some((repo, item_id)) = &queue_link {
                            update_queue_item_state(
                                repo,
                                item_id,
                                lanrurugi_storage::download_queue::DownloadQueueState::Cancelled,
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                        jobs.fail(&job_id_for_task, "cancelled".to_string()).await
                    }
                    Err(e) => {
                        if let Some((repo, item_id)) = &queue_link {
                            update_queue_item_state(
                                repo,
                                item_id,
                                lanrurugi_storage::download_queue::DownloadQueueState::Error,
                                None,
                                Some(e.clone()),
                                None,
                            )
                            .await;
                        }
                        jobs.fail(&job_id_for_task, format!("{e:?}")).await
                    }
                }
                return;
            }

            if let Some(file_path) = parsed.file_path {
                // Pre-existing fallback escape hatch — the plugin already downloaded/wrote the file
                // itself; unmanaged, no progress/concurrency/rate-limit treatment, since the byte
                // transfer already happened entirely inside the plugin process by this point. No
                // `archive_ids` either — this path never catalogs the file into an archive itself.
                if let Some((repo, item_id)) = &queue_link {
                    update_queue_item_state(
                        repo,
                        item_id,
                        lanrurugi_storage::download_queue::DownloadQueueState::Done,
                        None,
                        None,
                        None,
                    )
                    .await;
                }
                jobs.finish(&job_id_for_task, json!({ "file_path": file_path }))
                    .await;
                return;
            }

            let queue_error = lanrurugi_core::queue_error::QueueError::EmptyPluginResult {
                plugin: plugin_namespace_for_task.clone(),
            };
            if let Some((repo, item_id)) = &queue_link {
                update_queue_item_state(
                    repo,
                    item_id,
                    lanrurugi_storage::download_queue::DownloadQueueState::Error,
                    None,
                    Some(queue_error),
                    None,
                )
                .await;
            }
            jobs.fail(
                &job_id_for_task,
                "plugin returned neither downloads, file_path, nor error",
            )
            .await;
        })
        .await;

        if let Some((_, item_id)) = &queue_link_for_cleanup {
            state_for_cleanup
                .download_cancellations
                .lock()
                .await
                .remove(item_id);
        }
    });

    job_id
}

/// Builds a [`lanrurugi_core::queue_error::QueueError`] from a plugin-execution failure
/// (`PluginPool::execute`'s `Err`) — `QueueError::PluginReported` when the underlying
/// `ResponseError` carries a real `error_code` (the plugin itself threw a structured
/// `PluginErrorException`), `QueueError::PluginExecutionFailed` for everything else (a crash,
/// timeout, or unstructured throw — nothing translatable to extract).
fn queue_error_from_pool_error(
    plugin: &str,
    e: &lanrurugi_plugin::pool::PoolError,
) -> lanrurugi_core::queue_error::QueueError {
    use lanrurugi_core::queue_error::QueueError;
    tracing::warn!(plugin, error = %e, "plugin execution failed");
    if let lanrurugi_plugin::pool::PoolError::PluginError(err) = e {
        if let Some(error_code) = &err.error_code {
            return QueueError::PluginReported {
                plugin: plugin.to_string(),
                error_code: error_code.clone(),
                data: err.data.clone().unwrap_or_default(),
            };
        }
    }
    QueueError::PluginExecutionFailed {
        plugin: plugin.to_string(),
    }
}

/// Builds a `QueueError::PluginReported` from a plugin's own `{error_code, data}` payload (the
/// `PluginError` a metadata/download/script plugin `return`s rather than throws — see
/// `plugin-sdk.ts`'s `MetadataResult.error`/`DownloadResult.error` docs).
fn queue_error_from_plugin_error(
    plugin: &str,
    err: &PluginErrorJson,
) -> lanrurugi_core::queue_error::QueueError {
    lanrurugi_core::queue_error::QueueError::PluginReported {
        plugin: plugin.to_string(),
        error_code: err.error_code.clone(),
        data: err.data.clone().unwrap_or_default(),
    }
}

/// Best-effort partial update of one download-queue item's live-progress fields — a failure here
/// (e.g. the item was deleted mid-download) is logged, not propagated, since the actual download
/// job itself (the source of truth) keeps running/finishing regardless of whether its queue-item
/// mirror could be updated. `pub(crate)`: also the update path a local upload's own synchronous
/// ingest (`upload.rs::upload_archive`) writes its outcome through, so `Done`/`Error`/
/// `archive_ids` are recorded via the exact same set-if-some/error-unconditionally-overwrites
/// semantics a download uses, rather than a second, parallel implementation of the same rules.
pub(crate) async fn update_queue_item_state(
    repo: &lanrurugi_storage::download_queue::DownloadQueueRepository,
    item_id: &str,
    new_state: lanrurugi_storage::download_queue::DownloadQueueState,
    job_id: Option<String>,
    error: Option<lanrurugi_core::queue_error::QueueError>,
    archive_ids: Option<Vec<String>>,
) {
    match repo.get(item_id).await {
        Ok(Some(mut item)) => {
            item.state = new_state;
            if job_id.is_some() {
                item.job_id = job_id;
            }
            // Set-if-some, same as `job_id` above — persisted here (not just in the linked job's
            // ephemeral `JobRegistry` result) so the completed-item reader link survives a server
            // restart; see the field's own docs (`download_queue.rs`).
            if archive_ids.is_some() {
                item.archive_ids = archive_ids;
            }
            // Unconditional (not `if error.is_some()`): every real call site passes `None` to
            // mean "this transition has no error" — which should clear any stale error left over
            // from an earlier failed attempt on this same queue item (e.g. a retry that then
            // succeeds), not silently preserve it. The old conditional left a `Done` transition's
            // item permanently carrying its previous `Error` transition's message forever, a real,
            // confirmed bug (found via a live queue item that had `state: "done"` with real
            // successful `archive_ids` in its job result, yet still showed an "Invalid E*Hentai
            // login credentials" error from an earlier failed attempt).
            item.error = error;
            if let Err(e) = repo.update(&item).await {
                tracing::warn!(%item_id, error = %e, "failed to update download-queue item state");
            }
        }
        Ok(None) => {
            tracing::debug!(%item_id, "download-queue item no longer exists, skipping state update");
        }
        Err(e) => {
            tracing::warn!(%item_id, error = %e, "failed to load download-queue item for state update");
        }
    }
}

/// Resolves `plugin_namespace`'s effective `Domain Rule`s (plugin-declared defaults merged with
/// any persisted user override — see `download_manager::settings::resolve_domain_rules`) exactly
/// once, at the moment this download starts, gets/creates that plugin's own [`DownloadManager`],
/// and downloads every entry in `downloads` through it. Once every resource has finished
/// downloading, either bundles them all into one archive (spec FR-018's `bundle_as_archive: true`
/// — e.g. Pixiv's per-page images shipping as one manga archive) or catalogs each independently,
/// adding it to `category` (if any) so a non-bundled multi-resource download still lands as one
/// user-visible group. Returns the resulting archive IDs, or the first error encountered (any
/// already-downloaded/cataloged resources before that point are not rolled back — each is
/// independently already a real, valid archive by the time it's cataloged).
///
/// FR-016: `rules` below is a fixed snapshot for this download's entire lifetime, resolved once
/// here and never re-read from the live settings store partway through — a settings change that
/// lands while this download is already in flight only ever governs a *later* `download_url` call,
/// never this one (verified: `download_manager::acquire`'s own capacity-change handling replaces
/// the stale semaphore for future acquires without disturbing a permit already held by this call).
#[allow(clippy::too_many_arguments)]
async fn run_managed_downloads(
    state: AppState,
    plugin_namespace: String,
    downloads: Vec<DownloadRequestJson>,
    job_id: String,
    category: Option<String>,
    overwrite: bool,
    source_url: &str,
    cancel: &tokio_util::sync::CancellationToken,
    queue_item_id: Option<&str>,
) -> Result<Vec<String>, lanrurugi_core::queue_error::QueueError> {
    let manager = download_manager_for(&state, &plugin_namespace).await;
    let declared = fetch_declared_options(&state, &plugin_namespace)
        .await
        .unwrap_or_default();
    let override_ = state
        .plugin_options
        .get(&plugin_namespace)
        .await
        .unwrap_or_default();
    let rules: Vec<DomainRule> = resolve_domain_rules(&declared, override_.as_ref());

    // Record this download job's effective rate limit (and the matching domain-rule pattern) onto
    // its `JobStatus` once, at start, so the frontend's progress UI can show a highlighted badge +
    // tooltip for rate-limited downloads (issue #2). Uses the first resource's hostname as this
    // job's representative — the UI model is one combined progress indicator per job (FR-003), so
    // rate-limit display is a single value too, and real multi-resource downloads (Pixiv pages,
    // single-archive H@H) almost always share one domain/rule. Snapshotted here per FR-016: a
    // settings change mid-download never retroactively alters this value.
    //
    // Known approximation: `max_bytes_per_sec` comes from `resolve` (per-field independent
    // resolution — exact > wildcard > fallback, first rule declaring the field) while
    // `matched_pattern` comes from `resolved_key` (first pattern-matching rule in array order).
    // When an exact rule omits the rate limit and a wildcard supplies it, the two can name
    // different rules — an acceptable display-only approximation; fixing it would touch the
    // rate-limiter map's grouping key, out of scope here.
    if let Some(first) = downloads.first() {
        if let Ok(parsed) = url::Url::parse(&first.url) {
            let first_hostname = parsed.host_str().unwrap_or("");
            let resolved = crate::download_manager::domain_rules::resolve(&rules, first_hostname);
            let matched =
                crate::download_manager::domain_rules::resolved_key(&rules, first_hostname);
            state
                .jobs
                .set_rate_limit(
                    &job_id,
                    resolved.max_bytes_per_sec,
                    // Only record the pattern when a real hostname was matched, so a malformed
                    // (host-less) URL doesn't write a misleading catch-all "*".
                    if first_hostname.is_empty() {
                        None
                    } else {
                        Some(matched)
                    },
                )
                .await;
        }
    }

    let should_bundle =
        downloads.len() > 1 && resolve_bundle_as_archive(&declared, override_.as_ref());

    // spec FR-003: combined progress across every resource as one indicator for this job, not one
    // per resource — `download_one` deliberately reports only its own resource's byte counts (via
    // an mpsc channel, not a direct `JobRegistry` write or an async callback — see that function's
    // own docs on the real rustc limitation an async-closure version of this hit), so aggregation
    // into the single `job_id` happens here. `base_downloaded` is the running total from every
    // already-*completed* resource; each resource's own in-flight channel updates add their
    // current progress on top of that fixed base rather than overwriting it. Resources are
    // downloaded strictly one at a time in this loop (never concurrently with each other), so
    // `known_totals` mutated across sequential iterations needs no shared/interior mutability.
    let resource_count = downloads.len();
    let mut base_downloaded: u64 = 0;
    // Sum of every resource's own `total_bytes` *once known* — a job-wide total is only
    // meaningful once every resource's size has actually been reported by its own response;
    // until then (or if any resource never reports one), the combined total stays `None`
    // (indeterminate progress, spec FR-002) rather than under-reporting a partial sum as if it
    // were the real total.
    let mut known_totals: Vec<Option<u64>> = vec![None; resource_count];

    let mut downloaded_files = Vec::new();
    for (index, req) in downloads.into_iter().enumerate() {
        let stream_req = StreamDownloadRequest {
            url: req.url,
            method: req.method,
            headers: req.headers.into_iter().collect(),
            filename_hint: req.filename_hint,
        };
        let base = base_downloaded;

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        // Drains this resource's progress updates and writes the *combined* (base + this
        // resource's own progress) total into the real job, exactly once per update, until
        // `download_one` finishes and drops its sender (closing the channel, ending this loop).
        let jobs = state.jobs.clone();
        let job_id_for_drain = job_id.clone();
        let drain_task = tokio::spawn(async move {
            let mut known_totals = known_totals;
            while let Some((downloaded, total)) = progress_rx.recv().await {
                known_totals[index] = total;
                let combined_total = known_totals
                    .iter()
                    .all(Option::is_some)
                    .then(|| known_totals.iter().flatten().sum());
                jobs.set_download_progress(&job_id_for_drain, base + downloaded, combined_total)
                    .await;
            }
            known_totals
        });

        let downloaded_result = download_one(
            &manager,
            &rules,
            &stream_req,
            progress_tx,
            &state.library.temp_dir,
            cancel,
        )
        .await
        .map_err(|e| lanrurugi_core::queue_error::QueueError::from(&e))?;
        // Reclaim `known_totals` from the drain task (it owned the only mutable copy while
        // draining) so the next resource's iteration sees this resource's now-known total.
        known_totals = drain_task.await.map_err(|e| {
            tracing::warn!(error = %e, "progress-drain task panicked");
            lanrurugi_core::queue_error::QueueError::Internal
        })?;

        base_downloaded += downloaded_result.bytes_downloaded;
        downloaded_files.push(downloaded_result);
    }

    let mut archive_ids = Vec::new();
    if should_bundle {
        let bundle_filename =
            crate::download_manager::bundle::bundle_archive_filename(&plugin_namespace);
        let bundled = crate::download_manager::bundle::bundle_into_one_archive(
            &state.library.temp_dir,
            downloaded_files,
            &bundle_filename,
        )
        .await
        .map_err(|e| lanrurugi_core::queue_error::QueueError::from(&e))?;
        let ingested =
            ingest_downloaded_file(&state, &bundled, overwrite, Some(source_url), queue_item_id)
                .await
                .map_err(|e| lanrurugi_core::queue_error::QueueError::from(&e))?;
        if let Some(catid) = &category {
            let _ = crate::categories::add_archive_to_category(&state, catid, &ingested.archive_id)
                .await;
        }
        archive_ids.push(ingested.archive_id);
    } else {
        for downloaded_result in downloaded_files {
            let ingested = ingest_downloaded_file(
                &state,
                &downloaded_result,
                overwrite,
                Some(source_url),
                queue_item_id,
            )
            .await
            .map_err(|e| lanrurugi_core::queue_error::QueueError::from(&e))?;
            if let Some(catid) = &category {
                let _ =
                    crate::categories::add_archive_to_category(&state, catid, &ingested.archive_id)
                        .await;
            }
            archive_ids.push(ingested.archive_id);
        }
    }
    Ok(archive_ids)
}

/// Lazily creates (or returns the existing) [`DownloadManager`] for `namespace` — one instance
/// per plugin, since different plugins' domain rules are independent (spec Assumptions).
async fn download_manager_for(state: &AppState, namespace: &str) -> Arc<DownloadManager> {
    let mut managers = state.download_managers.lock().await;
    managers
        .entry(namespace.to_string())
        .or_insert_with(|| Arc::new(DownloadManager::new()))
        .clone()
}

/// Filenames are taken from the multipart field only for their base name — never used as a path,
/// so a client can't traverse outside `plugins_dir` via `../../etc/passwd`-style names. Mirrors
/// `upload::sanitize_filename`'s reasoning exactly.
fn sanitize_plugin_filename(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin.ts")
        .to_string()
}

/// `PUT`/`POST /plugins/upload` — accepts a single `.ts` plugin file, verifies it by actually
/// running its `plugin_info()` (a throwaway, zero-permission subprocess — same call
/// `PluginPool::plugin_info` uses everywhere else), then moves it into
/// `plugins_dir/custom/<category>/` based on what the plugin itself declared its `type` to be
/// (never trusting a client-supplied category). This is the automatic-classification counterpart
/// to legacy's own regex-on-`package`-declaration sniff (`process_upload`'s
/// `/package LANraragi::Plugin::(Login|Metadata|Scripts|Download)::/` match) — here it's the
/// real, structured `plugin_info()` response instead of guessing from source text, which can't be
/// fooled by a stray comment or a namespace that merely mentions one of those words.
///
/// On any validation failure the just-written file is removed, so a bad upload never leaves a
/// half-installed plugin sitting in `plugins_dir` (mirrors legacy's own `unlink($output_file)`
/// cleanup in `process_upload`'s error branch).
async fn upload_plugin(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return error(StatusCode::BAD_REQUEST, "upload_plugin", e.to_string()),
        };
        if field.name() == Some("file") {
            file_name = field.file_name().map(sanitize_plugin_filename);
            match field.bytes().await {
                Ok(b) => file_bytes = Some(b.to_vec()),
                Err(e) => return error(StatusCode::BAD_REQUEST, "upload_plugin", e.to_string()),
            }
        }
    }

    let Some(bytes) = file_bytes else {
        return error(
            StatusCode::BAD_REQUEST,
            "upload_plugin",
            "No file provided.",
        );
    };
    let Some(file_name) = file_name.filter(|n| n.ends_with(".ts")) else {
        return error(
            StatusCode::BAD_REQUEST,
            "upload_plugin",
            "Please upload a plugin (.ts) file.",
        );
    };
    // The namespace `plugin_info` (and every later `execute` call) will use, once staged.
    let staged_namespace = format!("{CUSTOM_PLUGIN_DIR}/{}", file_name.trim_end_matches(".ts"));

    let staging_dir = state.plugins_dir.join(CUSTOM_PLUGIN_DIR);
    if let Err(e) = tokio::fs::create_dir_all(&staging_dir).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_plugin",
            e.to_string(),
        );
    }
    let staged_path = staging_dir.join(&file_name);
    if staged_path.exists() {
        return error(
            StatusCode::CONFLICT,
            "upload_plugin",
            format!("A plugin named {file_name:?} is already installed."),
        );
    }
    if let Err(e) = tokio::fs::write(&staged_path, &bytes).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_plugin",
            e.to_string(),
        );
    }

    // Ask the plugin itself what it is — never trust a client-supplied category (constitution
    // Principle IV: only the sandboxed plugin's own declared metadata is authoritative here).
    let info = match state.plugins.plugin_info(&staged_namespace).await {
        Ok(info) => info,
        Err(e) => {
            let _ = tokio::fs::remove_file(&staged_path).await;
            return error(
                StatusCode::BAD_REQUEST,
                "upload_plugin",
                format!(
                    "Could not load this plugin — it might not implement the expected protocol. {e}"
                ),
            );
        }
    };

    if !PLUGIN_CATEGORIES.contains(&info.kind.as_str()) {
        let _ = tokio::fs::remove_file(&staged_path).await;
        return error(
            StatusCode::BAD_REQUEST,
            "upload_plugin",
            format!("Unknown plugin type {:?}.", info.kind),
        );
    }

    let final_dir = staging_dir.join(&info.kind);
    if let Err(e) = tokio::fs::create_dir_all(&final_dir).await {
        let _ = tokio::fs::remove_file(&staged_path).await;
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_plugin",
            e.to_string(),
        );
    }
    let final_path = final_dir.join(&file_name);
    if final_path.exists() {
        let _ = tokio::fs::remove_file(&staged_path).await;
        return error(
            StatusCode::CONFLICT,
            "upload_plugin",
            format!(
                "A {} plugin named {file_name:?} is already installed.",
                info.kind
            ),
        );
    }
    if let Err(e) = tokio::fs::rename(&staged_path, &final_path).await {
        let _ = tokio::fs::remove_file(&staged_path).await;
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_plugin",
            e.to_string(),
        );
    }

    axum::Json(json!({
        "operation": "upload_plugin",
        "success": 1,
        "namespace": format!("{CUSTOM_PLUGIN_DIR}/{}/{}", info.kind, file_name.trim_end_matches(".ts")),
        "name": info.name,
        "type": info.kind,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::plugin_options::DomainRuleOverride;

    #[test]
    fn trim_url_strips_scheme_www_query_and_trailing_slash() {
        assert_eq!(
            trim_url("https://www.e-hentai.org/g/123/abc/?p=1"),
            "e-hentai.org/g/123/abc"
        );
        assert_eq!(
            trim_url("http://e-hentai.org/g/123/abc/"),
            "e-hentai.org/g/123/abc"
        );
        assert_eq!(
            trim_url("  e-hentai.org/g/123/abc  "),
            "e-hentai.org/g/123/abc"
        );
    }

    #[test]
    fn trim_url_leaves_bare_paths_alone() {
        assert_eq!(trim_url("nhentai.net/g/999"), "nhentai.net/g/999");
    }

    fn rule(max_concurrent: Option<u32>, max_bytes_per_sec: Option<u64>) -> DomainRuleOverride {
        DomainRuleOverride {
            pattern: Some("*.example.com".to_string()),
            max_concurrent,
            max_bytes_per_sec,
        }
    }

    #[test]
    fn accepts_positive_concurrency_and_rate_limit_values() {
        assert!(validate_domain_rules(&[rule(Some(3), Some(1024))]).is_ok());
    }

    #[test]
    fn accepts_a_rule_with_neither_field_set() {
        assert!(validate_domain_rules(&[rule(None, None)]).is_ok());
    }

    #[test]
    fn rejects_a_zero_max_concurrent() {
        let (message, field) = validate_domain_rules(&[rule(Some(0), None)]).unwrap_err();
        assert_eq!(message, "max_concurrent must be a positive integer");
        assert_eq!(field, "domain_rules[0].max_concurrent");
    }

    #[test]
    fn rejects_a_zero_max_bytes_per_sec_on_the_general_fallback_rule() {
        // FR-009: the pattern-less/`"*"` general fallback rule is still subject to the same
        // FR-014 validation as any pattern-specific rule — a zero rate limit there is rejected
        // the same way, not silently treated as "unlimited" or skipped.
        let fallback = DomainRuleOverride {
            pattern: None,
            max_concurrent: None,
            max_bytes_per_sec: Some(0),
        };
        let (message, field) = validate_domain_rules(&[fallback]).unwrap_err();
        assert_eq!(message, "max_bytes_per_sec must be a positive integer");
        assert_eq!(field, "domain_rules[0].max_bytes_per_sec");
    }

    #[test]
    fn identifies_the_correct_index_among_multiple_rules() {
        let rules = [rule(Some(2), None), rule(None, Some(0))];
        let (_, field) = validate_domain_rules(&rules).unwrap_err();
        assert_eq!(field, "domain_rules[1].max_bytes_per_sec");
    }

    /// Opt-in, real-server regression test exercising the actual `ehdl` (`plugins/download/
    /// ehentai.ts`) plugin end-to-end through the real Deno dispatcher: logs in via the real
    /// `login/ehentai` plugin (using this machine's own already-configured E-Hentai session
    /// cookies, read from Redis exactly the way `with_login_cookies` does in production — this
    /// test deliberately re-touches that private helper's logic inline rather than calling it
    /// directly, since a `#[cfg(test)] mod` inside this same file *can* call private functions,
    /// but `with_login_cookies` needs a full `AppState`, which is far heavier to construct than
    /// the two `pool.execute` calls this test actually needs), then runs `execDownload` against a
    /// real gallery URL to obtain a genuinely live H@H download link, then feeds that link through
    /// the real `download_one` (`download_manager::stream`) to verify the Content-Disposition
    /// non-ASCII filename fix against an actual live response — not a local mock, and not a
    /// filename hardcoded into this repo's source (see that module's own test for why).
    ///
    /// Requires (all skipped, not failed, when unavailable — this test depends on infrastructure
    /// this repo doesn't control: a live external site, this machine's own login session, and GP
    /// balance to spend):
    /// - `deno` on `$PATH` (only present inside the dev container, not typically on a bare host)
    /// - `LANRURUGI_TEST_REDIS_URL` set (see `README.md`) and a real, currently-valid E-Hentai
    ///   session already saved under `LRR_PLUGIN_LOGIN/EHENTAI` (via the app's own Settings UI —
    ///   this test only *reads* that, it never prompts for or stores credentials itself)
    /// - `TEST_REAL_EHENTAI_GALLERY_URL` set to a real, currently-accessible gallery page URL
    ///   (e.g. `https://e-hentai.org/g/<gid>/<gtoken>/`) — left unset in `.env.local` by default
    ///   since running this consumes real GP on every invocation; set it only when deliberately
    ///   re-verifying this path end-to-end.
    #[tokio::test]
    async fn ehdl_plugin_produces_a_real_downloadable_url_with_a_correctly_decoded_filename() {
        // `.env.local`'s own template ships these as present-but-empty (`KEY=`) rather than
        // absent, so an empty string must be treated the same as unset.
        let gallery_url = std::env::var("TEST_REAL_EHENTAI_GALLERY_URL").unwrap_or_default();
        if gallery_url.is_empty() {
            eprintln!(
                "skipping: set TEST_REAL_EHENTAI_GALLERY_URL in .env.local to run this test \
                 against a real E-Hentai gallery"
            );
            return;
        }
        let redis_url = std::env::var("LANRURUGI_TEST_REDIS_URL").unwrap_or_default();
        if redis_url.is_empty() {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        }
        let deno_on_path = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("deno").is_file())
        });
        if !deno_on_path {
            eprintln!("skipping: deno not found on PATH");
            return;
        }

        let redis = lanrurugi_storage::redis::RedisDbs::connect(&redis_url)
            .expect("failed to build a Redis connection pool from LANRURUGI_TEST_REDIS_URL");
        let mut conn = redis
            .config
            .get()
            .await
            .expect("failed to reach the real Redis instance");
        let raw_customargs: Option<String> = conn
            .hget("LRR_PLUGIN_LOGIN/EHENTAI", "customargs")
            .await
            .unwrap_or_default();
        let Some(raw_customargs) = raw_customargs else {
            eprintln!(
                "skipping: no E-Hentai login saved under LRR_PLUGIN_LOGIN/EHENTAI — configure \
                 the login plugin via the app's Settings UI first"
            );
            return;
        };
        let login_customargs: Vec<String> =
            serde_json::from_str(&raw_customargs).expect("stored customargs must be a JSON array");

        // Real repo `plugins/` dir — this crate's own manifest dir is `crates/lanrurugi-api`, two
        // levels below the workspace root.
        let plugins_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .canonicalize()
            .expect("repo's real plugins/ dir must exist");
        let temp_dir = std::env::temp_dir();
        let dispatcher_path = temp_dir.join("lrr-ehdl-integration-test-dispatcher.ts");
        std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)
            .expect("failed to write out the real dispatcher script");
        let pool =
            lanrurugi_plugin::pool::PluginPool::new("deno", dispatcher_path.clone(), plugins_dir);

        let login_result = pool
            .execute(
                "login/ehentai",
                "exec_login",
                json!({ "customargs": login_customargs }),
            )
            .await
            .expect("real login/ehentai exec_login call failed");
        let cookies = login_result
            .get("cookies")
            .cloned()
            .expect("a real, valid E-Hentai session must yield cookies");

        let download_args = json!({
            "url": gallery_url,
            "category": null,
            "customargs": [""],
            "user_agent_cookies": cookies,
        });
        let download_result = pool
            .execute("download/ehentai", "exec_download", download_args)
            .await
            .expect("real download/ehentai exec_download call failed");
        let parsed: PluginDownloadResult = serde_json::from_value(download_result)
            .expect("execDownload must return the documented DownloadResult shape");
        if let Some(err) = &parsed.error {
            panic!(
                "ehdl plugin returned a real error (stale session, insufficient GP, or an \
                 invalid gallery URL — check TEST_REAL_EHENTAI_GALLERY_URL and the saved login): \
                 {} {:?}",
                err.error_code, err.data
            );
        }
        let downloads = parsed
            .downloads
            .filter(|d| !d.is_empty())
            .expect("a successful ehdl exec_download must return a non-empty downloads[]");
        assert_eq!(
            downloads.len(),
            1,
            "a single E-Hentai gallery archive download is always exactly one resource"
        );
        let h_at_h_url = &downloads[0].url;
        assert!(
            h_at_h_url.contains(".hath.network"),
            "the real download URL must point at an H@H client node, got: {h_at_h_url}"
        );

        // Now feed the real, live H@H URL through the real `download_one` — this is the actual
        // regression check for `content_disposition_filename`'s non-ASCII UTF-8 fix
        // (`download_manager::stream`), against a genuinely live response instead of a local mock.
        let manager = crate::download_manager::DownloadManager::new();
        let staging_dir = std::env::temp_dir();
        let req = StreamDownloadRequest {
            url: h_at_h_url.clone(),
            method: downloads[0].method.clone(),
            headers: downloads[0].headers.clone().into_iter().collect(),
            filename_hint: downloads[0].filename_hint.clone(),
        };
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let downloaded = download_one(
            &manager,
            &[],
            &req,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("the real H@H download must succeed");

        // The real bug this guards against: before the fix, a non-ASCII Content-Disposition
        // filename silently fell through to `filename_hint` (`{gID}_{gToken}.zip` — a meaningless
        // gallery-ID string), never the real archive title. A correctly decoded filename must be
        // neither empty nor exactly that fallback pattern.
        assert!(
            !downloaded.filename.is_empty(),
            "resolved filename must not be empty"
        );
        if let Some(hint) = &downloads[0].filename_hint {
            assert_ne!(
                &downloaded.filename, hint,
                "the real Content-Disposition filename must win over the gallery-ID \
                 filename_hint fallback — getting the hint back means the parsing bug regressed"
            );
        }

        tokio::fs::remove_file(&downloaded.path).await.ok();
        tokio::fs::remove_file(&dispatcher_path).await.ok();
    }

    /// Real, end-to-end replacement for the old `contract_api.rs` test
    /// `nhentai_source_converter_rewrites_short_numeric_source_tags_only`, which asserted against
    /// `POST /api/database/scripts/nhentai-source-converter` — a native Rust endpoint that no
    /// longer exists (`scripts.rs`'s own doc comment: `nHentaiSourceConverter.pm` was migrated to
    /// a real `script`-type plugin, `plugins/script/nhentaisourceconverter.ts`, run through the
    /// same `/plugins/use` machinery every other plugin uses). That old test was silently 404ing
    /// on every run — nobody noticed because `LANRURUGI_TEST_REDIS_URL` had never actually been
    /// wired into the containerized test flow until this session, so it (like every other
    /// Redis-gated test) was skipping, not failing.
    ///
    /// This test calls the real plugin's `runScript` directly through the real Deno dispatcher —
    /// no Redis, no network, no login needed at all (unlike the `ehdl` test above): this plugin's
    /// entire job is a pure, local `archives[].tags` string rewrite, handed back as `updates` for
    /// the host to apply (see `nhentaisourceconverter.ts`'s own doc comment on that read-compute-
    /// write split). Only requires `deno` on `$PATH`.
    #[tokio::test]
    async fn nhentai_source_converter_rewrites_short_numeric_source_tags_only() {
        let deno_on_path = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("deno").is_file())
        });
        if !deno_on_path {
            eprintln!("skipping: deno not found on PATH");
            return;
        }

        let plugins_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .canonicalize()
            .expect("repo's real plugins/ dir must exist");
        let dispatcher_path =
            std::env::temp_dir().join("lrr-nhsrcconv-integration-test-dispatcher.ts");
        std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)
            .expect("failed to write out the real dispatcher script");
        let pool =
            lanrurugi_plugin::pool::PluginPool::new("deno", dispatcher_path.clone(), plugins_dir);

        // Same fixture the old, now-dead test used: a mix of a short numeric `source:` tag (must
        // be rewritten), an unrelated tag, an already-converted tag (must stay untouched), and a
        // too-long numeric tag (7 digits — past the `{1,6}` the real plugin's own regex allows,
        // must also stay untouched).
        let archives = json!([
            { "id": "a", "tags": "source:123456, artist:someone, source:nhentai.net/g/1, source:1234567" },
        ]);
        let result = pool
            .execute(
                "script/nhentaisourceconverter",
                "exec_script",
                json!({ "archives": archives }),
            )
            .await
            .expect("real script/nhentaisourceconverter exec_script call failed");

        assert_eq!(result["modified"], 1);
        let updates = result["updates"]
            .as_array()
            .expect("updates must be an array");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0]["id"], "a");
        assert_eq!(
            updates[0]["tags"],
            "source:nhentai.net/g/123456, artist:someone, source:nhentai.net/g/1, source:1234567"
        );

        tokio::fs::remove_file(&dispatcher_path).await.ok();
    }
}
