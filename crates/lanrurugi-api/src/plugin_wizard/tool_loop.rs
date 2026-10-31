//! Shared `fetch_page` tool plumbing for the two AI call sites that need it — `generate.rs`'s
//! code-generation loop and `analyze_login.rs`'s login-mechanism analysis. Both are the same
//! "system executes — AI judges — proposes the next request" agentic loop, just with a different
//! prompt and final shape.
//!
//! Every fetch goes through [`FetchSession`]: the outbound policy (`fetch.rs`), bounded results
//! with a continuation for the rest (FR-016…FR-019), an accumulated budget per action (FR-018),
//! and an untrusted-data frame around page content (FR-030).

use std::collections::HashMap;

use lanrurugi_llm::{Tool, ToolCall};
use serde_json::json;

use super::fetch::{fetch_with_policy, FetchError, FetchPolicy, Refusal};

/// Most page content one tool result hands the model; the rest stays behind a continuation.
pub(super) const RESULT_CONTENT_CAP: usize = 48 * 1024;

/// Total page content one action may hand the model across all rounds. Five individually
/// acceptable results are not acceptable together.
pub(super) const ACTION_CONTENT_BUDGET: usize = 240 * 1024;

use crate::llm_prompts::{
    fetch_results, FETCH_PAGE_CONTINUATION_DESCRIPTION, FETCH_PAGE_DESCRIPTION,
    FETCH_PAGE_HEADERS_DESCRIPTION, UNTRUSTED_FRAME_CLOSE as FRAME_CLOSE,
    UNTRUSTED_FRAME_OPEN as FRAME_OPEN,
};

pub(super) fn fetch_page_tool() -> Tool {
    Tool::function(
        "fetch_page",
        FETCH_PAGE_DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "headers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": FETCH_PAGE_HEADERS_DESCRIPTION,
                },
                "continuation": {
                    "type": "string",
                    "description": FETCH_PAGE_CONTINUATION_DESCRIPTION,
                },
            },
        }),
    )
}

pub(super) fn extract_url_arg(call: &ToolCall) -> Option<String> {
    let args: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    args["url"].as_str().map(|s| s.to_string())
}

fn extract_continuation_arg(call: &ToolCall) -> Option<String> {
    let args: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    args["continuation"].as_str().map(|s| s.to_string())
}

/// The raw, unsubstituted `headers` object from the tool call — each value may still contain a
/// `{{name}}` placeholder, substituted only by the fetch policy for a hop allowed to carry it.
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

/// A page held for continuation reads.
struct StoredPage {
    url: String,
    body: String,
    /// Bytes the source sent (a lower bound when the read itself was capped).
    total_bytes: usize,
    read_capped: bool,
}

/// One action's fetches: the policy, the credential values the policy may substitute, held
/// pages for continuations, and the content handed to the model so far.
pub(super) struct FetchSession {
    policy: FetchPolicy,
    credential_values: HashMap<String, String>,
    pages: Vec<StoredPage>,
    content_returned: usize,
}

/// One executed `fetch_page` call: the JSON the model sees, plus its `status`.
pub(super) struct ToolOutcome {
    pub result: String,
    pub status: String,
}

impl FetchSession {
    pub(super) fn new(policy: FetchPolicy, credential_values: HashMap<String, String>) -> Self {
        Self {
            policy,
            credential_values,
            pages: Vec::new(),
            content_returned: 0,
        }
    }

    /// Whether the accumulated budget is spent (FR-018).
    pub(super) fn budget_exhausted(&self) -> bool {
        self.content_returned >= ACTION_CONTENT_BUDGET
    }

    /// Executes one `fetch_page` call; `default_url` is used when the call names neither a URL
    /// nor a continuation.
    pub(super) async fn execute(
        &mut self,
        call: &ToolCall,
        default_url: Option<String>,
    ) -> ToolOutcome {
        if let Some(handle) = extract_continuation_arg(call) {
            return self.continue_page(&handle);
        }
        let Some(url) = extract_url_arg(call).or(default_url) else {
            return outcome(json!({ "status": "error", "error": fetch_results::no_url() }));
        };
        let headers = extract_headers_arg(call);
        tracing::info!(url, "plugin wizard: AI requested fetch_page");
        match fetch_with_policy(&url, &headers, &self.credential_values, &self.policy).await {
            Ok(result) => {
                let mut extra = serde_json::Map::new();
                extra.insert(
                    "redirect_trail".into(),
                    json!(result
                        .redirect_trail
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()),
                );
                extra.insert("final_url".into(), json!(result.final_url.to_string()));
                extra.insert("http_status".into(), json!(result.status.as_u16()));
                if let Some(hop) = result.withheld_credentials_from_hop {
                    extra.insert("withheld_credentials_from_hop".into(), json!(hop));
                }
                let page = StoredPage {
                    url: url.clone(),
                    body: result.body,
                    total_bytes: result.total_bytes,
                    read_capped: result.body_truncated,
                };
                let base_status = if result.cloudflare_challenge {
                    "cf_challenge"
                } else {
                    "ok"
                };
                self.pages.push(page);
                let index = self.pages.len() - 1;
                self.page_result(index, 0, &url, base_status, extra)
            }
            Err(FetchError::RedirectCapExceeded { trail }) => outcome(json!({
                "requested_url": url,
                "redirect_trail": trail.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "status": "redirect_cap_exceeded",
            })),
            Err(FetchError::Refused { refusal, trail }) => {
                tracing::warn!(
                    url,
                    reason = refusal.reason(),
                    "plugin wizard: fetch_page refused by the outbound policy"
                );
                outcome(json!({
                    "requested_url": url,
                    "redirect_trail": trail.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "status": "refused",
                    "refusal_reason": refusal.reason(),
                    "detail": refusal_text(&refusal, self.policy.fetch_hosts()),
                }))
            }
            Err(FetchError::Request(msg)) => outcome(json!({
                "requested_url": url,
                "status": "error",
                "error": msg,
            })),
        }
    }

    /// `c<page>:<offset>` — re-checked against the policy so a handle cannot reach a host the
    /// policy refuses.
    fn continue_page(&mut self, handle: &str) -> ToolOutcome {
        let parsed = handle
            .strip_prefix('c')
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(p, o)| Some((p.parse::<usize>().ok()?, o.parse::<usize>().ok()?)));
        let Some((index, offset)) = parsed.filter(|(i, _)| *i < self.pages.len()) else {
            return outcome(json!({
                "status": "error",
                "error": fetch_results::bad_continuation(handle),
            }));
        };
        let url = self.pages[index].url.clone();
        let allowed = super::fetch::host_of(&url).is_some_and(|h| self.policy.allows_fetch(&h));
        if !allowed {
            return outcome(json!({
                "requested_url": url,
                "status": "refused",
                "refusal_reason": "host_not_permitted",
                "detail": refusal_text(
                    &Refusal::HostNotPermitted { host: super::fetch::host_of(&url).unwrap_or_default() },
                    self.policy.fetch_hosts(),
                ),
            }));
        }
        self.page_result(index, offset, &url, "ok", serde_json::Map::new())
    }

    /// Bounds one page's content from `offset`, frames it, and charges it to the budget.
    fn page_result(
        &mut self,
        index: usize,
        offset: usize,
        url: &str,
        base_status: &str,
        mut fields: serde_json::Map<String, serde_json::Value>,
    ) -> ToolOutcome {
        let page = &self.pages[index];
        let start = floor_char_boundary(&page.body, offset.min(page.body.len()));
        let end = floor_char_boundary(
            &page.body,
            (start + RESULT_CONTENT_CAP).min(page.body.len()),
        );
        let slice = &page.body[start..end];
        let more_in_body = end < page.body.len();
        let truncated = start > 0 || more_in_body || page.read_capped;

        fields.insert("requested_url".into(), json!(url));
        fields.insert(
            "status".into(),
            json!(if truncated { "truncated" } else { base_status }),
        );
        fields.insert(
            "content".into(),
            json!(format!("{FRAME_OPEN}\n{slice}\n{FRAME_CLOSE}")),
        );
        if truncated {
            if base_status != "ok" {
                fields.insert("page_status".into(), json!(base_status));
            }
            fields.insert("total_bytes".into(), json!(page.total_bytes));
            fields.insert("returned_bytes".into(), json!(slice.len()));
            fields.insert("offset".into(), json!(start));
            let notice = if more_in_body {
                fields.insert("continuation".into(), json!(format!("c{index}:{end}")));
                fetch_results::truncated(start, end, page.total_bytes, page.read_capped)
            } else {
                fetch_results::read_cap_reached(page.total_bytes, page.read_capped)
            };
            fields.insert("truncation_notice".into(), json!(notice));
        }
        self.content_returned += slice.len();
        let status = fields["status"].as_str().unwrap_or("ok").to_string();
        ToolOutcome {
            result: serde_json::Value::Object(fields).to_string(),
            status,
        }
    }
}

fn outcome(value: serde_json::Value) -> ToolOutcome {
    let status = value["status"].as_str().unwrap_or("error").to_string();
    ToolOutcome {
        result: value.to_string(),
        status,
    }
}

/// Model-legible refusal: which rule refused it and what would have been permitted. Never names
/// a credential value or a resolved address.
fn refusal_text(refusal: &Refusal, fetch_hosts: &[String]) -> String {
    let permitted = if fetch_hosts.is_empty() {
        fetch_results::no_permitted_sites()
    } else {
        fetch_hosts.join("、")
    };
    match refusal {
        Refusal::SchemeNotAllowed => fetch_results::refused_scheme(),
        Refusal::HostNotPermitted { host } => fetch_results::refused_host(host, &permitted),
        Refusal::CredentialHostNotPermitted { host } => {
            fetch_results::refused_credential_host(host)
        }
        Refusal::PrivateAddress { host } => fetch_results::refused_private(host),
        Refusal::UnresolvableHost { host } => fetch_results::refused_unresolvable(host),
        Refusal::UnknownCredentialReference { name } => {
            fetch_results::refused_unknown_credential(name)
        }
        Refusal::RedirectLeftPermittedSet { host } => {
            fetch_results::refused_redirect(host, &permitted)
        }
    }
}

/// The largest index `<= at` on a UTF-8 boundary.
fn floor_char_boundary(s: &str, mut at: usize) -> usize {
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            function: lanrurugi_llm::ToolCallFunction {
                name: "fetch_page".into(),
                arguments: args.to_string(),
            },
        }
    }

    fn session_with_page(body: String, read_capped: bool) -> FetchSession {
        let mut s = FetchSession::new(
            FetchPolicy::new(Some("site-a.example"), [], Vec::<String>::new()),
            HashMap::new(),
        );
        s.pages.push(StoredPage {
            url: "https://site-a.example/x".into(),
            total_bytes: body.len() + usize::from(read_capped),
            body,
            read_capped,
        });
        s
    }

    fn parsed(o: &ToolOutcome) -> serde_json::Value {
        serde_json::from_str(&o.result).unwrap()
    }

    #[test]
    fn a_page_under_the_cap_is_whole_and_framed() {
        let mut s = session_with_page("<html>small</html>".into(), false);
        let o = s.page_result(
            0,
            0,
            "https://site-a.example/x",
            "ok",
            serde_json::Map::new(),
        );
        let v = parsed(&o);
        assert_eq!(v["status"], "ok");
        assert_eq!(
            v["content"],
            format!("{FRAME_OPEN}\n<html>small</html>\n{FRAME_CLOSE}")
        );
        assert!(v.get("continuation").is_none());
    }

    #[test]
    fn a_large_page_truncates_with_a_continuation_that_reads_on() {
        let body = "a".repeat(RESULT_CONTENT_CAP * 2 + 100);
        let mut s = session_with_page(body, false);
        let first = parsed(&s.page_result(0, 0, "u", "ok", serde_json::Map::new()));
        assert_eq!(first["status"], "truncated");
        assert_eq!(first["returned_bytes"], RESULT_CONTENT_CAP);
        assert_eq!(first["total_bytes"], RESULT_CONTENT_CAP * 2 + 100);
        let handle = first["continuation"].as_str().unwrap().to_string();

        let next = parsed(&s.execute_sync_continuation(&handle));
        assert_eq!(next["offset"], RESULT_CONTENT_CAP);
        assert_eq!(next["returned_bytes"], RESULT_CONTENT_CAP);
        assert!(next["continuation"].is_string());
    }

    #[test]
    fn the_accumulated_budget_is_reached_by_several_large_results() {
        let body = "b".repeat(RESULT_CONTENT_CAP * 10);
        let mut s = session_with_page(body, false);
        let mut rounds = 0;
        while !s.budget_exhausted() {
            s.page_result(
                0,
                rounds * RESULT_CONTENT_CAP,
                "u",
                "ok",
                serde_json::Map::new(),
            );
            rounds += 1;
        }
        assert_eq!(rounds, ACTION_CONTENT_BUDGET / RESULT_CONTENT_CAP);
    }

    #[test]
    fn an_unknown_continuation_is_an_error_not_a_panic() {
        let mut s = session_with_page("x".into(), false);
        let v = parsed(&s.execute_sync_continuation("c9:0"));
        assert_eq!(v["status"], "error");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let body = "页".repeat(RESULT_CONTENT_CAP);
        let mut s = session_with_page(body, false);
        let v = parsed(&s.page_result(0, 0, "u", "ok", serde_json::Map::new()));
        let content = v["content"].as_str().unwrap();
        assert!(content.starts_with(FRAME_OPEN));
    }

    #[test]
    fn continuation_argument_is_recognised() {
        assert_eq!(
            extract_continuation_arg(&call(json!({ "continuation": "c0:10" }))),
            Some("c0:10".into())
        );
    }

    impl FetchSession {
        fn execute_sync_continuation(&mut self, handle: &str) -> ToolOutcome {
            self.continue_page(handle)
        }
    }
}
