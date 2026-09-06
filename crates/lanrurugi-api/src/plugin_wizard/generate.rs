//! `POST /plugin-wizard/generate/start` + `GET /plugin-wizard/generate/stream/{id}` (FR-008/
//! FR-009/FR-010) — runs the tool-calling agentic loop described in `spec.md` FR-010: build a
//! system/user prompt, call the LLM, and while the model keeps requesting `fetch_page`, execute it
//! locally and feed the result back, until the model returns final code (or the loop/overall
//! timeout is exceeded).
//!
//! Two-step, not a single `POST` returning JSON: real-time visibility into a slow generation
//! (confirmed live 2026-08-24 — the final code+explanation inference alone took 80+ seconds, with
//! the wizard UI showing nothing but a spinner the whole time and, on eventual timeout, losing
//! every round of progress that *had* happened) needs a streaming response, and `EventSource` (the
//! same SSE client every other real-time endpoint in this codebase already uses —
//! `download_queue.rs::compare_queue_item_stream`/`queue_stream`) is GET-only with no request
//! body. `/start` accepts the actual (potentially large — full conversation history, link lists)
//! request body, stashes it in `AppState::pending_generate_requests` keyed by a fresh id, and
//! returns just that id; `/stream/{id}` is the real `EventSource`-compatible GET endpoint, which
//! looks up (removing — single-use) the stashed body and runs the actual generation, emitting
//! `fetch_page`/`fetch_result` events per tool-calling round and `content_delta` events as the
//! final answer streams in token-by-token, terminated by exactly one `done` or `error` event —
//! never relying on connection-close/timeout as an implicit end signal, same convention
//! `compare_queue_item_stream`'s own docs establish. Because progress is pushed as it happens
//! rather than assembled into one final response, an eventual `GENERATE_TIMEOUT` expiry no longer
//! erases everything that already streamed — the frontend has already seen it.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use lanrurugi_llm::{LlmClient, Message, ToolChatResponse};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use super::tool_loop::{
    execute_fetch_tool, extract_headers_arg, extract_url_arg, fetch_page_tool,
    substitute_credential_placeholders,
};
use crate::AppState;

const PLUGIN_SDK: &str = include_str!("../../../lanrurugi-plugin/dispatcher/plugin-sdk.ts");
const SAMPLE_METADATA: &str =
    include_str!("../../../lanrurugi-plugin/samples/sample-metadata-plugin.ts");
const SAMPLE_DOWNLOAD: &str =
    include_str!("../../../lanrurugi-plugin/samples/sample-download-plugin.ts");

/// A safety cap on tool-calling rounds within one generation, independent of the overall
/// `GENERATE_TIMEOUT` below — a model that keeps calling `fetch_page` without ever converging
/// should still terminate deterministically rather than relying solely on the wall-clock timeout.
/// Lowered from 8 (2026-08-24): a real generation still timed out at 120s after 7 rounds of
/// fetch_page exploration (docs, two gallery pages, openapi.json, both v1/v2 gallery APIs, and a
/// CDN-config endpoint) plus the final code+explanation inference itself — 8 rounds left too much
/// room for the model to keep exploring instead of converging on an answer within the time budget.
const MAX_LOOP_ROUNDS: usize = 5;

/// research.md §6 — bounds the *entire* agentic loop (potentially several `tool_chat`/
/// `fetch_page` round-trips), separate from each individual call's own timeout. Raised from 120s
/// (2026-08-25) now that generation streams progress live (`content_delta`/`fetch_page`/
/// `fetch_result` SSE events) instead of returning one final response — the original 120s was
/// picked specifically to bound how long the UI would show nothing but a spinner, which no longer
/// applies once every round is visible as it happens; a real multi-round generation with several
/// genuinely necessary `fetch_page` calls plus a long final answer can legitimately take longer
/// than 120s without actually being stuck.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(300);

/// Splits the model's final answer into its code half and its user-facing explanation half (see
/// `GenerateResponse::explanation`'s own docs). An unlikely-to-collide literal line rather than
/// e.g. a JSON envelope — the code half must stay exactly the raw .ts source `looks_like_plugin_
/// code`/`format_with_deno` already expect verbatim, and `json_object` mode isn't available here
/// anyway (this call still uses `tools`, and DeepSeek documents `response_format: json_object` and
/// `tools` as mutually exclusive — see `analyze_login.rs`'s own docs on that constraint).
const EXPLANATION_MARKER: &str = "///===PLUGIN_WIZARD_EXPLANATION===///";

#[derive(Deserialize)]
pub(super) struct GenerateRequest {
    plugin_type: String,
    #[serde(default)]
    test_links: Vec<String>,
    #[serde(default)]
    auxiliary_reference_urls: Vec<String>,
    #[serde(default)]
    reference_sample_code: Option<String>,
    #[serde(default)]
    login_association: Option<LoginAssociationInput>,
    /// `login` type only — the credential-field list `POST /plugin-wizard/analyze-login` (T-new)
    /// already determined for this target site (password pair, a single token/API key, a raw
    /// cookie value, or something else entirely). AI must declare exactly these as `pluginInfo()`'s
    /// `parameters` and read them positionally from `execLogin`'s `hostArgs.customargs` — never
    /// invent its own field set, since the wizard's later trial-run/save steps are keyed off this
    /// exact list. Absent/empty for non-login types.
    #[serde(default)]
    login_parameters: Vec<LoginParameterInput>,
    /// Present only for an AI-auto-fix call (FR-017/US5) — same endpoint, same loop, just a
    /// different starting user prompt. `None` for a fresh generation.
    #[serde(default)]
    previous_code: Option<String>,
    #[serde(default)]
    previous_error: Option<String>,
    /// User-driven free-text follow-up request (e.g. "帮我加上从 source tag 提取 ID 的回退逻辑"),
    /// distinct from `previous_error`'s trial-run-failure-driven auto-fix — there's no implied
    /// failure here, just a want. Mutually exclusive with `previous_code`/`previous_error` in
    /// practice (the frontend only ever sets one "what changed this round" reason at a time), but
    /// not enforced at the type level since nothing downstream needs that guarantee.
    #[serde(default)]
    refine_instruction: Option<String>,
    /// Every prior round's own (what was asked, what code came back) pair for this same draft,
    /// oldest first — replayed verbatim as alternating user/assistant messages ahead of this
    /// round's own request so the model has full context for a multi-round refinement
    /// conversation, not just the single latest code snapshot. Empty for the very first round.
    /// Owned entirely by the frontend (`TypeSession.conversationHistory`, per `useWizardSession.ts`
    /// — no server-side session/draft history per spec's own frontend-only-state assumption).
    #[serde(default)]
    conversation_history: Vec<ConversationTurn>,
    /// Real values for a same-domain login plugin's own declared credential fields (keyed by
    /// `LoginParameter.name`, same shape `trial_run.rs::Credentials.fields` already uses) — sent
    /// only when this domain already has (or this session already generated) a login plugin *and*
    /// the user has actually typed values into its credential fields, even before any login trial
    /// run has verified them. Never forwarded to the LLM directly (FR-012) — used purely server-
    /// side to substitute `{{name}}` placeholders in an AI-composed `fetch_page` `headers` argument
    /// (`tool_loop.rs::substitute_credential_placeholders`), so the model can request an
    /// authenticated fetch of a real API endpoint without ever seeing the credential itself, only
    /// that fetch's resulting (now genuinely authenticated) page content.
    #[serde(default)]
    credential_values: std::collections::HashMap<String, String>,
    /// Metadata/download only — the same-domain login plugin's own declared credential field
    /// names/descriptions (from `analyze-login`), sent so the model knows *which* `{{name}}`
    /// placeholders it may reference in a `fetch_page` `headers` argument. Deliberately a separate
    /// field from `login_parameters` above: that one instructs the model to re-declare the exact
    /// same fields as *this* plugin's own `pluginInfo().parameters` (only correct when generating
    /// the login type itself); this one is purely informational — these are someone else's
    /// (the login plugin's) already-declared fields, not something this metadata/download plugin
    /// should redeclare.
    #[serde(default)]
    available_credential_fields: Vec<LoginParameterInput>,
}

#[derive(Deserialize)]
struct ConversationTurn {
    user_message: String,
    assistant_code: String,
}

#[derive(Deserialize)]
struct LoginAssociationInput {
    namespace: String,
}

#[derive(Deserialize)]
struct LoginParameterInput {
    name: String,
    description: String,
    required: bool,
}

enum GenerateError {
    /// The final response wasn't parseable as plugin code (spec Edge Cases: AI returned prose
    /// instead of code, or malformed code) — carries the raw output for the user to see.
    AiOutputNotCode(String),
    LlmUnavailable(String),
}

/// `POST /plugin-wizard/generate/start` — see this module's own top-level docs for why generation
/// is split into this plus `generate_stream` rather than one `POST` returning JSON. Accepts the
/// request body as a raw [`serde_json::Value`] (not a typed `GenerateRequest`) specifically so
/// this endpoint never has to duplicate `GenerateRequest`'s own field/default handling — the real
/// typed parse happens once, at `generate_stream` time, against whatever was stashed here
/// unmodified.
pub(super) async fn generate_start(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .pending_generate_requests
        .lock()
        .await
        .insert(id.clone(), body);
    (StatusCode::OK, Json(json!({ "generation_id": id }))).into_response()
}

/// `GET /plugin-wizard/generate/stream/{id}` — the real `EventSource`-compatible streaming
/// endpoint; see this module's own top-level docs for the full event sequence. `id` is single-use
/// (removed from `pending_generate_requests` the moment this is called), so replaying the same id
/// (a stale bookmark, a double-click race) always yields `generation_not_found` rather than
/// silently re-running an old request.
pub(super) async fn generate_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(body) = state.pending_generate_requests.lock().await.remove(&id) else {
        return sse_single_error_response("generation_not_found", "该次生成请求不存在或已被使用。");
    };
    let mut req: GenerateRequest = match serde_json::from_value(body) {
        Ok(req) => req,
        Err(e) => return sse_single_error_response("invalid_request", &e.to_string()),
    };
    let (credential_values, resolved_fields) = resolve_credentials(&state, &req).await;
    // Logged, not silent — a real report ("我记得我在插件设置页面已经输入过key了，为什么没读取
    // 到？", 2026-08-25) turned out undiagnosable with zero visibility into whether this call even
    // *had* a `login_association` to resolve against, whether Redis actually had a saved value for
    // it, or whether the model simply chose not to use the credential it was told about.
    tracing::info!(
        login_association = ?req.login_association.as_ref().map(|a| &a.namespace),
        resolved_from_redis = resolved_fields.is_some(),
        credential_field_count = credential_values.len(),
        "plugin wizard: resolved credential values for this generation",
    );
    if let Some(fields) = resolved_fields {
        req.available_credential_fields = fields;
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        let outcome = tokio::time::timeout(
            GENERATE_TIMEOUT,
            run_generation(&state, &req, &credential_values, &tx),
        )
        .await;
        let final_event = match outcome {
            // `resolved_credential_values` (real Redis-persisted values for the associated login
            // plugin, if any) rides along on `done` so the frontend can auto-prefill the trial-run
            // parameter panel by field-name match — previously computed here purely for the AI's
            // own `fetch_page` substitution and then discarded, leaving the trial-run UI's separate
            // `pluginParameterValues` state permanently blank even when Redis already had the exact
            // value the user was trying to test with (real report, 2026-08-25: "这里还是没读取到
            // 已经设置的key").
            Ok(Ok((code, explanation))) => Event::default().event("done").data(
                json!({
                    "code": code,
                    "explanation": explanation,
                    "resolved_credential_values": credential_values,
                })
                .to_string(),
            ),
            Ok(Err(GenerateError::AiOutputNotCode(raw))) => Event::default()
                .event("error")
                .data(json!({ "error": "ai_output_not_code", "raw_output": raw }).to_string()),
            Ok(Err(GenerateError::LlmUnavailable(detail))) => Event::default()
                .event("error")
                .data(json!({ "error": "llm_unavailable", "detail": detail }).to_string()),
            Err(_) => Event::default().event("error").data(
                json!({ "error": "llm_unavailable", "detail": "生成超时，请重试" }).to_string(),
            ),
        };
        let _ = tx.send(final_event);
    });

    let stream = futures_util::StreamExt::map(
        tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        Ok::<_, std::convert::Infallible>,
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// A bogus/expired id or an unparseable stashed body both happen before there's any real stream to
/// run — `EventSource` can't usefully read a plain non-2xx status's body, so both still respond
/// with a real (one-event) SSE stream carrying an `error` event, matching
/// `compare_queue_item_stream`'s own `error`-event convention.
fn sse_single_error_response(error: &'static str, detail: &str) -> Response {
    let event = Event::default()
        .event("error")
        .data(json!({ "error": error, "detail": detail }).to_string());
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::convert::Infallible>(event) });
    Sse::new(stream).into_response()
}

/// Resolves the *real* credential field names+values for `req.login_association`'s declared
/// namespace, preferring whatever's actually persisted for the real installed plugin over the
/// frontend's own (possibly stale, possibly never-filled) `req.credential_values`/`req.available_
/// credential_fields` — covers the far more common case a same-session-only design missed
/// entirely: the domain's login plugin was installed (and its credentials configured) in an
/// *earlier* session, not this one, so there's no `TypeSession` for it at all right now, only its
/// real, already-saved `LRR_PLUGIN_<NS>` Redis entry (`crate::plugins::get_plugin_customargs`, the
/// exact same storage `with_login_cookies`'s own real login calls already read from). Returns
/// `(values, None)` — falling back to `req.credential_values` as-is, `available_credential_fields`
/// left untouched — only when Redis genuinely has nothing saved for that plugin yet (every
/// customarg slot empty), which is the login TypeSession-this-session-generated-but-not-yet-saved
/// case the original frontend-only design covered.
async fn resolve_credentials(
    state: &AppState,
    req: &GenerateRequest,
) -> (
    std::collections::HashMap<String, String>,
    Option<Vec<LoginParameterInput>>,
) {
    let Some(assoc) = &req.login_association else {
        return (std::collections::HashMap::new(), None);
    };
    if let Some((ns, info)) =
        crate::plugins::resolve_declared_namespace(state, &assoc.namespace).await
    {
        // Login-plugin credential fields are always `string`-typed (never `bool`/`int` — a
        // credential is text), so `.as_str()` always succeeds here; falls back to `""` only for a
        // malformed/unexpected stored shape, same "treat as unconfigured" default
        // `get_plugin_customargs` itself uses.
        let values: Vec<String> =
            crate::plugins::get_plugin_customargs(state, &ns, &info.parameters)
                .await
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect();
        if values.iter().any(|v| !v.is_empty()) {
            let value_map = info
                .parameters
                .iter()
                .zip(&values)
                .map(|(p, v)| (p.name.clone(), v.clone()))
                .collect();
            let fields = info
                .parameters
                .iter()
                .map(|p| LoginParameterInput {
                    name: p.name.clone(),
                    description: p.description.clone(),
                    required: p.required,
                })
                .collect();
            return (value_map, Some(fields));
        }
    }
    (req.credential_values.clone(), None)
}

fn system_prompt(req: &GenerateRequest, deno_version: Option<&str>) -> String {
    crate::llm_prompts::plugin_generation_system_prompt(
        &req.plugin_type,
        req.reference_sample_code.as_deref(),
        req.login_association.is_some(),
        deno_version,
        SAMPLE_METADATA,
        SAMPLE_DOWNLOAD,
        PLUGIN_SDK,
        EXPLANATION_MARKER,
    )
}

fn user_prompt(req: &GenerateRequest) -> String {
    let mut parts = Vec::new();
    if !req.test_links.is_empty() {
        parts.push(format!(
            "目标页面链接（请先用 fetch_page 抓取这些链接查看真实页面结构）：{}",
            req.test_links.join("、")
        ));
    }
    if !req.auxiliary_reference_urls.is_empty() {
        parts.push(format!(
            "辅助参考信息/链接（可能是站点自身的 API/JSON 接口、登录页地址等，同样可以用 fetch_page 查看）：{}",
            req.auxiliary_reference_urls.join("、")
        ));
    }
    if !req.login_parameters.is_empty() {
        let fields = req
            .login_parameters
            .iter()
            .map(|p| {
                format!(
                    "{}（{}，{}）",
                    p.name,
                    p.description,
                    if p.required { "必填" } else { "可选" }
                )
            })
            .collect::<Vec<_>>()
            .join("、");
        parts.push(format!(
            "登录字段列表（已由分析步骤确定，必须原样使用，见上方说明）：{fields}"
        ));
    }
    if let Some(assoc) = &req.login_association {
        parts.push(format!(
            "这个插件依赖登录才能正常工作，对应的登录插件命名空间为 \"{}\"，请在 pluginInfo() 中声明 \
            login_from: \"{}\"。",
            assoc.namespace, assoc.namespace
        ));
        if !req.available_credential_fields.is_empty() {
            let fields = req
                .available_credential_fields
                .iter()
                .map(|p| format!("{}（{}）", p.name, p.description))
                .collect::<Vec<_>>()
                .join("、");
            parts.push(format!(
                "配套登录插件的凭证字段为：{fields}。请直接假设这些凭证是真实、已经就绪可用的——如果你\
                观察到某个接口/页面需要认证才能访问真实数据（比如返回 401/403，或接口文档明确标注需要 \
                Auth/Token/API Key），应主动在 fetch_page 的 headers 参数里引用对应字段名，写成 \
                {{{{字段名}}}} 形式的占位符（例如 headers: {{ \"Authorization\": \"Key {{{{token}}}}\" }}），\
                系统会在真正发出请求前用用户已经填好的真实值替换占位符，你自己不会看到替换后的值，只会\
                看到这次认证访问真正返回的页面内容。不要因为不确定有没有凭证就直接放弃认证路径、退而求\
                其次去猜测免登录的访问方式——凭证已经确定存在，你要做的是把访问方式具体定下来。"
            ));
        }
    }
    if let (Some(code), Some(err)) = (&req.previous_code, &req.previous_error) {
        parts.push(format!(
            "上一次生成的代码试运行失败，请基于错误信息修正：\n\n上一次的代码：\n```ts\n{code}\n```\n\n\
            试运行错误信息：{err}"
        ));
    }
    if let Some(instruction) = &req.refine_instruction {
        parts.push(format!(
            "用户在已生成的代码基础上，提出了以下进一步需求，请据此修改代码（当前代码已经在上面的对话\
            历史里给出，不需要重复粘贴）：{instruction}"
        ));
    }
    parts.join("\n\n")
}

/// Splits the model's final answer on [`EXPLANATION_MARKER`] into `(code, explanation)`. Falls
/// back to treating the whole response as code with a generic placeholder explanation if the
/// model forgot the marker entirely — the code half is the essential deliverable and
/// `looks_like_plugin_code` already independently guards its validity, so a missing explanation
/// alone must never fail the whole generation.
fn split_code_and_explanation(content: &str) -> (String, String) {
    match content.split_once(EXPLANATION_MARKER) {
        Some((code, explanation)) => (code.trim().to_string(), explanation.trim().to_string()),
        None => (
            content.trim().to_string(),
            "（本次生成未附带说明。）".to_string(),
        ),
    }
}

/// A lightweight structural check, not a full TS typecheck (T018's own scope) — just enough to
/// reject an obviously-non-code response (prose, an apology, an empty string) before handing it
/// back to the user as a usable draft.
/// A lightweight structural check, not a full TS typecheck — just enough to reject an obviously
/// non-code or *incomplete* response before handing it back to the user as a usable draft. Beyond
/// the original "does it mention pluginInfo at all" check, also requires the type-appropriate
/// entry function to actually appear and requires balanced braces/parens — a real, observed
/// failure mode is the model's response getting cut off mid-file by the `max_tokens` limit, which
/// still trivially contains the substring "pluginInfo" (it's declared near the top of every
/// plugin) despite the rest of the file — including the entry function's own closing brace —
/// never having been generated.
fn looks_like_plugin_code(content: &str, plugin_type: &str) -> bool {
    if !content.contains("pluginInfo") {
        return false;
    }
    let entry_fn = match plugin_type {
        "metadata" => "execMetadata",
        "download" => "execDownload",
        _ => "execLogin",
    };
    if !content.contains(entry_fn) {
        return false;
    }
    balanced(content, '{', '}') && balanced(content, '(', ')')
}

/// Counts `open`/`close` occurrences outside of string/template literals, comments, and regex
/// literals — a bare character scan false-positives on a lone paren inside a comment/string, or on
/// a regex like `/^https?:\/\//i` whose escaped slashes read as a `//` line comment. Regex vs.
/// division is resolved by the standard heuristic: a `/` starts a regex unless the previous
/// non-whitespace char is an identifier char/digit/`)`/`]`/`}`. Template-literal `${...}`
/// interpolation isn't re-entered as code — its parens still count toward the same outer balance.
fn balanced(content: &str, open: char, close: char) -> bool {
    let mut depth: i32 = 0;
    let mut chars = content.chars().peekable();
    let mut prev_significant: char = '\0';
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                prev_significant = '\n';
                continue;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                prev_significant = ' ';
                continue;
            }
            '/' if !matches!(
                prev_significant,
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$' | ')' | ']' | '}'
            ) =>
            {
                // Regex literal: consume up to its own unescaped closing `/`, then any trailing
                // flag letters (`gimsuy` etc. — accepting any identifier char here rather than
                // validating the exact flag set, since this function only needs to find the
                // literal's real end, not validate the regex itself).
                let mut in_class = false;
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                    } else if c == '[' {
                        in_class = true;
                    } else if c == ']' {
                        in_class = false;
                    } else if c == '/' && !in_class {
                        break;
                    } else if c == '\n' {
                        // A real regex literal never spans a newline — if we hit one, this `/`
                        // almost certainly wasn't a regex start after all (e.g. a division
                        // mis-heuristic); bail out of regex-consuming mode without eating
                        // anything further so the rest of the line still gets scanned normally
                        // for real braces/parens rather than silently disappearing.
                        break;
                    }
                }
                while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    chars.next();
                }
                prev_significant = '/';
                continue;
            }
            '\'' | '"' | '`' => {
                let quote = c;
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                    } else if c == quote {
                        break;
                    }
                }
                prev_significant = quote;
                continue;
            }
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
        if !c.is_whitespace() {
            prev_significant = c;
        }
    }
    depth == 0
}

/// Queries the real, exact Deno runtime version this generated code will actually be executed
/// under (via `PluginPool`'s own configured binary — the same one `format_with_deno` below shells
/// out to) rather than hardcoding a version string in the prompt that would silently go stale the
/// next time this deployment's Deno gets upgraded. `None` on any failure (binary missing, spawn
/// error, unparseable output) — the caller falls back to a generic, honestly-unversioned
/// description rather than fabricating a number.
async fn deno_version(deno_binary: &str) -> Option<String> {
    let output = tokio::process::Command::new(deno_binary)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // First line looks like "deno 2.9.1 (stable, release, x86_64-unknown-linux-gnu)".
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .next()
        .map(|s| s.trim().to_string())
}

/// Real formatting via `deno fmt -` (stdin in, stdout out — the `-` filename argument tells Deno
/// to format piped input instead of a real file on disk, so this never needs a temp file). Uses
/// the exact same `deno` binary `PluginPool` spawns every plugin worker with, not a bare `"deno"`
/// on `PATH`, so this never silently uses a different (or absent) Deno install than the one that
/// actually runs the generated plugin. Errors (deno missing, a real syntax error `deno fmt` itself
/// rejects, non-UTF8 output) are the caller's to handle — this never panics or blocks generation.
async fn format_with_deno(deno_binary: &str, code: &str) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(deno_binary)
        .args(["fmt", "--ext", "ts", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn deno fmt: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("deno fmt: no stdin handle")?;
    stdin
        .write_all(code.as_bytes())
        .await
        .map_err(|e| format!("failed to write to deno fmt stdin: {e}"))?;
    drop(stdin); // close stdin so `deno fmt` sees EOF and actually starts formatting

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("deno fmt did not complete: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "deno fmt exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("deno fmt produced non-UTF8 output: {e}"))
}

/// The URL `fetch_page` targets when AI's tool call doesn't specify one — the first supplied test
/// link, falling back to the first auxiliary reference URL (contracts/plugin-wizard-api.md's own
/// note on this default).
fn default_fetch_url(req: &GenerateRequest) -> Option<String> {
    req.test_links
        .first()
        .or(req.auxiliary_reference_urls.first())
        .cloned()
}

async fn run_generation(
    state: &AppState,
    req: &GenerateRequest,
    credential_values: &std::collections::HashMap<String, String>,
    event_tx: &UnboundedSender<Event>,
) -> Result<(String, String), GenerateError> {
    let deno_version = deno_version(state.plugins.deno_binary()).await;
    let mut messages = vec![Message::system(system_prompt(req, deno_version.as_deref()))];
    // Replay every prior round's own (ask, code) pair first, oldest first, so the model has full
    // multi-round context — not just this round's own new request layered on the latest code
    // snapshot. Only the code half of each prior round is replayed (not its explanation): a later
    // round only ever needs to know "what code did you write", not the user-facing summary of it.
    for turn in &req.conversation_history {
        messages.push(Message::user(turn.user_message.clone()));
        messages.push(Message::assistant(turn.assistant_code.clone()));
    }
    messages.push(Message::user(user_prompt(req)));
    let tools = vec![fetch_page_tool()];

    for _ in 0..MAX_LOOP_ROUNDS {
        // `false`: this loop's final answer is a raw .ts source file, not JSON — `force_json_
        // content` is for analyze_login.rs's structured-array case, not this one.
        // 16000 (raised from 8000, itself raised from 4000): `max_tokens` bounds `reasoning_
        // content` *and* `content` combined for this reasoning model, not `content` alone — a
        // real generation was observed spending its entire 8000-token budget on reasoning and
        // emitting zero bytes of actual code (`finish_reason: "length"`, `reasoning_tokens: 8000`,
        // 2026-08-25; see `lanrurugi-llm`'s own `thinking_low_effort` docs for the primary fix,
        // steering the model toward less of that in the first place — this larger ceiling is the
        // second line of defense, not a replacement for it).
        //
        // Streaming (not plain `tool_chat`): a tool-calling *decision* round is typically fast
        // (observed live: 0.3-4s), but the FINAL content-only round — the one that actually writes
        // the code+explanation — was observed taking 80+ seconds by itself, with zero visibility
        // into whether anything was happening. `on_content_delta` forwards every chunk of that
        // final round live as a `content_delta` SSE event the instant it arrives; a tool-calling
        // round never invokes this closure at all (the model emits `tool_calls`, not `content`, in
        // that case), so this one call site correctly covers both round shapes without needing to
        // know in advance which one a given round will turn out to be.
        let event_tx_for_delta = event_tx.clone();
        let response = state
            .redis
            .config
            .tool_chat_streaming(&messages, &tools, 0.3, 16000, false, move |delta| {
                let _ = event_tx_for_delta.send(
                    Event::default()
                        .event("content_delta")
                        .data(json!({ "text": delta }).to_string()),
                );
            })
            .await
            .map_err(GenerateError::LlmUnavailable)?;

        match response {
            ToolChatResponse::Content(content) => {
                let (code, explanation) = split_code_and_explanation(&content);
                if !looks_like_plugin_code(&code, &req.plugin_type) {
                    // Logged, not silent — this failure previously left no trace at all (the
                    // frontend's own `err.message` for this case was, until a companion fix,
                    // *also* just the bare "ai_output_not_code" code string with no diagnostic
                    // content), making a real "AI 未能生成有效代码" report undiagnosable from
                    // either side (observed live 2026-08-24). Logs a structural breakdown (which
                    // specific check failed) plus the full raw content, not just a length/preview
                    // — this is a rare failure path, not a hot loop, so the extra log volume is
                    // worth having the complete picture on the very first occurrence.
                    tracing::warn!(
                        plugin_type = %req.plugin_type,
                        content_len = content.len(),
                        marker_found = content.contains(EXPLANATION_MARKER),
                        has_plugin_info = code.contains("pluginInfo"),
                        braces_balanced = balanced(&code, '{', '}'),
                        parens_balanced = balanced(&code, '(', ')'),
                        content = %content,
                        "plugin wizard: AI's final answer failed looks_like_plugin_code",
                    );
                    // The raw, unsplit response is what's most useful to show the user here (the
                    // split code half alone could itself be misleadingly truncated at whatever
                    // text happened to look like the marker) — matches the pre-existing contract
                    // of `AiOutputNotCode` carrying the model's literal output.
                    return Err(GenerateError::AiOutputNotCode(content));
                }
                // Prompt instructions alone (standard 2-space indent, etc.) are a real but soft
                // constraint — a live-observed generation still came back with runaway indentation
                // despite them. Run the AI's own output through a real formatter (the same `deno`
                // binary the plugin sandbox itself uses, via `PluginPool::deno_binary`) rather than
                // trusting prompt compliance alone; if formatting fails for any reason (deno not on
                // PATH in some deployment, a genuine syntax error `looks_like_plugin_code`'s
                // lightweight brace-balance check didn't catch), fall back to the AI's raw output
                // rather than failing the whole generation over a cosmetic step.
                let formatted = match format_with_deno(state.plugins.deno_binary(), &code).await {
                    Ok(formatted) => formatted,
                    Err(err) => {
                        // Logged, not silent — a `deno fmt` failure here previously left no trace
                        // at all (the raw-output fallback swallowed the `Err` unconditionally),
                        // making a real "code came back unformatted" report undiagnosable after
                        // the fact (observed live 2026-08-24).
                        tracing::warn!(error = %err, "plugin wizard: deno fmt failed, returning unformatted code");
                        code
                    }
                };
                return Ok((formatted, explanation));
            }
            ToolChatResponse::ToolCalls(calls) => {
                messages.push(Message::assistant_tool_calls(calls.clone()));
                for call in calls {
                    let url = extract_url_arg(&call).or_else(|| default_fetch_url(req));
                    let Some(url) = url else {
                        messages.push(Message::tool_result(
                            call.id.clone(),
                            json!({ "status": "error", "error": "没有可供访问的 URL" }).to_string(),
                        ));
                        continue;
                    };
                    let _ = event_tx.send(
                        Event::default()
                            .event("fetch_page")
                            .data(json!({ "url": url }).to_string()),
                    );
                    let headers = substitute_credential_placeholders(
                        extract_headers_arg(&call),
                        credential_values,
                    );
                    let tool_result = execute_fetch_tool(&url, &headers).await;
                    let status = serde_json::from_str::<serde_json::Value>(&tool_result)
                        .ok()
                        .and_then(|v| v["status"].as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "unknown".to_string());
                    let _ = event_tx.send(
                        Event::default()
                            .event("fetch_result")
                            .data(json!({ "url": url, "status": status }).to_string()),
                    );
                    messages.push(Message::tool_result(call.id.clone(), tool_result));
                }
            }
        }
    }

    Err(GenerateError::LlmUnavailable(
        "生成未能在合理轮数内收敛，请重试".to_string(),
    ))
}

#[cfg(test)]
mod balanced_tests {
    use super::balanced;

    /// The exact real, observed false positive (2026-08-25): a `//` comment's own English
    /// parenthetical happens to close on the following comment line, which the old naive
    /// character-count implementation misread as an unbalanced paren in the surrounding real
    /// code, rejecting an otherwise-valid generated download plugin.
    #[test]
    fn a_lone_paren_split_across_two_comment_lines_does_not_break_balance() {
        let code = "function f() {\n  // the API explicitly warns against reconstructing archives by (fetching\n  // each page image).\n  return 1;\n}\n";
        assert!(balanced(code, '(', ')'));
        assert!(balanced(code, '{', '}'));
    }

    #[test]
    fn a_lone_paren_inside_a_string_literal_does_not_break_balance() {
        let code = r#"const s = "close paren only )"; function f() { return s; }"#;
        assert!(balanced(code, '(', ')'));
    }

    #[test]
    fn a_lone_paren_inside_a_block_comment_does_not_break_balance() {
        let code = "/* stray ) paren */\nfunction f() { return 1; }";
        assert!(balanced(code, '(', ')'));
    }

    #[test]
    fn an_escaped_quote_inside_a_string_does_not_end_it_early() {
        // Without escape handling, the `\"` here would be read as the string's own closing quote,
        // leaving the real `)` that follows counted as "inside a string" and skipped — which
        // would happen to still balance in this particular example, so the case that actually
        // matters is one where getting escape handling wrong would flip the real answer: a
        // genuinely unbalanced close-paren hidden after a wrongly-terminated string.
        let code = r#"const s = "a \" b )"; function f(x) { return x }"#;
        assert!(balanced(code, '(', ')'));
    }

    #[test]
    fn a_genuinely_truncated_file_still_reports_unbalanced() {
        // The failure mode this function exists to catch in the first place (module docs) — a
        // response cut off mid-file by max_tokens, missing its own closing braces/parens.
        let truncated = "export function pluginInfo() {\n  return {\n    namespace: \"x\",";
        assert!(!balanced(truncated, '{', '}'));
    }

    /// The exact real, observed false positive on a *second* generation the same day, after the
    /// comment-paren fix above already shipped: a regex literal with an escaped trailing slash
    /// before its own closing delimiter (`/^https?:\/\//i`) contains a genuine `//` substring
    /// (the second `\/`'s `/` immediately followed by the regex's own closing `/`), which — before
    /// this function understood regex literals at all — was misread as the start of a `//` line
    /// comment, silently swallowing the rest of that real code line (`i.test(fileUrl)) {`) and
    /// throwing off the brace/paren count for the remainder of an otherwise entirely valid,
    /// hand-verified-correct generated download plugin.
    #[test]
    fn a_regex_literal_with_an_escaped_trailing_slash_does_not_break_balance() {
        let code = r#"if (!/^https?:\/\//i.test(fileUrl)) {
  fileUrl = `https://nhentai.net/${fileUrl.replace(/^\/+/, "")}`;
}"#;
        assert!(balanced(code, '(', ')'));
        assert!(balanced(code, '{', '}'));
    }

    /// A bare `/` after an identifier/number/`)`/`]`/`}` is division, not the start of a regex
    /// literal — the heuristic must not treat every `/` as a regex opener, or a genuinely
    /// unbalanced file containing division would be misread as balanced by accident (the regex
    /// consumer eating real braces/parens that happen to follow on the same line).
    #[test]
    fn plain_division_is_not_mistaken_for_a_regex_literal() {
        let code = "function f(a: number, b: number) {\n  const x = a / b / 2;\n  return x;\n}";
        assert!(balanced(code, '{', '}'));
        assert!(balanced(code, '(', ')'));
    }

    #[test]
    fn a_character_class_containing_a_slash_does_not_end_the_regex_early() {
        // `[/]` inside a character class is a literal `/`, not the regex's own closing delimiter
        // — getting this wrong would end the regex at the wrong point and misparse whatever
        // (redundantly-escaped-in-real-source-but-still-legal) code follows.
        let code = r#"const r = /[a/b]{2}/; function f() { return r; }"#;
        assert!(balanced(code, '{', '}'));
        assert!(balanced(code, '(', ')'));
    }
}
