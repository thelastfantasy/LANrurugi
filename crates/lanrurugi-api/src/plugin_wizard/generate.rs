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
use lanrurugi_llm::{tool_chat_streaming, Message, ToolChatResponse};
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
    let (sample, sample_is_same_domain) = match &req.reference_sample_code {
        Some(code) => (code.as_str(), true),
        None => (
            if req.plugin_type == "download" {
                SAMPLE_DOWNLOAD
            } else {
                SAMPLE_METADATA
            },
            false,
        ),
    };
    // FR-009: a same-domain sample can be a *different* plugin type than the one being generated
    // now (the target type, by definition, never has an existing sample — FR-004 only lets the
    // user select types the domain lookup found missing) — e.g. generating "download" for a
    // domain whose "login" plugin already exists reuses that login plugin's source, even though
    // its execLogin/credential-handling shape has nothing to do with execDownload's own contract.
    // The wording below must say so explicitly: URL-matching/cookie/page-parsing conventions
    // transfer across types on the same domain (genuinely useful), but the entry-function
    // signature/return shape do NOT (must come from the SDK doc above instead) — an earlier
    // version of this text claimed the sample was "同类型" (same-type) unconditionally, which
    // could mislead the model into copying a structurally unrelated entry function.
    let sample_note = if sample_is_same_domain {
        "以下是该域名下已存在的插件代码（注意：类型可能与你现在要生成的不同，因为同一个域名下\
        目标类型本身从不会已经存在插件——你现在要生成的类型是上面缺失的那个）。它的 URL 匹配方式、\
        cookie/登录状态处理、页面解析思路仍然值得参考，但入口函数签名和返回值结构是每种类型各自\
        独有的，不要因为看到这份样例就照抄其入口函数形状——具体以上面 SDK 文档里你现在要生成的\
        类型的真实接口定义为准："
    } else {
        "该域名下没有可参考的现存插件；以下是一个通用教学样例（结构参考，不是同域名插件）："
    };

    let configurable_options_note = "此外，请结合你观察到的真实页面内容，主动判断该网站是否存在值得让\
        用户自己选择的可配置项——例如页面同时提供多语种标题/描述、有多种可选的标签命名风格、同一资源有\
        多种画质/分辨率可选等（多语种标题只是一个例子，不要局限于这一种情况，具体以你实际抓到的页面内容\
        为准；如果页面结构简单、没有这类可选项，就不必强行发明一个）。发现这类可配置项时，把它们声明为 \
        pluginInfo() 的 parameters（每项包含 name/description/required，required 通常为 false，因为\
        这类选项一般有合理的默认行为），并在入口函数里从 hostArgs.customargs 按声明顺序读取对应的值—— \
        这与已安装插件的设置页使用的是同一套机制，用户保存插件后可以在插件设置页里随时修改这些选项，\
        不需要重新生成代码。";

    let login_cookie_note = if req.login_association.is_some() {
        "\n\n认证方式的优先级（这个插件已经关联了一个登录插件，见下方\"登录字段列表\"/\"配套登录插件\"\
         说明）：真正的认证凭证在运行时通过 hostArgs.user_agent_cookies（cookie 认证站点）和/或 \
         hostArgs.user_agent_headers（header/token 认证站点，例如 Authorization: Key <api_key> 这种）\
         传入（宿主在每次调用前先跑一遍关联的登录插件，把它 execLogin 返回值里的 cookies/headers 两个\
         字段分别注入到这里——具体两者哪个有值，取决于关联的登录插件本身是走 cookie 还是走 header 认证，\
         你无法预先假设，必须两者都兼容处理），入口函数必须像这样消费——\
         `const ua = legacyCompat.userAgent(); \
         for (const c of (hostArgs.user_agent_cookies ?? [])) ua.cookie_jar.add(c); \
         const headers = hostArgs.user_agent_headers ?? {}; \
         if (Object.keys(headers).length > 0) ua.on(\"start\", (_ua, tx) => { for (const [k, v] of Object.entries(headers)) tx.req.headers.header(k, v); });` \
         然后用这个已经带上认证态的 ua 发起请求。绝对不要在这个插件自己的 pluginInfo().parameters / \
         customargs 里再重复声明一份账号/密码/API Key/Token 之类的凭证字段——凭证只应该在登录插件那一侧\
         声明和填写一次，下载/元数据插件这一侧只管消费 user_agent_cookies/user_agent_headers，不能形成\
         两条平行、互不相干的认证路径（这是一个已经在真实生成结果中出现过的错误：生成的下载插件声明了 \
         login_from 却完全没用上 user_agent_cookies，反而自己又加了一个重复的凭证参数；另一个已经在真实\
         代码库中发现过的错误是关联的登录插件本身走 header 认证却只返回了裸的 ua 对象而不声明 headers 字段\
         ——ua 上通过 on(\"start\", ...) 设置的 header 是闭包函数，跨进程 JSON 序列化时会被静默丢弃，\
         execLogin 必须显式在返回值里写 headers: {{ ... }} 才能让这个凭证真正传出去，见下方登录类型的\
         专门说明）。下面提到的\"把 Auth 信息声明为 configurable option 从 customargs 读取\"的建议，只\
         适用于**没有关联登录插件**、需要独立认证的情况，这里不适用。"
    } else {
        ""
    };

    let args_shape_note = match req.plugin_type.as_str() {
        "metadata" => {
            format!(
                "重要：execMetadata(hostArgs) 的 hostArgs 实际结构为 \
                 {{ url: string; arg: string; customargs: string[]; existing_tags: string; \
                 archive_title: string; thumbnail_hash: string; user_agent_cookies?: {{name,value,domain,path}}[]; \
                 user_agent_headers?: Record<string, string> }}。\
                 目标页面地址在 hostArgs.url（与 hostArgs.arg 相同），不是 oneshot_arg，也不是 archive_id —— \
                 插件 SDK 文档里的示例样例代码使用的是插件正式安装后、真实扫描归档时的参数形状，与向导试运行时\
                 传入的参数形状不同，请以这里给出的真实结构为准。{login_cookie_note}\n\n{configurable_options_note}"
            )
        }
        "download" => {
            format!(
                "重要：execDownload(hostArgs) 的 hostArgs 实际结构为 \
                 {{ url: string; category: string; customargs: string[]; user_agent_cookies?: {{name,value,domain,path}}[]; \
                 user_agent_headers?: Record<string, string> }}。\
                 目标页面地址在 hostArgs.url，不是 oneshot_arg，也不是 archive_id —— 插件 SDK 文档里的示例样例\
                 代码使用的是插件正式安装后、真实场景下的参数形状，与向导试运行时传入的参数形状不同，请以这里\
                 给出的真实结构为准。\n\n\
                 定位真实下载地址时：如果接口文档里已经存在一个明确标注为下载/获取文件相关的端点\
                 （通常会标注 Auth 方式、Rate limit、需要的参数等），优先直接采用文档给出的这个端点，\
                 不要舍近求远去猜测拼接未文档化的 CDN/图片地址——文档化的官方接口更稳定，不容易因为站点\
                 改版而失效。扫描文档罗列的多个端点时，路径或名称里带 download、archive、file 这类字样的\
                 端点应优先尝试。如果该端点标注需要 Auth（token/API key/cookie 等），把它作为一个\
                 configurable option 声明（见下方说明），从 hostArgs.customargs 读取，不要假设一定有\
                 免登录的下载方式。{login_cookie_note}\n\n{configurable_options_note}"
            )
        }
        _ => {
            "重要：execLogin(hostArgs) 的 hostArgs 实际结构为 { customargs: string[] }。customargs \
             数组按下面给出的\"登录字段列表\"顺序对应传入——你必须把这个字段列表原样声明为 pluginInfo() \
             的 parameters（name/description/required 三个字段照抄，不要自己另外发明字段名或增减字段），\
             并在 execLogin 里按声明顺序从 customargs[0]、customargs[1]... 依次读取，不能假设一定是账号\
             密码两个字段——具体是账号密码、还是单个 token/API key、还是 cookie 值，以字段列表的实际\
             内容为准。\n\n\
             execLogin 的返回值必须能真正让下游元数据/下载插件用上这次登录得到的凭证，返回值形状为 \
             LoginResult = {{ cookies?: {{name,value,domain,path}}[]; headers?: Record<string, string>; \
             error?: {{...}} }}——按目标站点的真实认证方式二选一（也可以两者都填）：\
             (1) 如果站点用 Set-Cookie/session cookie 认证，用 legacyCompat.userAgent() 构造 ua、通过 \
             ua.cookie_jar.add(...) 添加 cookie，最后 return ua（宿主只读取其中的 cookies 字段，其余属性\
             会在跨进程传输时被丢弃，这是预期行为）；\
             (2) 如果站点用 Authorization/自定义 header 或裸 token 认证（例如 API Key），绝对不要指望通过\
             ua.on(\"start\", ...) 设置的 header 能传出去——那是一个闭包函数，会在跨进程 JSON 序列化时被\
             静默丢弃，下游插件永远收不到。正确做法是直接在返回值里显式声明 \
             `return {{ headers: {{ Authorization: `Key ${{apiKey}}` }} }};`（字段名和值按目标站点真实\
             要求的 header 名/格式来定，不必是 Authorization），完全不需要用到 legacyCompat.userAgent()。\
             不确定目标站点是 cookie 认证还是 header 认证时，以你在 fetch_page 里观察到的真实响应头/请求\
             要求为准，不要凭经验猜测。"
                .to_string()
        }
    };

    let runtime_note = match deno_version {
        Some(v) => format!(
            "运行环境：这份代码最终会被 Deno（{v}）作为 ES 模块直接执行，不是 Node.js——没有 \
            require/module.exports，全局已经有标准 fetch/URL/TextEncoder 等 Web API 可用，不需要\
            额外 import 它们；文件扩展名是 .ts，但 Deno 运行 .ts 时只是把类型注解剥离后当 ES 模块\
            执行，并不做独立的类型检查。"
        ),
        None => "运行环境：这份代码最终会被 Deno 作为 ES 模块直接执行，不是 Node.js——没有 require/\
            module.exports，全局已经有标准 fetch/URL/TextEncoder 等 Web API 可用。"
            .to_string(),
    };

    format!(
        "你是 LANrurugi 项目的插件开发助手。{runtime_note}\n\n以下是插件 SDK 的完整类型/接口定义：\n\n```ts\n{PLUGIN_SDK}\n```\n\n\
        {sample_note}\n\n```ts\n{sample}\n```\n\n\
        {args_shape_note}\n\n\
        请生成一个类型为 \"{}\" 的插件代码，遵循 SDK 约定导出 pluginInfo() 和对应的入口函数。\
        pluginInfo() 的返回值必须包含 generated_by_wizard: true（这是本向导生成的插件的持久化标记，\
        必须原样保留，不得省略）。\n\n\
        pluginInfo() 的返回值还必须包含 domain_match 字段——一个字符串数组，列出这个插件真正处理的\
        每一个裸域名（不带协议前缀、不带路径，例如 [\"nhentai.net\"]，而不是 \"nhentai.net/g/\" 这种\
        带路径的写法）。domain_match 和 url_pattern 用途完全不同，不要混淆：url_pattern 仍按你平时的\
        写法来，是判断一个具体 URL 是否该触发这个插件真正抓取/下载的精确正则，可以包含路径/参数等\
        限制条件；domain_match 只用于回答\"这个域名是否已经有插件在处理\"这种更宽松的归属判断，只列\
        域名本身，不要写正则或路径片段。如果这个插件对应多个等价域名（如同一站点的桌面版/移动版域名），\
        把它们都列进 domain_match 数组。\n\n\
        用户没有提供页面结构描述——你必须主动调用 fetch_page 工具抓取下面\
        提供的真实链接，根据真实返回的内容自行判断目标字段（标题/标签/下载地址等）在页面中的选择器或\
        提取方式，不要凭空猜测选择器。\n\n\
        提供的链接里如果同时包含接口文档类地址（能看出是 API 文档/OpenAPI 规范的链接，或者抓回来的\
        内容本身就是接口说明文本而不是一个具体资源页）和具体资源样例页面（比如某个画廊/条目的详情\
        页），两者的分析价值不对等：文档类链接应该优先、完整地抓取，它直接给出数据结构、字段名、\
        接口路径这些真正需要的信息；具体资源样例页面主要用于核对你从文档里理解的结构是否与真实返回\
        一致，抓 1-2 个核对即可，不需要为了互相印证而把每一个样例链接都抓一遍——尤其是文档已经把\
        结构讲清楚时，继续逐个抓取样例链接不会带来新的结构性信息，只会浪费时间、增加超时风险。\n\n\
        格式要求（严格遵守，不要模仿上方 SDK 类型定义文件开头可能出现的写法）：\n\
        - 绝对不要输出 /// <reference types=\"...\" /> 这类三斜线指令。上面贴出的 SDK 类型定义文件\
        自己开头有一行这样的指令，那是那个文件自己在仓库目录结构里的相对路径引用，只对它自己有意义；\
        你生成的插件文件路径和用途都完全不同，机械照抄这一行只会产生一个指向错误路径、毫无意义的引用，\
        必须整行省略。\n\
        - 使用标准的 2 空格缩进，同一层级的代码保持相同缩进量，进入代码块（{{、(、[）缩进只增加一级、\
        退出代码块缩进立即恢复到该级别原来的宽度——不要出现缩进随行数递增、只增不减的情况。\n\
        - 不要重复声明上方 SDK 类型定义文件里已经给出的接口本身（如 PluginInfoResult、\
        DeclaredPermissions、MetadataResult、DownloadResult 等——这些类型已经在上面完整给出，直接\
        引用/复用它们描述的形状即可，不需要在你自己的文件里重新写一遍 interface/type 声明）。\
        但这**不代表**整份代码可以退化成不带类型的纯 JavaScript——这仍然是一份 .ts \
        文件，你自己声明的辅助函数、局部变量、返回值该标类型就要标类型，正常写地道的 TypeScript；\
        只省略重新声明一遍 SDK 已经给出的接口这一件具体的事，不要把这条理解成整个文件都不用写\
        类型注解。把篇幅留给真正的业务逻辑（页面抓取、字段提取），不要为了看起来更规范而重复抄写\
        已经在 SDK 文档里出现过的接口定义。\n\n\
        最终按以下格式输出，分两部分，不要用 markdown 代码块包裹任何一部分：\n\
        第一部分是完整的 .ts 源代码本身；紧接着另起一行，写上分隔符 {EXPLANATION_MARKER}；分隔符之后，\
        用中文简要说明这段代码具体做了什么——它依赖哪些真实页面/接口结构、从中提取了哪些字段、有没有做\
        容错或多种情况的回退处理，让用户不用读代码就能大致判断这个插件是否符合预期、有没有遗漏明显应该\
        支持的情况。这段说明是给最终用户看的产品说明，不是代码注释，不需要逐行讲解实现细节。\n\n\
        重要：这个最终答案的**第一个字符**就必须是代码本身（比如 export function pluginInfo()），\
        前面不能有任何导言、总结陈述或类似我已经分析完毕、关键发现是这类过渡性文字——如果你想\
        描述分析过程或思路，只能放在分隔符 {EXPLANATION_MARKER} 之后的说明部分，绝不能出现在代码\
        前面，哪怕只有一行。代码前面一旦出现任何非代码文本，整个文件就不再是合法的 TypeScript，会\
        直接导致格式化和加载失败。",
        req.plugin_type,
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
        let response = tool_chat_streaming(
            &state.redis.config,
            &messages,
            &tools,
            0.3,
            16000,
            false,
            move |delta| {
                let _ = event_tx_for_delta.send(
                    Event::default()
                        .event("content_delta")
                        .data(json!({ "text": delta }).to_string()),
                );
            },
        )
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
