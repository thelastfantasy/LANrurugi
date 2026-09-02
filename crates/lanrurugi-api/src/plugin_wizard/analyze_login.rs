//! `POST /plugin-wizard/analyze-login` — a focused agentic loop (FR-010's "system executes — AI
//! judges" pattern, same shape as `generate.rs`'s own loop) that inspects a real login page/API
//! doc and decides what credential fields a login plugin for that site actually needs: a
//! password pair, a single token/API key, a raw cookie value, or something else — never assumed
//! to always be account+secret. Returns a `parameters` array in the same shape
//! `PluginInfoResult.parameters` uses, so it can be echoed verbatim into the login plugin
//! `generate.rs` produces afterward (`GenerateRequest.login_parameters`) and drives the wizard's
//! own dynamic credential-field form.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use lanrurugi_llm::{tool_chat, Message, ToolChatResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::tool_loop::{execute_fetch_tool, extract_url_arg, fetch_page_tool};
use crate::AppState;

const MAX_LOOP_ROUNDS: usize = 6;

/// Same order of magnitude as `generate.rs::GENERATE_TIMEOUT` but shorter — this loop only needs
/// to inspect one page/doc and emit a short JSON array, not synthesize a whole plugin file.
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
pub(super) struct AnalyzeLoginRequest {
    /// The login page or API-documentation URL to inspect — required; `fetch_page` starts here
    /// if AI's own first tool call doesn't specify a different URL.
    reference_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub(super) struct LoginParameter {
    name: String,
    description: String,
    required: bool,
}

#[derive(Serialize)]
struct AnalyzeLoginResponse {
    parameters: Vec<LoginParameter>,
}

enum AnalyzeError {
    AiOutputNotParameters(String),
    LlmUnavailable(String),
}

pub(super) async fn analyze_login(
    State(state): State<AppState>,
    Json(req): Json<AnalyzeLoginRequest>,
) -> Response {
    match tokio::time::timeout(ANALYZE_TIMEOUT, run_analysis(&state, &req)).await {
        Ok(Ok(parameters)) => {
            (StatusCode::OK, Json(AnalyzeLoginResponse { parameters })).into_response()
        }
        Ok(Err(AnalyzeError::AiOutputNotParameters(raw))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "ai_output_not_parameters", "raw_output": raw })),
        )
            .into_response(),
        Ok(Err(AnalyzeError::LlmUnavailable(detail))) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "llm_unavailable", "detail": detail })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "llm_unavailable", "detail": "分析超时，请重试" })),
        )
            .into_response(),
    }
}

fn system_prompt() -> String {
    "你是 LANrurugi 项目的插件开发助手，现在的任务不是生成代码，而是判断一个网站的真实登录机制需要\
     哪些凭据字段。请调用 fetch_page 工具抓取用户提供的登录页或 API 文档地址（可以多次调用、可以跟踪\
     其中出现的其他相关链接），根据真实页面/文档内容判断该网站实际支持的登录方式：\n\
     - 如果同时支持多种方式（例如既有账号密码表单登录接口，也有 API key/token 认证接口），\
       优先选择 token/API key 方式，其次才是 cookie 值，账号密码登录的优先级最低——因为 token/API key \
       通常更稳定、不易触发人机验证或风控。\n\
     - 只有明确判断该网站只支持账号密码登录，才应该输出账号+密码两个字段。\n\
     - 只有明确判断该网站是纯 cookie 认证（例如需要用户从浏览器手动复制一个 session cookie 值）时，\
       才输出一个 cookie 字段。\n\n\
     最终只输出一个 JSON 数组本身（不要用 markdown 代码块包裹，不要任何解释性文字），数组每一项形如 \
     {\"name\": \"字段标识符（英文小写下划线命名，如 api_key/account/secret/cookie）\", \
     \"description\": \"给用户看的字段说明（用中文）\", \"required\": true}。字段数量应该精简，\
     只包含真正需要用户填写的凭据本身，不要包含额外的可选配置项。".to_string()
}

fn user_prompt(req: &AnalyzeLoginRequest) -> String {
    format!(
        "登录页或 API 文档地址（请先用 fetch_page 抓取查看真实内容）：{}",
        req.reference_url
    )
}

fn parse_parameters(content: &str) -> Option<Vec<LoginParameter>> {
    let parsed: Vec<LoginParameter> = serde_json::from_str(content).ok()?;
    if parsed.is_empty() || parsed.iter().any(|p| p.name.trim().is_empty()) {
        return None;
    }
    Some(parsed)
}

async fn run_analysis(
    state: &AppState,
    req: &AnalyzeLoginRequest,
) -> Result<Vec<LoginParameter>, AnalyzeError> {
    let mut messages = vec![
        Message::system(system_prompt()),
        Message::user(user_prompt(req)),
    ];
    let tools = vec![fetch_page_tool()];

    for _ in 0..MAX_LOOP_ROUNDS {
        // `false`: this round still carries `tools` (non-empty), and DeepSeek documents
        // `response_format: json_object` and `tools` as mutually exclusive on the same request —
        // the hard JSON guarantee only kicks in once the model has stopped requesting fetch_page
        // (see the reformat fallback below).
        let response = tool_chat(&state.redis.config, &messages, &tools, 0.2, 1000, false)
            .await
            .map_err(AnalyzeError::LlmUnavailable)?;

        match response {
            ToolChatResponse::Content(content) => {
                if let Some(parameters) = parse_parameters(&content) {
                    return Ok(parameters);
                }
                // The model gave its final answer but didn't format it as the required JSON
                // array (a real, observed failure mode — free-text prose instead of JSON despite
                // the system prompt's own instruction). Rather than failing outright, ask it to
                // reformat its own just-given answer as strict JSON, this time with a real
                // `response_format: json_object` guarantee (`tools` omitted for this one
                // follow-up call, since json_object mode requires that). This re-asks the model
                // to restate its own prior judgment in the required shape, not to re-derive it —
                // low risk of semantic drift since no new information enters the conversation.
                messages.push(Message::assistant(content));
                messages.push(Message::user(REFORMAT_AS_JSON_PROMPT.to_string()));
                let reformatted = tool_chat(&state.redis.config, &messages, &[], 0.0, 1000, true)
                    .await
                    .map_err(AnalyzeError::LlmUnavailable)?;
                return match reformatted {
                    ToolChatResponse::Content(json_content) => parse_parameters(&json_content)
                        .ok_or(AnalyzeError::AiOutputNotParameters(json_content)),
                    ToolChatResponse::ToolCalls(_) => {
                        // Shouldn't happen with `tools: []`, but the type system doesn't rule it
                        // out — treat as the same failure shape rather than panicking.
                        Err(AnalyzeError::AiOutputNotParameters(
                            "AI unexpectedly requested a tool call during the JSON-only reformat step".to_string(),
                        ))
                    }
                };
            }
            ToolChatResponse::ToolCalls(calls) => {
                messages.push(Message::assistant_tool_calls(calls.clone()));
                for call in calls {
                    let url = extract_url_arg(&call).unwrap_or_else(|| req.reference_url.clone());
                    // No credential exists yet at this stage — this call's whole job is
                    // *determining* what credential fields the site needs, so there's nothing to
                    // substitute into an AI-composed `headers` placeholder even if it asked for
                    // one.
                    let tool_result =
                        execute_fetch_tool(&url, &std::collections::HashMap::new()).await;
                    messages.push(Message::tool_result(call.id.clone(), tool_result));
                }
            }
        }
    }

    Err(AnalyzeError::LlmUnavailable(
        "分析未能在合理轮数内收敛，请重试".to_string(),
    ))
}

const REFORMAT_AS_JSON_PROMPT: &str =
    "请把你刚才的判断结果重新整理成严格的 JSON 数组格式本身，不要\
    markdown 代码块包裹，不要任何解释性文字，只输出这一个 JSON 数组，格式与之前的要求完全一致。";
