//! Shared `fetch_page` tool plumbing for the two AI call sites that need it — `generate.rs`'s
//! code-generation loop and `analyze_login.rs`'s login-mechanism analysis. Both are the same
//! "system executes — AI judges — proposes the next request" agentic loop (FR-010), just with a
//! different system/user prompt and a different expected final shape (plugin code vs. a
//! `parameters` array).

use std::collections::HashMap;

use lanrurugi_llm::{Tool, ToolCall};
use serde_json::json;

use super::fetch::{fetch_with_redirect_trail, FetchError};

pub(super) fn fetch_page_tool() -> Tool {
    Tool::function(
        "fetch_page",
        "Fetch a URL and return its final page content plus the full redirect trail followed to get there. \
         Optionally pass `headers` (e.g. for an endpoint that requires authentication) — reference a \
         credential by its declared field name wrapped in {{double braces}} (e.g. \
         `{\"Authorization\": \"Key {{token}}\"}`) rather than a literal value; the system substitutes the \
         real value server-side and you will never see it, only this fetch's resulting page content.",
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "headers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Optional extra request headers, e.g. {\"Authorization\": \"Key {{token}}\"} using a {{credential_field_name}} placeholder.",
                },
            },
            "required": ["url"]
        }),
    )
}

pub(super) fn extract_url_arg(call: &ToolCall) -> Option<String> {
    let args: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    args["url"].as_str().map(|s| s.to_string())
}

/// The raw, unsubstituted `headers` object from the tool call, if present — each value may still
/// contain a `{{name}}` placeholder the caller (`generate.rs`) is responsible for substituting via
/// [`substitute_credential_placeholders`] before this ever reaches a real HTTP request.
pub(super) fn extract_headers_arg(call: &ToolCall) -> HashMap<String, String> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.function.arguments) else {
        return HashMap::new();
    };
    let Some(headers) = args["headers"].as_object() else {
        return HashMap::new();
    };
    headers
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect()
}

/// Replaces every `{{name}}` occurrence in each header value with `credential_values[name]` — a
/// name with no matching credential is left as a literal, unsubstituted `{{name}}` (rather than
/// silently emptied), which surfaces as an obviously-wrong header value in the fetch result AI
/// itself sees, instead of a silently-broken auth attempt that looks identical to a real one. The
/// model itself never sees `credential_values`' real values — only whichever page content came
/// back once the system substituted them in, matching FR-012's "credential values never reach the
/// LLM" requirement (the substituted *header* never becomes part of any message sent back to the
/// model either — only the fetch's response body does).
pub(super) fn substitute_credential_placeholders(
    headers: HashMap<String, String>,
    credential_values: &HashMap<String, String>,
) -> HashMap<String, String> {
    headers
        .into_iter()
        .map(|(name, value)| {
            let mut substituted = value;
            for (cred_name, cred_value) in credential_values {
                substituted = substituted.replace(&format!("{{{{{cred_name}}}}}"), cred_value);
            }
            (name, substituted)
        })
        .collect()
}

pub(super) async fn execute_fetch_tool(url: &str, headers: &HashMap<String, String>) -> String {
    tracing::info!(url, "plugin wizard: AI requested fetch_page");
    match fetch_with_redirect_trail(url, headers).await {
        Ok(result) => json!({
            "requested_url": url,
            "redirect_trail": result.redirect_trail.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "final_url": result.final_url.to_string(),
            // Previously always "ok" regardless of content — a Cloudflare challenge page (a real,
            // previously invisible failure mode) looked identical to a genuine successful fetch to
            // the model, which had no way to tell it was reading Cloudflare's own interstitial
            // markup instead of the target site's real page structure. See
            // `fetch.rs::looks_like_cloudflare_challenge`'s own docs.
            "status": if result.cloudflare_challenge { "cf_challenge" } else { "ok" },
            "http_status": result.status.as_u16(),
            "content": result.body,
        })
        .to_string(),
        Err(FetchError::RedirectCapExceeded { trail }) => json!({
            "requested_url": url,
            "redirect_trail": trail.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "status": "redirect_cap_exceeded",
        })
        .to_string(),
        Err(FetchError::Request(msg)) => json!({
            "requested_url": url,
            "status": "error",
            "error": msg,
        })
        .to_string(),
    }
}
