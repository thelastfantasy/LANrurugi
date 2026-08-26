//! `POST /plugin-wizard/save` (FR-020/FR-021/FR-022, US6) — the terminal confirm-save action for
//! one type. Reuses `upload_plugin`'s own namespace-conflict-check + move-into-category +
//! rollback-on-failure logic verbatim via `plugins::move_into_category` (contracts/
//! plugin-wizard-api.md): unlike `upload_plugin`, there's no from-scratch staging/validation here
//! since the code already passed a real trial run — this endpoint just writes `code` straight to
//! a staging path under `custom/` and moves it into its declared category.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::plugins::{move_into_category, CUSTOM_PLUGIN_DIR, PLUGIN_CATEGORIES};
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct SaveRequest {
    plugin_type: String,
    code: String,
    /// User-chosen, human-meaningful file stem (no `.ts`, no path separators) — e.g. `"foosite"`
    /// for a plugin that will land at `plugins/custom/metadata/foosite.ts`. Required: without
    /// this, the file would need a machine-generated name (a UUID), which defeats FR-021's
    /// namespace-conflict check (nothing would ever plausibly collide with a random UUID) and
    /// gives the user nothing recognizable to manage the plugin by afterward.
    filename: String,
    /// Whatever real values the user already typed into the wizard's own trial-run parameter
    /// inputs (`TrialRunRequest.plugin_parameter_values` — e.g. a generated download plugin's own
    /// `api_key`), persisted straight into the newly-installed plugin's real `LRR_PLUGIN_<NS>`
    /// settings the instant save succeeds — so confirming "确认并安装" after a working trial run
    /// means the plugin is immediately ready to use for real, not merely installed-but-unconfigured
    /// pending a second manual trip to its settings page to re-enter the same values (a real,
    /// reported friction point, 2026-08-25).
    #[serde(default)]
    plugin_parameter_values: std::collections::HashMap<String, String>,
    /// Set when this save is meant to overwrite a plugin the wizard session itself already
    /// loaded (`editExistingType` — a domain lookup that found a previously wizard-generated
    /// `custom/` plugin, put into edit mode). Must travel paired with `overwrite_namespace`,
    /// never alone: a bare boolean here would let any request overwrite an arbitrary already-
    /// installed custom plugin just by naming it in `filename`, regardless of whether the
    /// session ever actually loaded that file. See the validation in `save()` below.
    #[serde(default)]
    allow_overwrite: bool,
    /// The exact file-path namespace (e.g. `custom/metadata/foosite`) this save is allowed to
    /// overwrite — the frontend sends `TypeSession.editingExistingNamespace` verbatim, the same
    /// namespace `plugin_wizard::lookup`'s own `TypeCoverage.namespace` reported when this
    /// session's draft was first loaded. `save()` requires this to exactly equal the namespace
    /// this request's own `plugin_type`/`filename` computes to before allowing the overwrite —
    /// never trusted as "just believe the client, overwrite whatever's at `filename`".
    #[serde(default)]
    overwrite_namespace: Option<String>,
}

#[derive(Serialize)]
struct SaveResponse {
    /// The file-path-derived namespace (`custom/<type>/<filename>`) — what every other endpoint
    /// (`/plugins/use`, `/plugins/options`, `/plugins/settings`) actually addresses this plugin
    /// by, and what the wizard UI displays ("Installed as ...").
    namespace: String,
    /// The plugin's own self-declared `pluginInfo().namespace` value — what a *different*
    /// plugin's `login_from` field must reference to resolve back to this one
    /// (`resolve_declared_namespace` matches on this field, not the file-path namespace above;
    /// every real installed plugin already follows this convention, e.g. `ehentai.ts`'s
    /// `login_from: "ehlogin"` matching `login/ehentai.ts`'s own `namespace: "ehlogin"`). The
    /// wizard's own `login_from` association (FR-025/T042/T043) must use this value, not
    /// `namespace` above, or `with_login_cookies`'s lookup silently finds nothing.
    declared_namespace: String,
}

pub(super) async fn save(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Json(req): Json<SaveRequest>,
) -> Response {
    if !PLUGIN_CATEGORIES.contains(&req.plugin_type.as_str()) {
        return error(
            StatusCode::BAD_REQUEST,
            format!("Unknown plugin_type {:?}.", req.plugin_type),
        );
    }
    if !is_safe_filename(&req.filename) {
        return error_typed(
            StatusCode::BAD_REQUEST,
            "invalid_filename",
            "filename must be a plain name with no path separators or \".ts\" extension.",
        );
    }

    let staging_dir = state.plugins_dir.join(CUSTOM_PLUGIN_DIR);
    if let Err(e) = tokio::fs::create_dir_all(&staging_dir).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Staged under a throwaway UUID name first (mirrors `upload_plugin`'s own staging step) so
    // the probe below can run before we commit to the user's requested final filename — the
    // conflict check that matters (FR-021) happens in `move_into_category` using `req.filename`,
    // not this staging name.
    let staging_file_name = format!("{}.ts", uuid::Uuid::new_v4());
    let staged_path = staging_dir.join(&staging_file_name);
    if let Err(e) = tokio::fs::write(&staged_path, &req.code).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Re-validate with a real plugin_info() probe (staged code came straight from a possibly
    // manually-edited draft, not necessarily the same bytes a trial run last verified) before
    // trusting its own declared `type` for the move — same authority principle as `upload_plugin`.
    let staged_namespace = format!(
        "{CUSTOM_PLUGIN_DIR}/{}",
        staging_file_name.trim_end_matches(".ts")
    );
    let info = match state.plugins.plugin_info(&staged_namespace).await {
        Ok(info) => info,
        Err(e) => {
            let _ = tokio::fs::remove_file(&staged_path).await;
            let detail = format!(
                "Could not load this plugin — it might not implement the expected protocol. {e}"
            );
            record_save_failure(&state, auth.as_ref(), &req, None, &detail).await;
            return error_typed(StatusCode::BAD_REQUEST, "invalid_plugin_code", detail);
        }
    };
    if info.kind != req.plugin_type {
        let _ = tokio::fs::remove_file(&staged_path).await;
        let detail = format!(
            "The code's own declared type ({:?}) does not match the requested plugin_type ({:?}).",
            info.kind, req.plugin_type
        );
        record_save_failure(&state, auth.as_ref(), &req, None, &detail).await;
        return error(StatusCode::BAD_REQUEST, detail);
    }

    let final_file_name = format!("{}.ts", req.filename);
    let final_namespace_guessed = final_namespace_guess(&info.kind, &req.filename);

    // FR-XXX (wizard edit mode): `allow_overwrite` only takes effect when the caller also names
    // the exact namespace it believes it's overwriting, and that name matches what this request's
    // own `plugin_type`/`filename` actually computes to — a mismatch means either a stale/forged
    // request or a genuine attempt to overwrite an unrelated file, neither of which should ever
    // touch the filesystem. Checked before `move_into_category` runs, not after.
    if req.allow_overwrite {
        match &req.overwrite_namespace {
            Some(ns) if *ns == final_namespace_guessed => {}
            _ => {
                let _ = tokio::fs::remove_file(&staged_path).await;
                let detail = format!(
                    "overwrite_namespace must equal the namespace this save resolves to ({final_namespace_guessed:?})."
                );
                record_save_failure(
                    &state,
                    auth.as_ref(),
                    &req,
                    Some(&final_namespace_guessed),
                    &detail,
                )
                .await;
                return error_typed(
                    StatusCode::BAD_REQUEST,
                    "overwrite_namespace_mismatch",
                    detail,
                );
            }
        }
    }

    let final_namespace = match move_into_category(
        &staging_dir,
        &staged_path,
        &final_file_name,
        &info.kind,
        "plugin_wizard_save",
        req.allow_overwrite,
    )
    .await
    {
        Ok(ns) => ns,
        Err(resp) => {
            let detail = if resp.status() == StatusCode::CONFLICT {
                format!("A plugin already exists at {final_namespace_guessed:?}.")
            } else {
                "Failed to install the plugin file.".to_string()
            };
            record_save_failure(
                &state,
                auth.as_ref(),
                &req,
                Some(&final_namespace_guessed),
                &detail,
            )
            .await;
            return conflict_response(resp, &final_namespace_guessed);
        }
    };

    if !req.plugin_parameter_values.is_empty() {
        // The wizard's own trial-run parameter form (`TrialRunResult.tsx`'s `pluginParameterValues`)
        // still collects every value as plain text, `bool` params included (a real settings-page
        // switch isn't rendered there — it's a lighter-weight trial-run input, not the full
        // `PluginParametersForm`) — `"true"`/non-empty is treated as checked, matching what a
        // generated plugin's own trial-run checkbox input actually submits.
        let customargs: Vec<Value> = info
            .parameters
            .iter()
            .map(|p| {
                let raw = req
                    .plugin_parameter_values
                    .get(&p.name)
                    .cloned()
                    .unwrap_or_default();
                match p.param_type.as_deref() {
                    Some("bool") => Value::Bool(raw == "true" || raw == "1"),
                    Some("int") => Value::Number(raw.parse::<i64>().unwrap_or(0).into()),
                    _ => Value::String(raw),
                }
            })
            .collect();
        if let Err(e) =
            crate::plugins::set_plugin_customargs(&state, &final_namespace, &customargs).await
        {
            // Not fatal — the plugin file itself is already correctly installed at this point;
            // losing the parameter pre-fill just means the user falls back to filling them in via
            // the real settings page once, same as before this feature existed.
            tracing::warn!(
                namespace = %final_namespace,
                error = %e,
                "plugin wizard: save succeeded but failed to persist trial-run parameter values",
            );
        }
    }

    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::PLUGIN_WIZARD_SAVE,
        lanrurugi_storage::activity::ActivityTarget {
            id: Some(final_namespace.clone()),
            label: Some(info.name.clone()),
            kind: Some("plugin".to_string()),
        },
        lanrurugi_storage::activity::Outcome::Success,
        None,
        Some(json!({ "type": info.kind, "overwrite": req.allow_overwrite })),
    )
    .await;

    Json(SaveResponse {
        namespace: final_namespace,
        declared_namespace: info.namespace,
    })
    .into_response()
}

/// Records a failed save attempt — real user feedback, 2026-08-26: asked whether AI plugin wizard
/// create/edit/save actions leave any activity trail for *failures*, not just successes (the
/// success path already did before this existed). Only called from the handful of `save()` exit
/// points past the point where the user's own "确认并保存/确认覆盖" click genuinely attempted a
/// real install — the earlier plain-request-shape rejections (unknown `plugin_type`, unsafe
/// filename, staging I/O) stay unrecorded, matching `upload_plugin`'s own established convention
/// of not logging pre-execution format rejections, only real attempted-and-failed installs.
/// `namespace` is `None` for the two failures that happen before a namespace guess is even
/// possible to compute meaningfully (before `info.kind` is known).
async fn record_save_failure(
    state: &AppState,
    auth: Option<&axum::extract::Extension<crate::auth_context::AuthContext>>,
    req: &SaveRequest,
    namespace: Option<&str>,
    reason: &str,
) {
    crate::activity::record_manual(
        state,
        auth.map(|e| &e.0),
        lanrurugi_storage::activity::action_types::PLUGIN_WIZARD_SAVE,
        lanrurugi_storage::activity::ActivityTarget {
            id: namespace.map(str::to_string),
            label: Some(req.filename.clone()),
            kind: Some("plugin".to_string()),
        },
        lanrurugi_storage::activity::Outcome::Failure {
            reason: reason.to_string(),
        },
        None,
        Some(json!({ "type": req.plugin_type, "overwrite": req.allow_overwrite })),
    )
    .await;
}

/// A plain file stem: no path separators, no `.` (which would allow smuggling an extension or a
/// `..` traversal component once `.ts` is appended), non-empty.
fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn final_namespace_guess(kind: &str, filename: &str) -> String {
    format!("{CUSTOM_PLUGIN_DIR}/{kind}/{filename}")
}

/// `move_into_category` returns `crate::common::error`'s generic `{operation, error, success}`
/// shape (shared with `upload_plugin`) — a real `409` here specifically means FR-021's
/// namespace conflict, so rewrap it into the shape `contracts/plugin-wizard-api.md` documents
/// (`{"error": "namespace_conflict", "namespace": ...}`) rather than leaking the shared helper's
/// generic message (which, since the staging step uses a throwaway UUID name, would otherwise
/// describe the wrong file).
fn conflict_response(resp: Response, namespace: &str) -> Response {
    if resp.status() == StatusCode::CONFLICT {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "namespace_conflict", "namespace": namespace })),
        )
            .into_response();
    }
    resp
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
