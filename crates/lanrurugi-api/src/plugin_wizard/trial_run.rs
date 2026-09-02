//! `POST /plugin-wizard/trial-run` (FR-014/FR-015/FR-016, US3) — stages a draft's code to a
//! throwaway namespace, probes it with `plugin_info()`, calls `execute()` for real against the
//! user-supplied links/credentials, and *always* deletes the staged file before returning
//! (T025) — nothing here is ever promoted; only `/save` does that.
//!
//! If the draft declares `login_from` (FR-013's up-front path, or FR-025's after-the-fact
//! association once the login plugin has been confirm-saved — see spec.md's amended FR-025), a
//! login call runs first and its cookies *and/or headers* (issue #78/#93 — a login plugin may
//! authenticate via either, and a trial run that only forwarded cookies silently dropped every
//! header/token-based credential, exactly the bug that motivated adding this field at all) are
//! injected into the metadata/download call, same as a real installed plugin's own
//! `with_login_cookies` (`plugins.rs`) — reused directly here for the no-explicit-credentials
//! path. Two
//! credential sources are supported: `login_credentials` in this same request (the wizard's own
//! fresher case — a login `TypeSession`'s test credentials, never persisted to Redis, so
//! `with_login_cookies`'s own Redis-settings lookup would find nothing for a plugin saved only
//! moments ago), falling back to `with_login_cookies`'s normal Redis-persisted-settings path when
//! no explicit credentials are supplied (e.g. `login_from` pointing at a pre-existing plugin the
//! user never re-entered credentials for in this session, FR-025's "associate with the existing
//! one" branch). If `login_from` points at a namespace that isn't a real, saved plugin (still just
//! a throwaway trial-run staging namespace from earlier this session), neither path finds
//! anything and the call proceeds unauthenticated, same as `with_login_cookies`'s own
//! failure-only-warns behavior.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::download_manager::settings::{merge, EffectivePluginOptions};
use crate::plugins::{with_login_cookies, CUSTOM_PLUGIN_DIR};
use crate::AppState;

/// Subdirectory of `custom/` reserved for trial-run staging — distinct from `upload_plugin`'s own
/// direct use of `custom/` so a crashed/killed request's leftover file (if T025's cleanup somehow
/// didn't run) can never collide with or be mistaken for a real uploaded plugin.
const WIZARD_STAGING_DIR: &str = "_wizard";

#[derive(Deserialize)]
pub(super) struct TrialRunRequest {
    plugin_type: String,
    code: String,
    #[serde(default)]
    test_links: Vec<String>,
    #[serde(default)]
    credentials: Option<Credentials>,
    /// FR-012/FR-025: only ever present on a metadata/download trial run whose draft declares
    /// `login_from` pointing at a login plugin this *same wizard session* just saved — the
    /// frontend's own login `TypeSession`'s field values, never persisted server-side, used only
    /// to run one fresh login call for this one trial run (see module docs).
    #[serde(default)]
    login_credentials: Option<Credentials>,
    /// Metadata/download only — user-supplied values for the *draft's own* declared `pluginInfo().
    /// parameters` (e.g. a generated download plugin's own `api_key`/`format` options — distinct
    /// from `credentials`/`login_credentials`, which are for a separate login plugin). Previously
    /// this trial-run always sent a hardcoded empty `customargs: []`, so a draft whose own
    /// generated code required a parameter (observed live 2026-08-24: a real generated nhentai
    /// download plugin required its own `api_key` param) could never be trial-run successfully at
    /// all from the wizard, even after the user had a real value ready to supply — there was no
    /// input for it anywhere in the flow. Keyed by `PluginParameter.name`, same convention
    /// `Credentials.fields`/`customargs_for` already use.
    #[serde(default)]
    plugin_parameter_values: std::collections::HashMap<String, String>,
}

/// The wizard no longer assumes every login is an account+secret pair (`/plugin-wizard/
/// analyze-login` may have determined the site actually needs a single token/API key, a raw
/// cookie value, or some other field set entirely) — `fields` is keyed by whatever field names
/// the analysis step (and the login plugin's own declared `pluginInfo().parameters`) settled on.
#[derive(Deserialize, Clone)]
struct Credentials {
    fields: std::collections::HashMap<String, String>,
}

/// Converts a `name -> value` field map into the positional `customargs` array the plugin's own
/// entry function actually reads — ordered to match `info.parameters` exactly (the same convention
/// every real installed plugin's `customargs` already follows), not insertion/HashMap order, which
/// is unspecified. Used both for login credentials (`Credentials.fields`) and, since T-new, a
/// metadata/download draft's own declared parameter values (`TrialRunRequest.
/// plugin_parameter_values`) — same shape, same positional-mapping rule either way.
fn customargs_for(
    fields: &std::collections::HashMap<String, String>,
    info: &lanrurugi_plugin::protocol::PluginInfo,
) -> Vec<String> {
    info.parameters
        .iter()
        .map(|p| fields.get(&p.name).cloned().unwrap_or_default())
        .collect()
}

/// [`PluginParameter`](lanrurugi_plugin::protocol::PluginParameter) reshaped for the trial-run
/// response — tells the frontend which input fields to render for a draft's own declared
/// parameters (see `TrialRunRequest.plugin_parameter_values`'s own docs).
#[derive(Serialize)]
struct DeclaredParameter {
    name: String,
    description: String,
    required: bool,
}

#[derive(Serialize)]
struct PerLinkResult {
    link: String,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(super) async fn trial_run(
    State(state): State<AppState>,
    Json(req): Json<TrialRunRequest>,
) -> Response {
    let staging_dir = state
        .plugins_dir
        .join(CUSTOM_PLUGIN_DIR)
        .join(WIZARD_STAGING_DIR);
    if let Err(e) = tokio::fs::create_dir_all(&staging_dir).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    let file_name = format!("{}.ts", uuid::Uuid::new_v4());
    let staged_path = staging_dir.join(&file_name);
    let namespace = format!(
        "{CUSTOM_PLUGIN_DIR}/{WIZARD_STAGING_DIR}/{}",
        file_name.trim_end_matches(".ts")
    );

    if let Err(e) = tokio::fs::write(&staged_path, &req.code).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    let result = run_trial(&state, &namespace, &req).await;

    // T025: unconditional cleanup — runs regardless of which branch `run_trial` returned from.
    let _ = tokio::fs::remove_file(&staged_path).await;

    result
}

async fn run_trial(state: &AppState, namespace: &str, req: &TrialRunRequest) -> Response {
    // T022/T026: zero-permission probe first, exactly like `upload_plugin` — catches
    // syntactically invalid or protocol-non-conformant drafts before attempting a real call. Its
    // result is also what `run_link_trial` needs to know whether this draft declares `login_from`.
    let info = match state.plugins.plugin_info(namespace).await {
        Ok(info) => info,
        Err(e) => {
            return error_typed(
                StatusCode::BAD_REQUEST,
                "invalid_plugin_code",
                format!("pluginInfo() threw: {e}"),
            )
        }
    };

    match req.plugin_type.as_str() {
        "login" => run_login_trial(state, namespace, &info, req).await,
        "metadata" | "download" => run_link_trial(state, namespace, &info, req).await,
        other => error(
            StatusCode::BAD_REQUEST,
            format!("Unknown plugin_type {other:?}."),
        ),
    }
}

/// Probes the *staged draft itself* for a `pluginOptions()` export — same zero-permission call
/// `GET /plugins/options` makes for an installed plugin (`plugins.rs::fetch_declared_options`),
/// just pointed at the trial-run's own throwaway namespace instead. A draft never has a persisted
/// `PluginOptionsOverride` (it isn't saved yet), so `merge` always sees `None` there — the
/// response is always 100% plugin-declared defaults, never a user override. Lets the frontend
/// render the same domain-rate-limit/bundle-as-archive settings panel for a not-yet-saved draft
/// as it already does for an installed plugin, driven by whatever the draft's own code actually
/// exports right now rather than an assumption about what a "download plugin" should have.
async fn probe_declared_options(
    state: &AppState,
    namespace: &str,
) -> Option<EffectivePluginOptions> {
    let declared = state
        .plugins
        .plugin_options(namespace)
        .await
        .ok()
        .flatten()?;
    Some(merge(namespace, &declared, None))
}

async fn run_login_trial(
    state: &AppState,
    namespace: &str,
    info: &lanrurugi_plugin::protocol::PluginInfo,
    req: &TrialRunRequest,
) -> Response {
    let Some(credentials) = &req.credentials else {
        return error(
            StatusCode::BAD_REQUEST,
            "Missing `credentials` for a login trial run.",
        );
    };
    // FR-012: credentials go straight into this one `execute()` call's args and nowhere else —
    // never logged, never echoed back beyond whatever the plugin's own result legitimately
    // contains (T024).
    let args = json!({ "customargs": customargs_for(&credentials.fields, info) });

    match state.plugins.execute(namespace, "exec_login", args).await {
        Ok(result) => {
            // A login plugin may authenticate via a cookie, a header/token, or both (issue
            // #78/#93) — neither field alone tells the whole story, so this message must check
            // both rather than assuming `cookies` is the only credential shape that counts as
            // success.
            let has_cookies = result.get("cookies").is_some_and(|v| !v.is_null());
            let has_headers = result.get("headers").is_some_and(|v| !v.is_null());
            let detail = match (has_cookies, has_headers) {
                (true, true) => "Login succeeded, cookies and headers obtained.".to_string(),
                (true, false) => "Login succeeded, cookies obtained.".to_string(),
                (false, true) => "Login succeeded, headers obtained.".to_string(),
                (false, false) => {
                    "Login call completed but returned no cookies or headers.".to_string()
                }
            };
            Json(json!({ "outcome": "success", "detail": detail })).into_response()
        }
        Err(e) => Json(json!({ "outcome": "failure", "detail": e.to_string() })).into_response(),
    }
}

async fn run_link_trial(
    state: &AppState,
    namespace: &str,
    info: &lanrurugi_plugin::protocol::PluginInfo,
    req: &TrialRunRequest,
) -> Response {
    let method = if req.plugin_type == "metadata" {
        "exec_metadata"
    } else {
        "exec_download"
    };

    // See module docs: explicit `login_credentials` (this session's own fresh login draft) takes
    // priority over `with_login_cookies`'s normal Redis-persisted-settings lookup.
    let (login_cookies, login_headers) =
        resolve_login_credentials(state, info, req.login_credentials.as_ref()).await;
    // The draft's *own* declared parameters (e.g. a generated download plugin's own `api_key`) —
    // previously always sent as an empty `customargs: []` regardless of what the draft actually
    // declared (see `TrialRunRequest.plugin_parameter_values`'s own docs for the real failure this
    // caused).
    let plugin_customargs = customargs_for(&req.plugin_parameter_values, info);

    let mut per_link = Vec::with_capacity(req.test_links.len());
    for link in &req.test_links {
        let mut args = if method == "exec_metadata" {
            json!({
                "url": link,
                "arg": link,
                "customargs": plugin_customargs.clone(),
                "existing_tags": "",
                "archive_title": "",
                "thumbnail_hash": "",
            })
        } else {
            json!({ "url": link, "category": "", "customargs": plugin_customargs.clone() })
        };
        if let Some(cookies) = &login_cookies {
            args["user_agent_cookies"] = cookies.clone();
        }
        if let Some(headers) = &login_headers {
            args["user_agent_headers"] = headers.clone();
        }

        match state.plugins.execute(namespace, method, args).await {
            Ok(data) => {
                // SDK's MetadataResult/DownloadResult both define `error?: PluginError` as the
                // documented way for a plugin to report an *expected* semantic failure (e.g. "this
                // URL isn't a gallery page") without throwing — a plugin following that convention
                // never surfaces as a Rust-level `Err` here, so a bare `Ok(data)` alone doesn't
                // mean the plugin actually extracted anything usable. Observed live (2026-08-24):
                // an API-docs URL correctly triggered a generated download plugin's own `{ error:
                // {...} }` return, but the trial run still reported it as "成功" until this check
                // was added.
                if let Some(err) = data.get("error").filter(|v| !v.is_null()) {
                    let message = err
                        .get("error_code")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| err.to_string());
                    per_link.push(PerLinkResult {
                        link: link.clone(),
                        outcome: "failure",
                        data: None,
                        error: Some(message),
                    });
                } else {
                    per_link.push(PerLinkResult {
                        link: link.clone(),
                        outcome: "success",
                        data: Some(data),
                        error: None,
                    });
                }
            }
            Err(e) => per_link.push(PerLinkResult {
                link: link.clone(),
                outcome: "failure",
                data: None,
                error: Some(e.to_string()),
            }),
        }
    }

    let login_suggestion = if per_link.iter().any(|r| r.outcome == "failure") {
        Some(classify_login_relevance(state, &per_link).await)
    } else {
        None
    };
    // Lets the frontend render an input per declared parameter (see `TrialRunRequest.plugin_
    // parameter_values`'s own docs) — the wizard had no other way to learn what a draft's own code
    // actually declared here, since `pluginInfo()`'s return value was probed server-side
    // (`run_trial`, above) but never surfaced back to the caller until now.
    let declared_parameters: Vec<DeclaredParameter> = info
        .parameters
        .iter()
        .map(|p| DeclaredParameter {
            name: p.name.clone(),
            description: p.description.clone(),
            required: p.required,
        })
        .collect();
    // Only a download plugin can declare pluginOptions() at all (SDK docs) — skip the extra
    // Deno round-trip entirely for a metadata draft, which can never have one.
    let declared_options = if req.plugin_type == "download" {
        probe_declared_options(state, namespace).await
    } else {
        None
    };

    Json(json!({
        "per_link": per_link,
        "login_suggestion": login_suggestion,
        "declared_parameters": declared_parameters,
        "declared_options": declared_options,
    }))
    .into_response()
}

/// See module docs. Returns the `(cookies, headers)` values to inject into
/// `args["user_agent_cookies"]`/`args["user_agent_headers"]` — `(None, None)` if this draft has
/// no `login_from`, the login call failed, or it returned neither. Mirrors
/// `lanrurugi-api::plugins::with_login_cookies`'s own two-field handling (issue #78/#93: a login
/// plugin authenticating via a header/token rather than a cookie — e.g. `nhapiauth` — has nothing
/// in `cookies` at all, so reading only that field silently drops its credential here exactly the
/// same way the production path used to).
async fn resolve_login_credentials(
    state: &AppState,
    info: &lanrurugi_plugin::protocol::PluginInfo,
    explicit_credentials: Option<&Credentials>,
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let Some(login_from) = info.login_from.as_ref() else {
        return (None, None);
    };
    let Some(credentials) = explicit_credentials else {
        // No fresh credentials supplied — fall back to the normal installed-plugin path (Redis-
        // persisted settings for whatever `login_from` resolves to, if anything).
        let with_cookies = with_login_cookies(state, info, json!({})).await;
        return (
            with_cookies.get("user_agent_cookies").cloned(),
            with_cookies.get("user_agent_headers").cloned(),
        );
    };

    let Some(login_ns) = resolve_login_namespace(state, login_from).await else {
        return (None, None);
    };
    let Ok(login_info) = state.plugins.plugin_info(&login_ns).await else {
        return (None, None);
    };
    let args = json!({ "customargs": customargs_for(&credentials.fields, &login_info) });
    match state.plugins.execute(&login_ns, "exec_login", args).await {
        Ok(result) => (
            result.get("cookies").cloned(),
            result.get("headers").cloned(),
        ),
        Err(e) => {
            tracing::warn!(
                login_plugin = %login_ns,
                error = %e,
                "wizard trial-run's own login call failed, continuing without a logged-in user agent"
            );
            (None, None)
        }
    }
}

/// Finds the installed plugin whose own `pluginInfo().namespace` matches `declared_namespace` —
/// the file-path namespace to actually `execute()` against, same matching rule
/// `plugins.rs::resolve_declared_namespace` uses internally (not reused directly since it's
/// private to that module; this is a minimal, read-only re-implementation of the same lookup).
async fn resolve_login_namespace(state: &AppState, declared_namespace: &str) -> Option<String> {
    for ns in crate::plugins::discover_namespaces(&state.plugins_dir).await {
        if let Ok(info) = state.plugins.plugin_info(&ns).await {
            if info.namespace == declared_namespace {
                return Some(ns);
            }
        }
    }
    None
}

/// US7/FR-025 (T040) — a focused, tool-less `tool_chat()` classification call over the failed
/// links' own error text, never a local status-code/keyword heuristic (FR-010).
async fn classify_login_relevance(
    state: &AppState,
    per_link: &[PerLinkResult],
) -> serde_json::Value {
    let failures: Vec<&str> = per_link
        .iter()
        .filter(|r| r.outcome == "failure")
        .filter_map(|r| r.error.as_deref())
        .collect();
    if failures.is_empty() {
        return json!({ "relevant": false, "reasoning": "No failures to classify." });
    }

    let system = "You judge whether a plugin trial-run failure indicates the target page \
        requires a login (access denied, redirected to a login/signin page, 401/403 status, a \
        paywall-like response) as opposed to an unrelated cause (404, network timeout, a plugin \
        logic bug). Respond with strictly this JSON shape: \
        {\"relevant\": boolean, \"reasoning\": string}.";
    let user = format!(
        "Trial-run error(s) from the failed link(s):\n\n{}",
        failures.join("\n---\n")
    );

    #[derive(Deserialize)]
    struct Classification {
        relevant: bool,
        reasoning: String,
    }

    match lanrurugi_llm::json_chat::<Classification>(&state.redis.config, system, &user, 0.3, 500)
        .await
    {
        Ok(c) => json!({ "relevant": c.relevant, "reasoning": c.reasoning }),
        Err(e) => json!({
            "relevant": false,
            "reasoning": format!("Could not classify (LLM unavailable): {e}"),
        }),
    }
}

fn error(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "error": detail.into() }))).into_response()
}

fn error_typed(status: StatusCode, kind: &str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": kind, "detail": detail.into() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the exact bug reported live (2026-08-25): a wizard trial run for a
    /// download/metadata draft with `login_from` pointing at a header-authenticated login plugin
    /// (`nhapiauth`) reported "Download requires a valid API key" even after `plugins.rs::
    /// with_login_cookies` was fixed to forward `headers` — because `resolve_login_credentials`
    /// (this module's *own*, separate credential-resolution path for the `login_credentials`-
    /// present case, used whenever the wizard is testing a login draft this same session, not yet
    /// persisted to Redis) only ever read `result.get("cookies")`, silently dropping `headers`
    /// exactly the way the now-fixed production path used to. Exercises `resolve_login_credentials`
    /// directly (not the full `/trial-run` HTTP route, which needs a real authenticated session)
    /// against the real, installed `login/nhentai` plugin through the real Deno dispatcher, with a
    /// fake key — proving the explicit-`login_credentials` branch now returns `headers` too.
    #[tokio::test]
    async fn trial_run_credential_resolution_forwards_header_auth_too() {
        let deno_on_path = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("deno").is_file())
        });
        if !deno_on_path {
            eprintln!("skipping: deno not found on PATH");
            return;
        }
        let Some(base_state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set / real Redis unreachable");
            return;
        };

        let plugins_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .canonicalize()
            .expect("repo's real plugins/ dir must exist");
        let dispatcher_path =
            std::env::temp_dir().join("lrr-trial-run-header-credential-test-dispatcher.ts");
        std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)
            .expect("failed to write out the real dispatcher script");
        std::fs::write(
            std::env::temp_dir().join("plugin-sdk.ts"),
            lanrurugi_plugin::PLUGIN_SDK_SCRIPT,
        )
        .expect("failed to write out the real plugin SDK script");
        let state = AppState {
            plugins: std::sync::Arc::new(lanrurugi_plugin::pool::PluginPool::new(
                "deno",
                dispatcher_path.clone(),
                plugins_dir.clone(),
            )),
            // Must be the real `plugins/` dir, not the dispatcher's own temp-file parent —
            // `resolve_login_namespace` below calls `discover_namespaces(&state.plugins_dir)`
            // to find which installed file declares `namespace: "nhapiauth"`, and scanning the
            // wrong directory silently finds nothing (this test previously always short-circuited
            // via the `deno`/Redis "skipping" early-returns above, so this bug went unnoticed
            // until CI actually ran it for the first time, 2026-08-26).
            plugins_dir,
            ..base_state
        };

        // A draft `metadata`/`download` plugin declaring `login_from: "nhapiauth"` — same
        // `PluginInfo` shape `run_link_trial` already has in hand from its own zero-permission
        // `plugin_info()` probe of the staged draft; this test skips the staging step and
        // constructs it directly since only `login_from` matters for credential resolution.
        let info = lanrurugi_plugin::protocol::PluginInfo {
            namespace: "test/draft".to_string(),
            kind: "download".to_string(),
            parameters: vec![],
            declared_permissions: lanrurugi_plugin::protocol::DeclaredPermissions {
                net: vec![],
                read: false,
                write: false,
            },
            login_from: Some("nhapiauth".to_string()),
            name: String::new(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            icon: None,
            oneshot_arg: None,
            url_pattern: None,
            domain_match: vec![],
            generated_by_wizard: false,
            sidecar_files: vec![],
        };
        let credentials = Credentials {
            fields: std::collections::HashMap::from([(
                "param1".to_string(),
                "fake-test-key".to_string(),
            )]),
        };

        let (cookies, headers) = resolve_login_credentials(&state, &info, Some(&credentials)).await;
        assert!(cookies.is_none(), "nhapiauth never sets a cookie");
        let headers = headers.expect("headers must be forwarded for a header-authenticated login");
        assert_eq!(headers["Authorization"], "Key fake-test-key");

        tokio::fs::remove_file(&dispatcher_path).await.ok();
    }
}
