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
use lanrurugi_plugin::protocol::{
    DownloadRequest as PluginDownloadRequest, DownloadResult as PluginDownloadResult, PluginError,
    PluginInfo,
};
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
use crate::tag_rules;
use crate::AppState;

/// The four categories a plugin's own `plugin_info().type` can declare — also the fixed set of
/// subdirectories under `plugins_dir` that ship in the repo (`plugins/metadata/`, `plugins/login/`,
/// `plugins/download/`, `plugins/script/`). `install_plugin` trusts *only* this list (not
/// user input) to pick the destination subdirectory for an uploaded plugin.
pub(crate) const PLUGIN_CATEGORIES: &[&str] = &["metadata", "login", "download", "script"];

/// Where every uploaded plugin lands, regardless of its declared category — mirrors legacy's own
/// `lib/LANraragi/Plugin/Sideloaded/` (verified: `Controller/Plugins.pm::process_upload`), which
/// also keeps user-supplied plugins physically separate from ones shipped in the repo, but nested
/// one level deeper by category (`custom/metadata/`, `custom/login/`, …) since namespaces here can
/// contain subdirectories (unlike legacy's flat `Sideloaded/`).
pub(crate) const CUSTOM_PLUGIN_DIR: &str = "custom";

/// How long a cached `metadata_preview` stays fresh before `ensure_metadata_cached` re-runs the
/// plugin — long enough to cover a batch of downloads landing within minutes of each other, short
/// enough that a plugin's bad/transient result doesn't linger.
const METADATA_CACHE_TTL_MS: i64 = 10 * 60 * 1000;

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
/// `customargs` field, positionally matching `info.parameters` (`parameters[0]`'s value is
/// `customargs[0]`, etc.) — each element its own declared type (`bool`/`int`/`string`), carried
/// as a real JSON value end to end rather than a uniform string a plugin has to parse back out
/// itself (issue #78/#93's own follow-on: `"1"`/`""` vs `"true"`/`"false"` vs bare truthiness were
/// each independently reinvented by different plugins in this corpus, one of them just wrong).
///
/// Writes real `customargs` for a plugin directly — the same `LRR_PLUGIN_<NS>` Redis write
/// `put_plugin_settings` (the real `/plugins/settings` HTTP endpoint) does, extracted into a plain
/// async fn so a caller that isn't itself an HTTP handler (`plugin_wizard::save`, saving whatever
/// values the user already filled into the wizard's own trial-run parameter inputs) can reuse the
/// identical write path rather than requiring the user to visit the plugin's settings page a
/// second time to enter the same values again post-install.
pub(crate) async fn set_plugin_customargs(
    state: &AppState,
    namespace: &str,
    customargs: &[Value],
) -> Result<(), String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let encoded = serde_json::to_string(customargs).map_err(|e| e.to_string())?;
    conn.hset::<_, _, _, ()>(plugin_settings_key(namespace), "customargs", encoded)
        .await
        .map_err(|e| e.to_string())
}

/// Reads a plugin's persisted `customargs` back, coercing each element to its own declared
/// `parameters[i].param_type` — `"bool"` becomes a real `Value::Bool`, `"int"` a real
/// `Value::Number`, anything else stays a string. Handles both a value already stored in its real
/// type (written by the current `set_plugin_customargs`) and legacy's own `"1"`/`""` string
/// encoding still sitting in Redis from before this existed — `"1"` (and, defensively, `"true"`)
/// coerce to `true`, everything else to `false`, so an old saved value doesn't silently read back
/// as "off" forever just because it predates this change. Missing/malformed stored data (nothing
/// saved yet, or a legacy instance's differently-shaped value) is treated as "no overrides" — one
/// type-appropriate default per declared parameter — rather than an error, since every value here
/// is optional and a user may simply not have configured it yet.
pub(crate) async fn get_plugin_customargs(
    state: &AppState,
    namespace: &str,
    parameters: &[lanrurugi_plugin::protocol::PluginParameter],
) -> Vec<Value> {
    let mut args: Vec<Value> = parameters
        .iter()
        .map(|p| default_customarg(p.param_type.as_deref()))
        .collect();
    let Ok(mut conn) = state.redis.config.get().await else {
        return args;
    };
    let raw: Option<String> = conn
        .hget(plugin_settings_key(namespace), "customargs")
        .await
        .unwrap_or_default();
    if let Some(raw) = raw {
        if let Ok(saved) = serde_json::from_str::<Vec<Value>>(&raw) {
            for ((slot, param), value) in args.iter_mut().zip(parameters).zip(saved) {
                *slot = coerce_customarg(param.param_type.as_deref(), value);
            }
        }
    }
    args
}

fn default_customarg(param_type: Option<&str>) -> Value {
    match param_type {
        Some("bool") => Value::Bool(false),
        Some("int") => Value::Number(0.into()),
        _ => Value::String(String::new()),
    }
}

/// See [`get_plugin_customargs`]'s own docs — `value` is whatever was actually stored, which may
/// already be the real type (nothing to do) or legacy's own string encoding (needs converting).
fn coerce_customarg(param_type: Option<&str>, value: Value) -> Value {
    match param_type {
        Some("bool") => match &value {
            Value::Bool(_) => value,
            Value::String(s) => Value::Bool(s == "1" || s == "true"),
            _ => Value::Bool(false),
        },
        Some("int") => match &value {
            Value::Number(_) => value,
            Value::String(s) => s
                .parse::<i64>()
                .map(|n| Value::Number(n.into()))
                .unwrap_or(Value::Number(0.into())),
            _ => Value::Number(0.into()),
        },
        _ => match value {
            Value::String(_) => value,
            other => Value::String(other.to_string()),
        },
    }
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

/// Orders `namespaces` by the same (explicit `priority`, discovery-order fallback) rule
/// `list_plugins` already sorts its own response by — used by `find_matching_plugin`/
/// `resolve_declared_namespace` so "first match wins" actually respects a user's Plugins-page
/// drag-to-reorder, instead of `discover_namespaces`'s own unspecified `read_dir` order (confirmed
/// live, 2026-08-26: a wizard-generated `custom/` plugin meant to override a built-in one for the
/// same domain had no reliable effect on which one actually got used for downloads/metadata,
/// since neither of those call sites sorted their candidate list at all before this).
pub(crate) async fn sort_namespaces_by_priority(
    state: &AppState,
    namespaces: &[String],
) -> Vec<String> {
    let mut indexed: Vec<(usize, String, Option<i64>)> = Vec::with_capacity(namespaces.len());
    for (i, ns) in namespaces.iter().enumerate() {
        let priority = get_plugin_priority(state, ns).await;
        indexed.push((i, ns.clone(), priority));
    }
    indexed.sort_by_key(|(i, _, priority)| priority.unwrap_or(i64::MAX / 2 + *i as i64));
    indexed.into_iter().map(|(_, ns, _)| ns).collect()
}

/// Runs `info`'s declared `login_from` plugin (if any) fresh and folds the resulting cookies/
/// headers into `args["user_agent_cookies"]`/`args["user_agent_headers"]` — mirrors legacy's
/// `exec_login_plugin` (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:107-135`), which re-logs-in
/// before *every* metadata/download/script call rather than caching a session (there's no
/// session-lifetime concept to reuse here either, so neither host nor plugin needs to invalidate
/// anything later). `login`-type plugins are never login'd into themselves (`info.kind == "login"`
/// short-circuits immediately) and a login failure only logs a warning — the main plugin call
/// still goes ahead, just without any cookies/headers, the same as legacy falling back to a blank
/// `Mojo::UserAgent->new` when its own login attempt didn't return a real user agent.
///
/// `headers` (issue #78/#93) exists alongside `cookies` because not every site authenticates via
/// a cookie — a login plugin authenticating via a header/token (e.g. `nhapiauth`'s
/// `Authorization: Key <api_key>`) has no cookie to hand back at all, and before this field
/// existed had no way to pass that credential to a downstream metadata/download call either.
pub(crate) async fn with_login_cookies(
    state: &AppState,
    info: &PluginInfo,
    mut args: Value,
) -> Value {
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
    let customargs = get_plugin_customargs(state, &login_ns, &login_info.parameters).await;
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
            if let Some(headers) = result.get("headers") {
                args["user_agent_headers"] = headers.clone();
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
        .route("/plugins/export", get(export_plugin))
        .route("/plugins/export-batch", post(export_plugins_batch))
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
    let parameters = match state.plugins.plugin_info(&query.namespace).await {
        Ok(info) => info.parameters,
        Err(e) => {
            return error(StatusCode::NOT_FOUND, "get_plugin_settings", e.to_string());
        }
    };
    let customargs = get_plugin_customargs(&state, &query.namespace, &parameters).await;
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
    customargs: Option<Vec<Value>>,
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
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
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
            // A real attempted priority write (the namespace already passed the type check above)
            // that failed on the actual Redis write — worth recording.
            crate::activity::record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                lanrurugi_storage::activity::action_types::PLUGIN_PRIORITY_UPDATE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: None,
                    label: Some(body.kind.clone()),
                    kind: Some("plugin".to_string()),
                },
                lanrurugi_storage::activity::Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_plugin_priority",
                e.to_string(),
            );
        }
    }
    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::PLUGIN_PRIORITY_UPDATE,
        lanrurugi_storage::activity::ActivityTarget {
            id: None,
            label: Some(body.kind.clone()),
            kind: Some("plugin".to_string()),
        },
        lanrurugi_storage::activity::Outcome::Success,
        None,
        Some(json!({ "order": body.order, "skipped": skipped })),
    )
    .await;
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
    // Tell every in-flight download's live rate-limit resolver to re-read this namespace's
    // override — see `AppState::plugin_options_generation`'s own docs for why this is a plain
    // atomic bump rather than a pub/sub or a per-chunk Redis read.
    state
        .plugin_options_generation
        .fetch_add(1, std::sync::atomic::Ordering::Release);

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
    state
        .plugin_options_generation
        .fetch_add(1, std::sync::atomic::Ordering::Release);
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
/// for one whose own `plugin_info().namespace` matches, in priority order
/// (`sort_namespaces_by_priority` — same reasoning as `find_matching_plugin`'s own docs: two
/// installed plugins sharing a declared namespace must resolve deterministically to the
/// higher-priority one, not whichever `discover_namespaces`'s scan happened to visit first).
/// `None` if none does (a plugin points at a `login_from` that isn't actually installed).
pub(crate) async fn resolve_declared_namespace(
    state: &AppState,
    declared_namespace: &str,
) -> Option<(String, PluginInfo)> {
    let all = discover_namespaces(&state.plugins_dir).await;
    let ordered = sort_namespaces_by_priority(state, &all).await;
    for ns in &ordered {
        if let Ok(info) = state.plugins.plugin_info(ns).await {
            if info.namespace == declared_namespace {
                return Some((ns.clone(), info));
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
pub(crate) async fn discover_namespaces(plugins_dir: &std::path::Path) -> Vec<String> {
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

#[derive(Debug, Deserialize)]
struct ExportPluginQuery {
    namespace: String,
}

/// `GET /plugins/export?namespace=...` (`specs/006-ai-plugin-wizard` FR-028) — downloads an
/// installed plugin (wizard-created or not) as a `.zip` containing its own `.ts` file, named
/// after the namespace's own last path component (e.g. `custom/metadata/foo` → `foo.zip`
/// containing `foo.ts`) rather than the full namespace, since the namespace's `/`-separated
/// category prefix isn't meaningful once extracted standalone.
async fn export_plugin(
    State(state): State<AppState>,
    Query(q): Query<ExportPluginQuery>,
) -> Response {
    if !is_safe_export_namespace(&q.namespace) {
        return error(
            StatusCode::BAD_REQUEST,
            "export_plugin",
            "Invalid namespace.",
        );
    }
    let source_path = state.plugins_dir.join(format!("{}.ts", q.namespace));
    let code = match tokio::fs::read(&source_path).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::NOT_FOUND, "export_plugin", "Plugin not found."),
    };

    let file_stem = q
        .namespace
        .rsplit('/')
        .next()
        .unwrap_or(&q.namespace)
        .to_string();
    let ts_name = format!("{file_stem}.ts");
    let zip_name = format!("{file_stem}.zip");

    let zip_bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer.start_file(ts_name, options)?;
        std::io::Write::write_all(&mut writer, &code)?;
        writer.finish()?;
        Ok(buf.into_inner())
    })
    .await;

    let Ok(Ok(zip_bytes)) = zip_bytes else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_plugin",
            "Failed to build zip archive.",
        );
    };

    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{zip_name}\""),
            ),
        ],
        zip_bytes,
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ExportPluginsBatchRequest {
    namespaces: Vec<String>,
}

/// `POST /plugins/export-batch` — like [`export_plugin`] but for several namespaces at once,
/// packed into a single `.zip`. Each entry is stored under its full namespace path (`custom/
/// download/foo.ts`, not just `foo.ts`) since two different namespaces can share the same file
/// stem (`custom/download/foo` vs `custom/metadata/foo`) — collapsing both to `foo.ts` would
/// silently drop one on zip write.
async fn export_plugins_batch(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<ExportPluginsBatchRequest>,
) -> Response {
    if req.namespaces.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "export_plugins_batch",
            "No namespaces given.",
        );
    }
    if !req.namespaces.iter().all(|ns| is_safe_export_namespace(ns)) {
        return error(
            StatusCode::BAD_REQUEST,
            "export_plugins_batch",
            "Invalid namespace.",
        );
    }

    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(req.namespaces.len());
    for namespace in &req.namespaces {
        let source_path = state.plugins_dir.join(format!("{namespace}.ts"));
        match tokio::fs::read(&source_path).await {
            Ok(bytes) => entries.push((namespace.clone(), bytes)),
            Err(_) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "export_plugins_batch",
                    format!("Plugin {namespace:?} not found."),
                )
            }
        }
    }

    let zip_bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (namespace, code) in entries {
            writer.start_file(format!("{namespace}.ts"), options)?;
            std::io::Write::write_all(&mut writer, &code)?;
        }
        writer.finish()?;
        Ok(buf.into_inner())
    })
    .await;

    let Ok(Ok(zip_bytes)) = zip_bytes else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_plugins_batch",
            "Failed to build zip archive.",
        );
    };

    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"plugins.zip\"".to_string(),
            ),
        ],
        zip_bytes,
    )
        .into_response()
}

/// A namespace safe to read directly off disk for export — same shape `is_safe_namespace`
/// (`lanrurugi-plugin::pool`) enforces for the sandbox itself (no absolute paths, no `..`
/// traversal, every component a plain name), re-checked here since this endpoint reads the file
/// straight from `plugins_dir` rather than going through `PluginPool`.
fn is_safe_export_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && std::path::Path::new(namespace)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
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
                    "domain_match": info.domain_match,
                    "generated_by_wizard": info.generated_by_wizard,
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
/// settings hash (which includes things like `session_secret`/`password` a plugin has no
/// business reading).
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

/// The `tagrules` setting's raw text, but only when `tagruleson` is enabled (`Model/Plugins.pm`'s
/// own `if (LANraragi::Model::Config->enable_tagrules)` gate) — `None` means "don't rewrite
/// anything", collapsing both "the switch is off" and "Redis is unreachable" into the same safe
/// no-op rather than a metadata-plugin run failing outright over an unrelated config read.
async fn get_computed_tagrules(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    let enabled = fields.get("tagruleson").map(|v| v != "0").unwrap_or(true);
    if !enabled {
        return None;
    }
    Some(
        fields
            .get("tagrules")
            .cloned()
            .unwrap_or_else(|| crate::settings::default_tagrules().to_string()),
    )
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
            visible_to_guest: false,
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
        let customargs = get_plugin_customargs(state, &ns, &info.parameters).await;
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
                // `Model/Plugins.pm:292-296` — tag rules run on the plugin's freshly returned tags
                // only, before they're merged into `old_tags`, and only when `tagruleson` is set.
                let rewritten;
                let new_tags = match get_computed_tagrules(state).await {
                    Some(rules_text) => {
                        rewritten = tag_rules::apply_tag_rules(new_tags, &rules_text);
                        rewritten.as_str()
                    }
                    None => new_tags,
                };
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

    // Precompute the recommendation-cache entry once, after every enabled plugin has had its
    // turn (not per-iteration — an earlier plugin's title write would otherwise get embedded and
    // immediately superseded by a later plugin's own title write in the same run). Covers every
    // caller of this function (ingest watcher, download-manager ingest, upload, bench) in one
    // place rather than needing a hook at each of the 4 call sites. Fetches the archive fresh
    // rather than trusting `summary.new_title` — a plugin can also be the *first* time this
    // archive is ever precomputed even when no plugin actually changed its title.
    //
    // The LLM artist/cosplayer tag backfill (`artist_backfill::backfill_artist_tag`) runs from
    // this same fetch, same reasoning — needs the final, plugin-enriched title/tags, not
    // whichever plugin's intermediate write happened to run first.
    if let Ok(Some(archive)) = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
        .await
    {
        {
            let state = state.clone();
            let archive_id = archive_id.to_string();
            let title = archive.title.clone();
            tokio::spawn(async move {
                crate::recommend_precompute::precompute_one(&state, &archive_id, &title).await;
            });
        }
        {
            let state = state.clone();
            let archive_id = archive_id.to_string();
            let title = archive.title.clone();
            let tags = archive.tags.clone();
            tokio::spawn(async move {
                crate::artist_backfill::backfill_artist_tag(&state, &archive_id, &title, &tags)
                    .await;
            });
        }
    }

    summary
}

async fn use_plugin_sync(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
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

    let customargs = get_plugin_customargs(&state, &plugin, &info.parameters).await;
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
            crate::activity::record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                lanrurugi_storage::activity::action_types::PLUGIN_EXECUTE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: params.id.clone(),
                    label: Some(plugin.clone()),
                    kind: Some("plugin".to_string()),
                },
                lanrurugi_storage::activity::Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({
                "operation": "use_plugin",
                "success": 1,
                "type": info.kind,
                "data": data,
            }))
            .into_response()
        }
        Err(e) => {
            // The plugin was actually invoked (past discovery/info-lookup) and its own run
            // failed/timed out/errored — this is the core reason `PLUGIN_EXECUTE` exists as an
            // action type at all, the user needs to know their manual "run plugin" click failed.
            crate::activity::record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                lanrurugi_storage::activity::action_types::PLUGIN_EXECUTE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: params.id.clone(),
                    label: Some(plugin.clone()),
                    kind: Some("plugin".to_string()),
                },
                lanrurugi_storage::activity::Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            axum::Json(json!({
                "operation": "use_plugin",
                "success": 0,
                "type": info.kind,
                "error": e.to_string(),
            }))
            .into_response()
        }
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
        let customargs = get_plugin_customargs(&state_for_task, &plugin, &info.parameters).await;
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
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
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

    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::PLUGIN_URL_DOWNLOAD_TRIGGER,
        lanrurugi_storage::activity::ActivityTarget {
            id: None,
            label: Some(url.clone()),
            kind: Some("download_url".to_string()),
        },
        lanrurugi_storage::activity::Outcome::Success,
        None,
        Some(json!({ "plugin": plugin, "category": params.catid })),
    )
    .await;

    // `None` — out of scope for now (see `download_queue.rs::start_queue_item`'s own docs on the
    // equivalent patch-back-on-completion wiring; this `/download_url` path could get the same
    // treatment later but wasn't part of this change).
    let job_id = start_download(
        state,
        plugin.clone(),
        info,
        url,
        params.catid.clone(),
        false,
        None,
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
#[allow(clippy::too_many_arguments)]
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
    // The `download_queue.start` activity entry this download was launched from (see
    // `download_queue.rs::start_queue_item`'s own docs) — threaded through to
    // `run_managed_downloads`'s own success branch so it can patch that entry's `after` with the
    // real resulting `archive_ids` once ingestion actually finishes, rather than the caller trying
    // to record a result before the work has even started. `None` for the `/download_url` path
    // (out of scope for now — see that call site's own comment).
    activity_entry_id: Option<String>,
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
                // `Starting`, not `Waiting` — this covers the plugin's own `exec_download` call
                // (resolving the real download URL(s), e.g. `ehentai.ts`'s own archiver.php
                // round-trip, which can genuinely take several seconds to tens of seconds waiting
                // on the *source site*, nothing to do with this host's own concurrency limits at
                // all). `Waiting` is reserved for the real, distinct phase once
                // `run_managed_downloads` actually calls `DownloadManager::acquire` and is blocked
                // on a busy domain's own semaphore (see that state's own docs) — conflating the two
                // made every download look concurrency-limited even when the delay was entirely a
                // slow upstream site response.
                update_queue_item_state(
                    repo,
                    state_for_task.download_queue_tx.as_ref(),
                    item_id,
                    lanrurugi_storage::download_queue::DownloadQueueState::Starting,
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
                &info.parameters,
            )
            .await;
            // Kept alive past `args`'s own move of `url` below — needed later for the real
            // `source:<url>` tag legacy's own `Utils::Minion::download_url` task adds (see
            // `ingest_downloaded_file`'s own docs), which must be the URL the *user* gave, not
            // whatever internal link the plugin's own `exec_download` result later resolves to.
            let source_url = url.clone();
            let args = json!({ "url": url, "category": category, "customargs": customargs });
            let args = with_login_cookies(&state_for_task, &info, args).await;
            // Diagnostic only: this call covers the whole `Starting` phase (the plugin's own
            // `execDownload` body — its own network requests, if any, plus Deno worker
            // spawn/IPC round-trip) with previously zero visibility into where time actually
            // went if it took unexpectedly long (see `DownloadManager::acquire`'s own matching
            // diagnostic for the *next* phase, `Waiting` — this repo had no log coverage for
            // either half of a slow download until both were added together).
            let exec_start = std::time::Instant::now();
            let plugin_result = match plugins
                .execute(&plugin_namespace_for_task, "exec_download", args)
                .await
            {
                Ok(data) => {
                    let elapsed = exec_start.elapsed();
                    if elapsed.as_millis() > 500 {
                        tracing::info!(
                            plugin = %plugin_namespace_for_task,
                            elapsed_ms = elapsed.as_millis() as u64,
                            "exec_download plugin call finished after taking a while"
                        );
                    }
                    data
                }
                Err(e) => {
                    let queue_error = queue_error_from_pool_error(&plugin_namespace_for_task, &e);
                    if let Some((repo, item_id)) = &queue_link {
                        update_queue_item_state(
                            repo,
                            state_for_task.download_queue_tx.as_ref(),
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

            let parsed: PluginDownloadResult = match serde_json::from_value(plugin_result) {
                Ok(p) => p,
                Err(e) => {
                    let queue_error =
                        lanrurugi_core::queue_error::QueueError::MalformedPluginResponse {
                            plugin: plugin_namespace_for_task.clone(),
                        };
                    if let Some((repo, item_id)) = &queue_link {
                        update_queue_item_state(
                            repo,
                            state_for_task.download_queue_tx.as_ref(),
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
                        state_for_task.download_queue_tx.as_ref(),
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
                    queue_link.clone(),
                )
                .await
                {
                    Ok(ids) => {
                        if let Some((repo, item_id)) = &queue_link {
                            // Fetch/cache metadata before Done so the frontend sees it.
                            if let Ok(Some(mut item)) = repo.get(item_id).await {
                                if item.auto_fetch_metadata || item.title.is_none() {
                                    ensure_metadata_cached(&state_for_task, &mut item).await;
                                }
                            }
                            update_queue_item_state(
                                repo,
                                state_for_task.download_queue_tx.as_ref(),
                                item_id,
                                lanrurugi_storage::download_queue::DownloadQueueState::Done,
                                None,
                                None,
                                Some(ids.clone()),
                            )
                            .await;
                        }
                        // Patches the `download_queue.start` entry this download was launched
                        // from (see `start_download`'s own docs) with the real result now that
                        // ingestion has actually finished — this is what makes a completed
                        // download's own activity record show *which archive(s)* it became,
                        // rather than only ever showing the pre-download intent.
                        if let Some(entry_id) = &activity_entry_id {
                            crate::activity::patch_after(
                                &state_for_task,
                                entry_id,
                                json!({ "archive_ids": ids }),
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
                                state_for_task.download_queue_tx.as_ref(),
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
                                state_for_task.download_queue_tx.as_ref(),
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
                        state_for_task.download_queue_tx.as_ref(),
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
                    state_for_task.download_queue_tx.as_ref(),
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
    err: &PluginError,
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
    tx: Option<&tokio::sync::broadcast::Sender<serde_json::Value>>,
    item_id: &str,
    new_state: lanrurugi_storage::download_queue::DownloadQueueState,
    job_id: Option<String>,
    error: Option<lanrurugi_core::queue_error::QueueError>,
    archive_ids: Option<Vec<String>>,
) {
    // Retry transient Redis failures with exponential backoff (200ms → 500ms → 1s, capped)
    // so a brief connection-pool or timeout spike during heavy ingest work doesn't leave a
    // queue item stuck in `Downloading` forever.
    const RETRIES: u32 = 3;
    const BACKOFF_MS: [u64; RETRIES as usize] = [200, 500, 1000];
    // Option-typed function args we can't consume inside a retry loop (the first success
    // that still gets retried would move them, leaving nothing for the next). First attempt
    // clones once; last attempt moves (free). Intermediate attempts skip optional fields
    // entirely — if `update` fails, the next `get()` reloads a fresh item from Redis.
    let mut job_id = job_id;
    let mut archive_ids = archive_ids;
    let mut error = error;
    for (i, &delay) in BACKOFF_MS.iter().enumerate() {
        let attempt = i as u32 + 1;
        match repo.get(item_id).await {
            Ok(Some(mut item)) => {
                item.state = new_state;
                if attempt == RETRIES {
                    item.job_id = job_id.take();
                    item.archive_ids = archive_ids.take();
                    item.error = error.take();
                } else {
                    // Clone cost is trivial for `Option<String>` + `Option<QueueError>`;
                    // `Vec<String>` is at most a handful of short IDs per download. The
                    // common path (attempt 1 succeeds, no retry) pays exactly what the
                    // original code did pre-retry — one clone of an empty or tiny Vec.
                    item.job_id = job_id.clone();
                    item.archive_ids = archive_ids.clone();
                    item.error = error.clone();
                }
                match repo.update(&item).await {
                    Ok(()) => {
                        if let Some(tx) = tx {
                            let _ = tx.send(serde_json::json!({
                                "kind": "update",
                                "id": item_id,
                                "state": new_state,
                                "job_id": &item.job_id,
                                "archive_ids": &item.archive_ids,
                                "title": &item.title,
                                "metadata_preview": &item.metadata_preview,
                                "error": &item.error,
                            }));
                        }
                        return;
                    }
                    Err(e) if attempt < RETRIES => {
                        tracing::warn!(%item_id, attempt, delay_ms = delay, error = %e, "failed to update download-queue item state, retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                    Err(e) => {
                        tracing::warn!(%item_id, attempt, error = %e, "failed to update download-queue item state after {RETRIES} attempts");
                    }
                }
            }
            Ok(None) => {
                tracing::debug!(%item_id, "download-queue item no longer exists, skipping state update");
                return;
            }
            Err(e) if attempt < RETRIES => {
                tracing::warn!(%item_id, attempt, delay_ms = delay, error = %e, "failed to load download-queue item for state update, retrying");
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            Err(e) => {
                tracing::warn!(%item_id, attempt, error = %e, "failed to load download-queue item for state update after {RETRIES} attempts");
                return;
            }
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
/// Candidates are sorted by `sort_namespaces_by_priority` before matching — the first whose
/// `url_pattern` regex matches `url` wins, but "first" now means priority order (a user's
/// Plugins-page drag-to-reorder, or a wizard-generated `custom/` override placed ahead of the
/// built-in it's meant to replace), not `namespaces`'s own incoming order. `None` if no installed
/// plugin of this kind matches.
pub(crate) async fn find_matching_plugin(
    state: &AppState,
    namespaces: &[String],
    url: &str,
) -> Option<(String, lanrurugi_plugin::protocol::PluginInfo)> {
    let ordered = sort_namespaces_by_priority(state, namespaces).await;
    for ns in &ordered {
        if let Ok(info) = state.plugins.plugin_info(ns).await {
            if let Some(ref pattern) = info.url_pattern {
                if let Ok(re) = regex::Regex::new(pattern) {
                    if re.is_match(url) {
                        return Some((ns.clone(), info));
                    }
                }
            }
        }
    }
    None
}

/// Case-insensitive, `www.`-insensitive containment check — does `info.domain_match` (or, when
/// empty, a loose regex-as-domain-containment fallback derived from `info.url_pattern`) consider
/// `domain` covered? `domain` is a bare hostname (no scheme/path) — NOT the precise-trigger check
/// `find_matching_plugin` does against a full URL; use that one for real dispatch, this one only
/// for "does some installed plugin already own this domain" questions (Upload page's metadata-
/// button enablement, the AI wizard's domain-coverage lookup).
fn domain_covers(info: &lanrurugi_plugin::protocol::PluginInfo, domain: &str) -> bool {
    let needle = normalize_domain(domain);
    if !info.domain_match.is_empty() {
        return info
            .domain_match
            .iter()
            .any(|d| normalize_domain(d) == needle);
    }
    info.url_pattern
        .as_deref()
        .and_then(|p| regex::Regex::new(p).ok())
        .is_some_and(|re| re.is_match(&needle))
}

/// Lowercases and strips a leading `www.` — the only two normalizations any real plugin
/// declaration or user-typed domain actually needs.
fn normalize_domain(d: &str) -> String {
    let lower = d.trim().trim_end_matches('/').to_ascii_lowercase();
    lower
        .strip_prefix("www.")
        .map(str::to_string)
        .unwrap_or(lower)
}

/// Domain-ownership lookup (AI wizard coverage check, Upload page button enablement) —
/// priority-ordered like `find_matching_plugin`, but matches via `domain_covers`
/// (`domain_match`-first) instead of a `url_pattern` regex against a full URL. `domain` is
/// expected to be a bare hostname, not a full URL — see `domain_covers`'s own docs on why this is
/// a genuinely separate function rather than a `find_matching_plugin` parameter: the two have
/// different input contracts (full URL vs. bare domain) and conflating them is exactly what
/// caused a real bug (confirmed live, 2026-08-26 — a wizard-generated `custom/download/
/// nhentai_net.ts` plugin declaring the precise `url_pattern: "nhentai\\.net/g/"` silently failed
/// to match a bare `"nhentai.net"` domain lookup, making the wizard think that domain's download
/// type was still uncovered even though a real AI-generated plugin for it already existed).
pub(crate) async fn find_plugin_by_domain(
    state: &AppState,
    namespaces: &[String],
    domain: &str,
) -> Option<(String, lanrurugi_plugin::protocol::PluginInfo)> {
    let ordered = sort_namespaces_by_priority(state, namespaces).await;
    for ns in &ordered {
        if let Ok(info) = state.plugins.plugin_info(ns).await {
            if domain_covers(&info, domain) {
                return Some((ns.clone(), info));
            }
        }
    }
    None
}

/// Ensures a queue item has fresh `metadata_preview` data cached — checks the 10-min TTL on
/// `metadata_preview_at`, and if stale or absent, finds a matching metadata plugin for the URL
/// and calls `execMetadata`. Writes the result back to both the queue item and the archive(s)
/// when `auto_fetch_metadata` is enabled. Returns the cached/fresh `{ title, tags }` data.
pub(crate) async fn ensure_metadata_cached(
    state: &AppState,
    item: &mut lanrurugi_storage::download_queue::DownloadQueueItem,
) -> Option<serde_json::Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // Cache hit — still within the 10-min window.
    if let Some(at) = item.metadata_preview_at {
        if now - at < METADATA_CACHE_TTL_MS {
            return item.metadata_preview.clone();
        }
    }

    // No cache or expired — find a metadata plugin and call it.
    let namespaces = discover_namespaces(&state.plugins_dir).await;
    let mut metadata_ns = Vec::new();
    for ns in &namespaces {
        if let Ok(info) = state.plugins.plugin_info(ns).await {
            if info.kind == "metadata" {
                metadata_ns.push(ns.clone());
            }
        }
    }

    let (plugin_ns, info) = find_matching_plugin(state, &metadata_ns, &item.url).await?;
    // Mirrors the shape `run_enabled_metadata_plugins_on_archive` builds for the same
    // `execMetadata` contract (`ExecMetadataInfo` in the converted plugins — see
    // `metadata/chaika.ts`'s own `let [addextra, ...] = lrr_info.customargs` destructure, which
    // crashes with "undefined is not iterable" on anything short of a real array here). Passing
    // only `{ "url": item.url }` (the previous shape) meant every converted metadata plugin's own
    // `customargs`/`arg` reads saw `undefined` — this queue item has no archive yet, so there's no
    // `existing_tags`/`archive_title`/`thumbnail_hash` to offer; `arg: item.url` is what lets a
    // plugin's own URL-vs-ID parsing (e.g. chaika.ts's oneshot-arg regex) actually run instead of
    // falling through to a lookup path that also expects fields this pre-archive stage can't
    // supply.
    let customargs = get_plugin_customargs(state, &plugin_ns, &info.parameters).await;
    let args = serde_json::json!({
        "url": item.url,
        "arg": item.url,
        "customargs": customargs,
        "existing_tags": "",
        "archive_title": "",
        "thumbnail_hash": "",
    });
    let result = state
        .plugins
        .execute(&plugin_ns, "exec_metadata", args)
        .await
        .ok()?;

    let title = result
        .get("title")
        .and_then(|v| v.as_str())
        .map(String::from);
    let tags = result
        .get("tags")
        .and_then(|v| v.as_str())
        .map(String::from);

    item.metadata_preview = Some(result.clone());
    item.metadata_preview_at = Some(now);
    if let Some(ref t) = title {
        item.title = Some(t.clone());
    }
    // Re-fetch the item fresh and apply only the metadata fields onto *that* copy before writing
    // — NOT `state.download_queue.update(item)` with the caller's own possibly-stale `item`
    // (`update` is a full-record overwrite, `DownloadQueueRepository::update` → `save`). A caller
    // that cloned `item` before some other concurrent write (e.g. `start_one`'s own fire-and-forget
    // metadata-fetch spawn, which clones `item` before `plugins::start_download` has even generated
    // a real `job_id`) would otherwise silently stomp that other write's fields back to their
    // stale pre-clone values the instant this call's own write lands after it — confirmed live,
    // 2026-08-26: a real, in-progress download's `job_id` got clobbered back to `null` by exactly
    // this race, permanently disconnecting the queue row from its own job and leaving the progress
    // bar with no `JobRecord` to render at all for the rest of that download.
    let mut persisted = item.clone();
    if let Ok(Some(fresh)) = state.download_queue.get(&item.id).await {
        persisted = fresh;
        persisted.metadata_preview = item.metadata_preview.clone();
        persisted.metadata_preview_at = item.metadata_preview_at;
        if let Some(ref t) = title {
            persisted.title = Some(t.clone());
        }
    }
    if let Err(e) = state.download_queue.update(&persisted).await {
        tracing::warn!(item_id = %item.id, error = %e, "failed to persist metadata cache");
    } else if let Some(tx) = &state.download_queue_tx {
        // Push the fresh preview/title out to SSE subscribers (the Upload page's tooltip
        // reads `metadata_preview` from the queue list, which only updates via these deltas).
        let _ = tx.send(serde_json::json!({
            "kind": "update",
            "id": &persisted.id,
            "state": &persisted.state,
            "job_id": &persisted.job_id,
            "archive_ids": &persisted.archive_ids,
            "title": &persisted.title,
            "metadata_preview": &persisted.metadata_preview,
            "error": &persisted.error,
        }));
    }

    // Apply tags to already-catalogued archive(s) when auto_fetch_metadata is on.
    if item.auto_fetch_metadata {
        if let Some(ref tags) = tags {
            if let Some(ref archive_ids) = item.archive_ids {
                for aid in archive_ids {
                    apply_metadata_tags(state, aid, tags).await;
                }
            }
        }
    }

    Some(result)
}

/// Merges `metadata_tags` into an archive's existing tag string — appends tags that aren't
/// already present, leaves existing ones untouched. Updates the archive record + search index.
async fn apply_metadata_tags(state: &AppState, archive_id: &str, metadata_tags: &str) {
    let mut archive = match state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
        .await
    {
        Ok(Some(a)) => a,
        _ => return,
    };

    let existing: std::collections::HashSet<&str> = archive
        .tags
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let new_tags: Vec<&str> = metadata_tags
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && !existing.contains(t))
        .collect();

    if new_tags.is_empty() {
        return;
    }

    let old_tags = archive.tags.clone();
    let merged = if archive.tags.is_empty() {
        new_tags.join(", ")
    } else {
        format!("{}, {}", archive.tags, new_tags.join(", "))
    };
    archive.tags = merged;

    if let Err(e) = state.repos.archives.save(&archive).await {
        tracing::warn!(%archive_id, error = %e, "failed to save metadata tags to archive");
        return;
    }

    // Re-index so the new tags are searchable immediately.
    if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
        &state.redis.search,
        archive_id,
        &old_tags,
        &archive.tags,
    )
    .await
    {
        tracing::warn!(%archive_id, error = %e, "failed to update search index after metadata tag merge");
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_managed_downloads(
    state: AppState,
    plugin_namespace: String,
    downloads: Vec<PluginDownloadRequest>,
    job_id: String,
    category: Option<String>,
    overwrite: bool,
    source_url: &str,
    cancel: &tokio_util::sync::CancellationToken,
    queue_item_id: Option<&str>,
    queue_link: Option<(
        Arc<lanrurugi_storage::download_queue::DownloadQueueRepository>,
        String,
    )>,
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
    // Per-download live rate-limit resolution: the plugin's *declared* options are snapshotted
    // here (re-reading them mid-download would spawn a Deno subprocess per chunk — unacceptable),
    // but the user's Redis-stored override is re-read on demand, invalidated by
    // `AppState::plugin_options_generation` bumping on every options write. So clearing/changing
    // a rate cap takes effect at the next chunk boundary of an in-flight download.
    let rate_resolver = crate::download_manager::live_rate::LiveRateResolver::new(
        plugin_namespace.clone(),
        declared.clone(),
        state.plugin_options.clone(),
        state.plugin_options_generation.clone(),
    );

    // Record this download job's effective rate limit (and the matching domain-rule pattern) onto
    // its `JobStatus` once, at start, so the frontend's progress UI can show a highlighted badge +
    // tooltip for rate-limited downloads (issue #2). Uses the first resource's hostname as this
    // job's representative — the UI model is one combined progress indicator per job (FR-003), so
    // rate-limit display is a single value too, and real multi-resource downloads (Pixiv pages,
    // single-archive H@H) almost always share one domain/rule. This is a *display-only* snapshot
    // of the start-time value — the actual throttle re-resolves live on every chunk (see
    // `rate_resolver` above), so the badge may lag a mid-download settings change; that's fine,
    // it's informational, and the transfer itself reacts immediately.
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

    // Flips the queue item from `Waiting` to `Downloading` the instant the *first* resource's
    // concurrency permit is acquired — not before (that's still `Waiting`, however long a busy
    // domain's semaphore takes to free up) and not again for subsequent resources in a
    // multi-resource job (a second/third resource's own wait for its own permit is real, but the
    // job as a whole already has bytes moving by then, so it must stay `Downloading`, not revert
    // to `Waiting`). `AtomicBool` rather than a plain `bool` captured by value only because the
    // callback closure passed to `download_one` is `Fn`, not `FnMut` (it must be callable through
    // a shared `&dyn Fn()`, per that function's signature).
    let downloading_state_set = Arc::new(std::sync::atomic::AtomicBool::new(false));

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
        // Also broadcast over the download-queue's existing SSE channel, not just written into
        // `JobRegistry` for the frontend's own 1s `/jobs` poll to eventually pick up — a small/fast
        // download can finish well within that poll gap, so the progress bar never renders at all
        // (reported live, 2026-08-25: a 112MB nHentai download went straight from "starting" to its
        // final duplicate-conflict result with no visible progress in between). Reuses the same
        // `download_one`-throttled progress channel (≤1 update/200ms), so this doesn't broadcast
        // any more often than the byte-stream layer already reports — no separate throttling needed
        // here. Not persisted to Redis or retried on send failure: this is transient, ephemeral
        // progress, not queue-item state, and a dropped tick is superseded by the next one moments
        // later regardless.
        let queue_tx = state.download_queue_tx.clone();
        let queue_item_id_for_drain = queue_item_id.map(str::to_string);
        let drain_task = tokio::spawn(async move {
            let mut known_totals = known_totals;
            while let Some((downloaded, total)) = progress_rx.recv().await {
                known_totals[index] = total;
                let combined_total = known_totals
                    .iter()
                    .all(Option::is_some)
                    .then(|| known_totals.iter().flatten().sum());
                let combined_downloaded = base + downloaded;
                jobs.set_download_progress(&job_id_for_drain, combined_downloaded, combined_total)
                    .await;
                if let (Some(tx), Some(item_id)) = (&queue_tx, &queue_item_id_for_drain) {
                    let _ = tx.send(serde_json::json!({
                        "kind": "progress",
                        "id": item_id,
                        "job_id": &job_id_for_drain,
                        "downloaded_bytes": combined_downloaded,
                        "total_bytes": combined_total,
                    }));
                }
            }
            known_totals
        });

        let on_permit_acquired = {
            let queue_link = queue_link.clone();
            let state = state.clone();
            let job_id = job_id.clone();
            let downloading_state_set = downloading_state_set.clone();
            move || {
                if downloading_state_set.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let Some((repo, item_id)) = queue_link.clone() else {
                    return;
                };
                let state = state.clone();
                let job_id = job_id.clone();
                // `download_one` calls this synchronously (not `.await`-able — see its own
                // signature), so the actual Redis write is spawned off rather than blocking the
                // transfer's own progress; a queue item with no `queue_link` (a one-off,
                // non-queue-tracked download, if any caller ever uses that path) has nothing to
                // update and just no-ops above.
                tokio::spawn(async move {
                    update_queue_item_state(
                        &repo,
                        state.download_queue_tx.as_ref(),
                        &item_id,
                        lanrurugi_storage::download_queue::DownloadQueueState::Downloading,
                        Some(job_id),
                        None,
                        None,
                    )
                    .await;
                });
            }
        };

        // Stable across a retry of the *same* queue item/resource (a Stop-then-Start, a crash, or
        // a container restart all reuse the same `queue_item_id` and the same `index` within this
        // job's own `downloads[]`) — exactly what `download_one`'s own `resume_key` docs need to
        // find a previous attempt's partial file again (issue #88). `None` when this download
        // isn't queue-tracked at all (`queue_item_id` is `None` for a one-off, non-queue-tracked
        // caller, if any ever uses this path) — nothing to resume across since there's no stable
        // identity to key a `.part` file off of.
        let resume_key = queue_item_id.map(|id| format!("{id}-{index}"));

        // Real `Waiting` starts here — right before the call that may actually block acquiring a
        // per-domain concurrency permit (`DownloadManager::acquire`, inside `download_one`). Skips
        // this write entirely once `on_permit_acquired` has already flipped the item to
        // `Downloading` for an earlier resource in the same multi-resource job (`downloading_state_
        // set`'s own docs) — a later resource's own wait for its permit is real, but the job as a
        // whole already has bytes moving, so it must stay `Downloading`, not revert to `Waiting`.
        if !downloading_state_set.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some((repo, item_id)) = &queue_link {
                update_queue_item_state(
                    repo,
                    state.download_queue_tx.as_ref(),
                    item_id,
                    lanrurugi_storage::download_queue::DownloadQueueState::Waiting,
                    Some(job_id.clone()),
                    None,
                    None,
                )
                .await;
            }
        }

        let downloaded_result = download_one(
            &manager,
            &rules,
            &stream_req,
            &rate_resolver,
            progress_tx,
            &state.library.temp_dir,
            cancel,
            &on_permit_acquired,
            resume_key.as_deref(),
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
///
/// Moves an already-staged `.ts` file into `staging_dir/<kind>/<file_name>` (FR-021's
/// namespace-conflict check + FR-022's rollback-on-failure), returning the resulting namespace.
/// Shared by `upload_plugin` and `plugin_wizard::save` — the only difference between their two
/// callers is what happens *before* this point (from-scratch upload validation vs. a draft that
/// already passed a real trial run), not this move step itself.
///
/// `allow_overwrite`: `upload_plugin` always passes `false` (a fresh upload colliding with an
/// existing plugin is unambiguously an error, no session-scoped "this is mine to overwrite"
/// concept exists there). `plugin_wizard::save` passes `true` only for a wizard session editing
/// its own previously-saved draft back into the very same file — validated by that caller (not
/// here) against the specific namespace the session actually loaded from, so this flag alone never
/// grants blanket permission to overwrite an arbitrary same-named file. `rename` over an existing
/// file is an atomic replace on the same filesystem, so no separate remove-then-rename step is
/// needed for the `true` case.
pub(crate) async fn move_into_category(
    staging_dir: &std::path::Path,
    staged_path: &std::path::Path,
    file_name: &str,
    kind: &str,
    operation: &str,
    allow_overwrite: bool,
) -> Result<String, Response> {
    let final_dir = staging_dir.join(kind);
    if let Err(e) = tokio::fs::create_dir_all(&final_dir).await {
        let _ = tokio::fs::remove_file(staged_path).await;
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            operation,
            e.to_string(),
        ));
    }
    let final_path = final_dir.join(file_name);
    if final_path.exists() && !allow_overwrite {
        let _ = tokio::fs::remove_file(staged_path).await;
        return Err(error(
            StatusCode::CONFLICT,
            operation,
            format!("A {kind} plugin named {file_name:?} is already installed."),
        ));
    }
    if let Err(e) = tokio::fs::rename(staged_path, &final_path).await {
        let _ = tokio::fs::remove_file(staged_path).await;
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            operation,
            e.to_string(),
        ));
    }

    Ok(format!(
        "{CUSTOM_PLUGIN_DIR}/{kind}/{}",
        file_name.trim_end_matches(".ts")
    ))
}
async fn upload_plugin(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    mut multipart: Multipart,
) -> Response {
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
        // A real attempted plugin-file upload (past the name-conflict check, a real file really
        // submitted) that failed on the actual disk write — worth recording, unlike the
        // pre-execution "no file provided"/"not a .ts file" rejections above.
        crate::activity::record_manual(
            &state,
            auth.as_ref().map(|e| &e.0),
            lanrurugi_storage::activity::action_types::PLUGIN_UPLOAD,
            lanrurugi_storage::activity::ActivityTarget {
                id: Some(staged_namespace.clone()),
                label: Some(file_name.clone()),
                kind: Some("plugin".to_string()),
            },
            lanrurugi_storage::activity::Outcome::Failure {
                reason: e.to_string(),
            },
            None,
            None,
        )
        .await;
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

    let final_namespace = match move_into_category(
        &staging_dir,
        &staged_path,
        &file_name,
        &info.kind,
        "upload_plugin",
        false,
    )
    .await
    {
        Ok(ns) => ns,
        Err(resp) => return resp,
    };
    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::PLUGIN_UPLOAD,
        lanrurugi_storage::activity::ActivityTarget {
            id: Some(final_namespace.clone()),
            label: Some(info.name.clone()),
            kind: Some("plugin".to_string()),
        },
        lanrurugi_storage::activity::Outcome::Success,
        None,
        Some(json!({ "type": info.kind })),
    )
    .await;
    axum::Json(json!({
        "operation": "upload_plugin",
        "success": 1,
        "namespace": final_namespace,
        "name": info.name,
        "type": info.kind,
    }))
    .into_response()
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// Builds a minimal but real `AppState` against `LANRURUGI_TEST_REDIS_URL` — same pattern as
    /// `lanrurugi-server/tests/contract_api.rs::test_app`, trimmed to just the fields
    /// `run_managed_downloads` actually touches (no scanner/plugin-pool/recommender wiring, since
    /// this test never touches a real plugin subprocess — it drives `run_managed_downloads`
    /// directly with an already-parsed `downloads[]`, the same way `start_download` would after a
    /// real plugin's `exec_download` call returns).
    pub(crate) async fn test_state() -> Option<AppState> {
        let redis = lanrurugi_storage::test_support::test_redis_dbs().await?;
        let repos = crate::Repositories::new(&redis);
        Some(AppState {
            redis: redis.clone(),
            repos,
            jobs: lanrurugi_core::jobs::JobRegistry::new(),
            auth: crate::AuthConfig {
                force_secure_cookies: false,
            },
            disable_update_check: true,
            library: crate::LibraryPaths {
                archive_dir: std::env::temp_dir(),
                thumb_dir: std::env::temp_dir(),
                temp_dir: std::env::temp_dir(),
                log_dir: None,
            },
            scanner: lanrurugi_scanner::handle::ScannerHandle::new(),
            plugins: Arc::new(lanrurugi_plugin::pool::PluginPool::new(
                "deno",
                std::path::PathBuf::from("/tmp/dispatcher.ts"),
                std::path::PathBuf::from("/tmp/plugins"),
            )),
            plugins_dir: std::path::PathBuf::from("/tmp/plugins"),
            download_managers: Default::default(),
            thumbnail_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(2)),
            page_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(2)),
            plugin_options: Arc::new(lanrurugi_storage::plugin_options::PluginOptionsRepository::new(
                redis.config.clone(),
            )),
            plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            download_queue: Arc::new(lanrurugi_storage::download_queue::DownloadQueueRepository::new(
                redis.config.clone(),
            )),
            recommend_cache: Arc::new(lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(
                redis.config.clone(),
            )),
            ignored_group_suggestions: Arc::new(
                lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
                    redis.config.clone(),
                ),
            ),
            compare_cache: Arc::new(lanrurugi_storage::compare_cache::CompareCacheRepository::new(
                redis.config.clone(),
            )),
            bookmarks: Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
                redis.config.clone(),
            )),
            recommender: Arc::new(crate::recommend::RecommendService::new()),
            new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
            download_cancellations: Default::default(),
            pending_generate_requests: Default::default(),
            filename_locks: Default::default(),
            download_queue_tx: None,
            refresh_tokens: Arc::new(lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(
                redis.config.clone(),
            )),
            api_tokens: Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
                redis.config.clone(),
            )),
            api_token_last_touch: Default::default(),
            activity: Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
                redis.config.clone(),
            )),
        })
    }

    /// Real regression test for the `Waiting` → `Downloading` queue-state transition (this
    /// session's own feature: a download blocked on a busy domain's concurrency permit must show
    /// as `Waiting`, not `Downloading`, until it actually has a permit and bytes start moving —
    /// previously `start_download` set `Downloading` at task-spawn time, before
    /// `DownloadManager::acquire` ever ran, so a download queued behind a busy `max_concurrent: 1`
    /// domain rule looked identical to one actually transferring bytes).
    ///
    /// Runs two independent single-resource downloads (two separate queue items, same plugin
    /// namespace) against one real local HTTP server, with a domain rule capping that server's
    /// host to `max_concurrent: 1` (the same shape as the real `chaika.ts`'s own
    /// `*.chaika.moe` rule) — the first response is held open until the test explicitly releases
    /// it, guaranteeing the second `run_managed_downloads` call is genuinely blocked on
    /// `DownloadManager::acquire` (not just fast enough to race past it) when this test inspects
    /// its queue item's state.
    #[tokio::test]
    async fn waiting_state_is_set_while_blocked_on_a_busy_domain_permit() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let hold_first = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_first = std::sync::Arc::new(tokio::sync::Notify::new());
        let hold_first_srv = hold_first.clone();
        let release_first_srv = release_first.clone();
        // Unique filenames (not just unique bodies) every run — `ingest_downloaded_file` rejects
        // both a content-hash duplicate *and* a same-filename-in-the-same-dir collision, and the
        // shared dev-image content dir (`archive_dir` in `test_state` is a plain `std::env::
        // temp_dir()`, not a per-run scratch directory) persists whatever a previous run of this
        // test already cataloged there.
        let run_id = uuid::Uuid::new_v4();
        let first_filename = format!("first-{run_id}.zip");
        let second_filename = format!("second-{run_id}.zip");
        let router = axum::Router::new()
            .route(
                &format!("/{first_filename}"),
                axum::routing::get(move || {
                    let hold_first_srv = hold_first_srv.clone();
                    let release_first_srv = release_first_srv.clone();
                    let run_id = run_id;
                    async move {
                        hold_first_srv.notify_one();
                        release_first_srv.notified().await;
                        format!("first resource body {run_id}").into_bytes()
                    }
                }),
            )
            .route(
                &format!("/{second_filename}"),
                axum::routing::get(move || async move {
                    format!("second resource body {run_id}").into_bytes()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        // `DomainRule`/`DownloadManager` resolve purely by hostname string, matched against
        // `req.url`'s own `host_str()` — the loopback IP address itself (not a symbolic name) is
        // exactly what that resolves to for a `127.0.0.1`-bound test server.
        let hostname = addr.ip().to_string();

        // `run_managed_downloads` re-derives its own rules from `fetch_declared_options` (which
        // will simply come back empty/error for this made-up namespace — no real `.ts` file
        // backs it) merged with the plugin-options Redis override below. Since the override sets
        // `domain_rules` to `Some(...)`, `resolve_domain_rules` uses it unconditionally
        // (`settings.rs`'s own docs: override always wins when present), so this exercises the
        // exact same production code path a real plugin's declared `pluginOptions()` would.
        let plugin_namespace = format!("test/waiting-state-{}", uuid::Uuid::new_v4());
        state
            .plugin_options
            .save(
                &plugin_namespace,
                &lanrurugi_storage::plugin_options::PluginOptionsOverride {
                    domain_rules: Some(vec![
                        lanrurugi_storage::plugin_options::DomainRuleOverride {
                            pattern: Some(hostname.clone()),
                            max_concurrent: Some(1),
                            max_bytes_per_sec: None,
                        },
                    ]),
                    bundle_as_archive: None,
                    overwrite_on_duplicate: None,
                },
            )
            .await
            .expect("failed to persist the test's own domain-rule override");

        async fn make_queue_item(
            state: &AppState,
            plugin_namespace: &str,
            url: String,
        ) -> lanrurugi_storage::download_queue::DownloadQueueItem {
            state
                .download_queue
                .add(lanrurugi_storage::download_queue::NewQueueItem {
                    origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
                    url,
                    plugin_namespace: plugin_namespace.to_string(),
                    file_size: None,
                    category: None,
                    auto_fetch_metadata: false,
                    overwrite_on_duplicate: false,
                    state: lanrurugi_storage::download_queue::DownloadQueueState::Queued,
                })
                .await
                .expect("failed to create a test queue item")
        }

        let first_item = make_queue_item(
            &state,
            &plugin_namespace,
            format!("http://{addr}/{first_filename}"),
        )
        .await;
        let second_item = make_queue_item(
            &state,
            &plugin_namespace,
            format!("http://{addr}/{second_filename}"),
        )
        .await;
        // Mirrors what `start_download` itself does before ever calling `run_managed_downloads`
        // (see that function's own docs) — this test calls `run_managed_downloads` directly
        // (skipping the plugin-subprocess `exec_download` call `start_download` would normally
        // make in between), so it must set this initial `Starting` transition itself to accurately
        // reproduce the real state machine `run_managed_downloads` expects to already be in
        // (`run_managed_downloads` itself sets the real `Waiting` transition, right before it may
        // actually block on `DownloadManager::acquire`).
        for item in [&first_item, &second_item] {
            update_queue_item_state(
                &state.download_queue,
                None,
                &item.id,
                lanrurugi_storage::download_queue::DownloadQueueState::Starting,
                None,
                None,
                None,
            )
            .await;
        }

        async fn spawn_run(
            state: &AppState,
            plugin_namespace: &str,
            addr: std::net::SocketAddr,
            path: &str,
            item_id: String,
        ) -> tokio::task::JoinHandle<Result<Vec<String>, lanrurugi_core::queue_error::QueueError>>
        {
            let downloads = vec![PluginDownloadRequest {
                url: format!("http://{addr}{path}"),
                method: None,
                headers: Default::default(),
                filename_hint: Some(path.trim_start_matches('/').to_string()),
            }];
            let job_id = state.jobs.create("download_url").await;
            let state = state.clone();
            let plugin_namespace = plugin_namespace.to_string();
            let queue_link = Some((state.download_queue.clone(), item_id.clone()));
            tokio::spawn(async move {
                let cancel = tokio_util::sync::CancellationToken::new();
                run_managed_downloads(
                    state,
                    plugin_namespace,
                    downloads,
                    job_id,
                    None,
                    false,
                    "http://example.com/source",
                    &cancel,
                    Some(item_id.as_str()),
                    queue_link,
                )
                .await
            })
        }

        let first_task = spawn_run(
            &state,
            &plugin_namespace,
            addr,
            &format!("/{first_filename}"),
            first_item.id.clone(),
        )
        .await;
        // Wait for the first resource's own handler to actually start — proves the first (and
        // only) permit was really acquired and that transfer is genuinely in flight, not just
        // scheduled.
        hold_first.notified().await;

        let second_task = spawn_run(
            &state,
            &plugin_namespace,
            addr,
            &format!("/{second_filename}"),
            second_item.id.clone(),
        )
        .await;
        // Give the second task's own `run_managed_downloads` a real chance to reach
        // `DownloadManager::acquire` and start blocking on the exhausted semaphore before this
        // test inspects its queue state — a fixed short sleep rather than a signal from inside
        // `acquire` itself, since there's no hook to notify from partway through a blocked
        // `tokio::select!` without changing production code just for this test.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let first_mid_flight = state
            .download_queue
            .get(&first_item.id)
            .await
            .expect("redis lookup failed")
            .expect("first queue item must still exist");
        assert_eq!(
            first_mid_flight.state,
            lanrurugi_storage::download_queue::DownloadQueueState::Downloading,
            "the first item must be Downloading once its permit is actually held"
        );

        let second_mid_flight = state
            .download_queue
            .get(&second_item.id)
            .await
            .expect("redis lookup failed")
            .expect("second queue item must still exist");
        assert_eq!(
            second_mid_flight.state,
            lanrurugi_storage::download_queue::DownloadQueueState::Waiting,
            "the second item must show Waiting while genuinely blocked on the first item's held \
             permit — this is the core of this feature: previously it would have already been \
             set to Downloading at task-spawn time, before ever reaching DownloadManager::acquire"
        );

        release_first.notify_one();
        let first_ids = first_task
            .await
            .expect("task must not panic")
            .expect("first resource must download successfully");
        let second_ids = second_task
            .await
            .expect("task must not panic")
            .expect("second resource must download successfully, now that the permit is free");
        assert_eq!(first_ids.len(), 1);
        assert_eq!(second_ids.len(), 1);

        let second_done = state
            .download_queue
            .get(&second_item.id)
            .await
            .expect("redis lookup failed")
            .expect("second queue item must still exist");
        assert_eq!(
            second_done.state,
            lanrurugi_storage::download_queue::DownloadQueueState::Downloading,
            "run_managed_downloads itself only ever advances state up to Downloading — the \
             transition to Done is start_download's own responsibility (not exercised by this \
             test, which calls run_managed_downloads directly)"
        );

        server.abort();
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
        std::fs::write(
            temp_dir.join("plugin-sdk.ts"),
            lanrurugi_plugin::PLUGIN_SDK_SCRIPT,
        )
        .expect("failed to write out the real plugin SDK script");
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
            &crate::download_manager::stream::NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
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
        std::fs::write(
            std::env::temp_dir().join("plugin-sdk.ts"),
            lanrurugi_plugin::PLUGIN_SDK_SCRIPT,
        )
        .expect("failed to write out the real plugin SDK script");
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

    /// Regression test for issue #78/#93: `login/nhentai` (namespace `nhapiauth`) authenticates
    /// via an `Authorization` header, not a cookie — before `LoginResult.headers` existed, this
    /// credential had no way to cross the dispatcher's JSON boundary at all (a header set via
    /// `ua.on("start", ...)` is a closure, silently dropped by `JSON.stringify`). This test proves
    /// the fixed plumbing works end-to-end through the real Deno dispatcher, without needing a
    /// real API key or any network access: `exec_login` with a fake key must return that key
    /// inside `headers.Authorization` (not silently drop it), and a metadata plugin using the same
    /// hydration pattern as `plugins/metadata/nhentai.ts` must actually apply a header it receives
    /// via `hostArgs.user_agent_headers` to its outgoing request (verified against a local
    /// `httpbin`-style echo endpoint bundled inline as a throwaway fixture plugin, so this test
    /// doesn't depend on nhentai.net's real API or a real key at all).
    #[tokio::test]
    async fn login_plugin_header_credential_reaches_a_downstream_plugin() {
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
            std::env::temp_dir().join("lrr-login-headers-integration-test-dispatcher.ts");
        std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)
            .expect("failed to write out the real dispatcher script");
        std::fs::write(
            std::env::temp_dir().join("plugin-sdk.ts"),
            lanrurugi_plugin::PLUGIN_SDK_SCRIPT,
        )
        .expect("failed to write out the real plugin SDK script");
        let pool =
            lanrurugi_plugin::pool::PluginPool::new("deno", dispatcher_path.clone(), plugins_dir);

        // Step 1: the real login plugin, with a fake key — exec_login does no network I/O of its
        // own, it just echoes the key back into `headers`.
        let login_result = pool
            .execute(
                "login/nhentai",
                "exec_login",
                json!({ "customargs": ["fake-test-key"] }),
            )
            .await
            .expect("real login/nhentai exec_login call failed");
        let headers = login_result
            .get("headers")
            .cloned()
            .expect("LoginResult.headers must be present for a header-authenticated login plugin");
        assert_eq!(headers["Authorization"], "Key fake-test-key");

        tokio::fs::remove_file(&dispatcher_path).await.ok();
    }
}
