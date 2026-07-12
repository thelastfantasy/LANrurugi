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
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::error;
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

/// Redis key a plugin's own configured custom value lives under — legacy's `LRR_PLUGIN_<NS>`
/// hash (`~/LANraragi/lib/LANraragi/Utils/Plugins.pm:106-167`), on the same `config` logical DB
/// legacy itself uses (`Model/Config.pm::get_redis_config`) so a value configured through a
/// legacy instance sharing this Redis is read correctly here with zero migration (Principle I).
/// Only one `arg` field is stored — matching the host's own current single-generic-value calling
/// convention (see `use_plugin_sync`'s `args` below); a plugin declaring more than one custom
/// parameter can't be fully represented by this yet, same limitation `lanrurugi-plugin-converter`
/// already surfaces as a conversion warning.
fn plugin_settings_key(namespace: &str) -> String {
    format!("LRR_PLUGIN_{}", namespace.to_uppercase())
}

async fn get_plugin_arg(state: &AppState, namespace: &str) -> String {
    let Ok(mut conn) = state.redis.config.get().await else {
        return String::new();
    };
    let value: Option<String> = conn
        .hget(plugin_settings_key(namespace), "arg")
        .await
        .unwrap_or_default();
    value.unwrap_or_default()
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
    let Some(login_ns) = &info.login_from else {
        return args;
    };
    let login_arg = get_plugin_arg(state, login_ns).await;
    let login_args = json!({ "arg": login_arg });
    match state
        .plugins
        .execute(login_ns, "exec_login", login_args)
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
        .route("/download_url", post(download_url))
        .route("/plugins/upload", post(upload_plugin))
}

#[derive(Debug, Deserialize)]
pub struct PluginSettingsQuery {
    namespace: String,
}

async fn get_plugin_settings(
    State(state): State<AppState>,
    Query(query): Query<PluginSettingsQuery>,
) -> Response {
    let arg = get_plugin_arg(&state, &query.namespace).await;
    axum::Json(json!({ "arg": arg })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct PutPluginSettingsBody {
    #[serde(default)]
    arg: String,
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
    let result: Result<(), _> = conn
        .hset(plugin_settings_key(&query.namespace), "arg", &body.arg)
        .await;
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
    let mut plugins = Vec::new();
    for ns in namespaces {
        if let Ok(info) = state.plugins.plugin_info(&ns).await {
            if kind == "all" || info.kind == kind {
                plugins.push(json!({
                    "namespace": info.namespace,
                    "type": info.kind,
                    "name": info.name,
                    "author": info.author,
                    "description": info.description,
                    "version": info.version,
                    "icon": info.icon,
                    "oneshot_arg": info.oneshot_arg,
                    "parameters": info.parameters.iter().map(|p| json!({
                        "name": p.name,
                        "desc": p.description,
                    })).collect::<Vec<_>>(),
                }));
            }
        }
    }
    axum::Json(plugins).into_response()
}

#[derive(Debug, Deserialize)]
pub struct UsePluginParams {
    id: Option<String>,
    plugin: Option<String>,
    arg: Option<String>,
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
    // already knows.
    let file_path = match &params.id {
        Some(id) => state
            .repos
            .archives
            .get(id)
            .await
            .ok()
            .flatten()
            .map(|a| a.file),
        None => None,
    };

    let args = json!({
        "archive_id": params.id,
        "arg": params.arg,
        "file_path": file_path,
    });
    let args = with_sidecar_files(&info, args, file_path.as_deref());
    let args = with_login_cookies(&state, &info, args).await;
    let method = plugin_method(&info.kind);

    match state.plugins.execute(&plugin, method, args).await {
        Ok(data) => axum::Json(json!({
            "operation": "use_plugin",
            "success": 1,
            "type": info.kind,
            "data": data,
        }))
        .into_response(),
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
            Some(id) => archives.get(id).await.ok().flatten().map(|a| a.file),
            None => None,
        };
        let method = plugin_method(&info.kind);
        let args = json!({ "archive_id": archive_id, "arg": arg, "file_path": file_path });
        let args = with_sidecar_files(&info, args, file_path.as_deref());
        let args = with_login_cookies(&state_for_task, &info, args).await;
        match plugins.execute(&plugin, method, args).await {
            Ok(data) => jobs.finish(&job_id_for_task, data).await,
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

    let job_id = state.jobs.create("download_url").await;
    let jobs = state.jobs.clone();
    let plugins = state.plugins.clone();
    let job_id_for_task = job_id.clone();
    let category = params.catid.clone();
    let state_for_task = state.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let args = json!({ "arg": url, "category": category });
        let args = with_login_cookies(&state_for_task, &info, args).await;
        match plugins.execute(&plugin, "exec_download", args).await {
            Ok(data) => jobs.finish(&job_id_for_task, data).await,
            Err(e) => jobs.fail(&job_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({
        "operation": "download_url",
        "url": params.url,
        "category": params.catid,
        "success": 1,
        "job": job_id,
    }))
    .into_response()
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
