//! Quickstart S2 (`specs/014-ai-harness-hardening`): every model call is bounded, retries at most
//! once and only for a retryable class, names its failure, and stops when cancelled. A local stub
//! stands in for the provider; each test picks the stub's behaviour through its system prompt, and
//! the stub counts the requests each scenario received.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lanrurugi_llm::{
    CallSpec, Conversation, FeatureId, LlmClient, LlmErrorClass, Message, StreamEvent,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Stub {
    hits: Arc<Mutex<HashMap<String, usize>>>,
    bodies: Arc<Mutex<HashMap<String, Vec<Value>>>>,
}

impl Stub {
    fn hits(&self, scenario: &str) -> usize {
        self.hits
            .lock()
            .unwrap()
            .get(scenario)
            .copied()
            .unwrap_or(0)
    }
}

fn ok_body(content: &str) -> Value {
    json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
    })
}

/// The scenario name is the system prompt.
fn scenario_of(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn chat(stub: Stub, body: Value) -> Response {
    let scenario = scenario_of(&body);
    let n = {
        let mut hits = stub.hits.lock().unwrap();
        let n = hits.entry(scenario.clone()).or_default();
        *n += 1;
        *n
    };
    stub.bodies
        .lock()
        .unwrap()
        .entry(scenario.clone())
        .or_default()
        .push(body.clone());
    let base = scenario.split(':').next().unwrap_or_default();
    match base {
        "ok" => Json(ok_body("{\"a\":1}")).into_response(),
        "stall" => {
            tokio::time::sleep(Duration::from_secs(600)).await;
            StatusCode::OK.into_response()
        }
        "flaky" if n == 1 => (StatusCode::INTERNAL_SERVER_ERROR, "{}").into_response(),
        "flaky" => Json(ok_body("{\"a\":1}")).into_response(),
        "auth" => (
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"bad key"}}"#,
        )
            .into_response(),
        "balance" => (StatusCode::PAYMENT_REQUIRED, "{}").into_response(),
        "limited" if n == 1 => {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", "1".parse().unwrap());
            (StatusCode::TOO_MANY_REQUESTS, headers, "{}").into_response()
        }
        "limited" => Json(ok_body("{\"a\":1}")).into_response(),
        "limited_long" => {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", "120".parse().unwrap());
            (StatusCode::TOO_MANY_REQUESTS, headers, "{}").into_response()
        }
        "bad_request" => (StatusCode::BAD_REQUEST, "{}").into_response(),
        // Reasoning spends the whole budget: empty content cut off at the limit.
        "exhausted" if n == 1 => Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "length" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 200, "total_tokens": 210 }
        }))
        .into_response(),
        "exhausted" => Json(ok_body("{\"a\":1}")).into_response(),
        // First stream ends without a finish_reason (truncated); the retry finishes properly.
        "truncated" => {
            let mut sse =
                String::from("data: {\"choices\":[{\"delta\":{\"content\":\"part\"}}]}\n\n");
            if n > 1 {
                sse.push_str("data: {\"choices\":[{\"delta\":{\"content\":\" done\"},\"finish_reason\":\"stop\"}]}\n\n");
            }
            sse.push_str("data: [DONE]\n\n");
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from(sse))
                .unwrap()
        }
        _ => (StatusCode::NOT_FOUND, "{}").into_response(),
    }
}

/// One stub for the whole binary: the base URL is process-global.
fn stub() -> &'static Stub {
    static STUB: OnceLock<Stub> = OnceLock::new();
    STUB.get_or_init(|| {
        let stub = Stub::default();
        let for_server = stub.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let app = Router::new()
                    .route(
                        "/chat/completions",
                        post(move |Json(body): Json<Value>| chat(for_server.clone(), body)),
                    )
                    // No balance endpoint on a custom base URL: 404 means "not supported".
                    .route("/user/balance", get(|| async { StatusCode::NOT_FOUND }));
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        // Set once, before any test reads them, from the first `stub()` call.
        std::env::set_var("LANRURUGI_DEEPSEEK_BASE_URL", format!("http://{addr}"));
        std::env::set_var("DEEPSEEK_API_KEY", "test-key");
        stub
    })
}

/// A pool whose server does not exist: the key falls back to the env var and the ledger write
/// fails quietly, which is exactly the "observe, never interfere" behaviour under test.
fn pool() -> deadpool_redis::Pool {
    deadpool_redis::Config::from_url("redis://127.0.0.1:9/")
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap()
}

fn spec(feature: FeatureId, deadline_ms: u64) -> CallSpec {
    CallSpec::new(feature).with_deadline(Duration::from_millis(deadline_ms))
}

#[tokio::test]
async fn s2_a_stalled_provider_times_out_by_the_bound_for_every_feature() {
    let stub = stub();
    for feature in FeatureId::ALL
        .iter()
        .filter(|f| **f != FeatureId::TRANSLATION)
    {
        let scenario = format!("stall:{}", feature.as_str());
        let started = Instant::now();
        let err = pool()
            .chat(&spec(*feature, 300), &scenario, "u", 0.0, 10)
            .await
            .unwrap_err();
        assert_eq!(err.class(), LlmErrorClass::Timeout, "{}", feature.as_str());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{}",
            feature.as_str()
        );
        assert_eq!(
            stub.hits(&scenario),
            1,
            "{} retried a timeout",
            feature.as_str()
        );
    }
}

#[tokio::test]
async fn s2_a_transient_failure_is_retried_exactly_once() {
    let stub = stub();
    let out: Value = pool()
        .json_chat(
            &spec(FeatureId::ARCHIVE_SPLIT, 10_000),
            "flaky",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(out["a"], 1);
    assert_eq!(stub.hits("flaky"), 2);
}

#[tokio::test]
async fn s2_auth_and_balance_are_named_and_never_retried() {
    let stub = stub();
    let auth = pool()
        .chat(
            &spec(FeatureId::TANKOUBON_RENAME, 10_000),
            "auth",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(auth.class(), LlmErrorClass::Auth);
    assert_eq!(auth.message(), "LLM: API Key 无效 (401) — bad key");
    assert_eq!(stub.hits("auth"), 1);

    let balance = pool()
        .chat(
            &spec(FeatureId::TANKOUBON_RENAME, 10_000),
            "balance",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(balance.class(), LlmErrorClass::Balance);
    assert_eq!(stub.hits("balance"), 1);
}

#[tokio::test]
async fn s2_a_rate_limit_hint_is_honoured_once_within_the_deadline() {
    let stub = stub();
    let out = pool()
        .chat(
            &spec(FeatureId::RECOMMEND_RERANK, 10_000),
            "limited",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(out, "{\"a\":1}");
    assert_eq!(stub.hits("limited"), 2);

    // A hint longer than what is left of the deadline fails now rather than sleeping past it.
    let started = Instant::now();
    let err = pool()
        .chat(
            &spec(FeatureId::RECOMMEND_RERANK, 5_000),
            "limited_long",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(err.class(), LlmErrorClass::RateLimited);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(stub.hits("limited_long"), 1);
}

#[tokio::test]
async fn s2_other_client_errors_are_malformed_and_not_retried() {
    let stub = stub();
    let err = pool()
        .chat(
            &spec(FeatureId::PROVIDER_PRICING, 10_000),
            "bad_request",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(err.class(), LlmErrorClass::Malformed);
    assert_eq!(stub.hits("bad_request"), 1);
}

#[tokio::test]
async fn s2_a_truncated_stream_is_announced_and_retried_without_thinking() {
    let stub = stub();
    let events = Mutex::new(Vec::new());
    let out = pool()
        .tool_chat_streaming(
            &spec(FeatureId::PLUGIN_GENERATION, 10_000),
            &[Message::system("truncated"), Message::user("u")],
            &[],
            0.0,
            10,
            false,
            |event| {
                events.lock().unwrap().push(match event {
                    StreamEvent::ContentDelta(t) => t.to_string(),
                    StreamEvent::Retrying => "<retrying>".to_string(),
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        lanrurugi_llm::ToolChatResponse::Content("part done".into())
    );
    assert_eq!(stub.hits("truncated"), 2);
    // FR-031: the retry is announced before any of the retried round's output.
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["part", "<retrying>", "part", " done"]
    );
    let bodies = stub.bodies.lock().unwrap();
    assert_eq!(bodies["truncated"][0]["thinking"]["type"], "enabled");
    assert_eq!(bodies["truncated"][1]["thinking"]["type"], "disabled");
}

#[tokio::test]
async fn s2_cancellation_stops_the_call_and_issues_nothing_further() {
    let stub = stub();
    let cancel = CancellationToken::new();
    let spec = spec(FeatureId::PLUGIN_GENERATION, 60_000).with_cancel(cancel.clone());
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let err = pool()
        .chat(&spec, "stall:cancel", "u", 0.0, 10)
        .await
        .unwrap_err();
    assert_eq!(err.class(), LlmErrorClass::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stub.hits("stall:cancel"), 1);
}

#[tokio::test]
async fn s2_success_reports_through_the_same_seam() {
    let stub = stub();
    let out = pool()
        .chat(
            &spec(FeatureId::SUBSCRIPTION_CONDITION, 10_000),
            "ok",
            "u",
            0.0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(out, "{\"a\":1}");
    assert_eq!(stub.hits("ok"), 1);
}

/// SC-008 (T042): the request bodies the provider receives are exactly the pre-seam shapes — the
/// seam changed failure handling and measurement, never what is sent.
#[tokio::test]
async fn s7_request_bodies_are_unchanged() {
    let stub = stub();
    let _ = pool()
        .chat(
            &spec(FeatureId::ARTIST_BACKFILL, 10_000),
            "ok:body",
            "the user",
            0.2,
            200,
        )
        .await;
    let body = stub.bodies.lock().unwrap()["ok:body"][0].clone();
    assert_eq!(
        body,
        json!({
            "model": lanrurugi_llm::DEEPSEEK_MODEL,
            "messages": [
                { "role": "system", "content": "ok:body" },
                { "role": "user", "content": "the user" },
            ],
            "temperature": 0.2f32,
            "max_tokens": 200,
            "response_format": { "type": "json_object" },
        })
    );

    let _ = pool()
        .tool_chat(
            &spec(FeatureId::PLUGIN_LOGIN_ANALYSIS, 10_000),
            &[Message::system("ok:tool"), Message::user("u")],
            &[],
            0.0,
            1000,
            true,
        )
        .await;
    let body = stub.bodies.lock().unwrap()["ok:tool"][0].clone();
    assert_eq!(
        body,
        json!({
            "model": lanrurugi_llm::DEEPSEEK_MODEL,
            "messages": [
                { "role": "system", "content": "ok:tool" },
                { "role": "user", "content": "u" },
            ],
            "temperature": 0.0,
            "max_tokens": 1000,
            "thinking": { "type": "enabled", "reasoning_effort": "low" },
            "response_format": { "type": "json_object" },
        })
    );
}

/// Prefix stability: each round of a conversation must extend the previous request, or the
/// provider's prompt cache misses from the changed message on. Appending passes; rewriting an
/// earlier message is flagged (and still sent — the round is correct, only more expensive).
#[tokio::test]
async fn conversation_rounds_must_extend_the_previous_request() {
    stub();
    let conversation = Conversation::new();
    let spec = spec(FeatureId::PLUGIN_GENERATION, 10_000).in_conversation(conversation.clone());
    let mut messages = vec![Message::system("ok:conversation"), Message::user("ask")];
    pool()
        .tool_chat(&spec, &messages, &[], 0.0, 10, false)
        .await
        .unwrap();
    messages.push(Message::assistant("answer"));
    messages.push(Message::user("follow-up"));
    pool()
        .tool_chat(&spec, &messages, &[], 0.0, 10, false)
        .await
        .unwrap();
    assert_eq!(conversation.breaks(), 0);

    messages[1] = Message::user("ask, rewritten");
    pool()
        .tool_chat(&spec, &messages, &[], 0.0, 10, false)
        .await
        .unwrap();
    assert_eq!(conversation.breaks(), 1);
}

/// A small-budget JSON call whose reasoning used every output token comes back empty and cut off
/// at the limit. That is a truncation, retried once with thinking off — not unparseable output.
#[tokio::test]
async fn an_answer_lost_to_reasoning_is_retried_once_without_thinking() {
    let stub = stub();
    let out: Value = pool()
        .json_chat(
            &spec(FeatureId::ARTIST_BACKFILL, 10_000),
            "exhausted",
            "u",
            0.2,
            200,
        )
        .await
        .unwrap();
    assert_eq!(out["a"], 1);
    assert_eq!(stub.hits("exhausted"), 2);
    let bodies = stub.bodies.lock().unwrap();
    let sent = &bodies["exhausted"];
    assert!(
        sent[0].get("thinking").is_none(),
        "the first attempt is unchanged"
    );
    assert_eq!(sent[1]["thinking"]["type"], "disabled");
}
