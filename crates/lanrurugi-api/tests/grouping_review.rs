//! `specs/016-tankoubon-grouping-harness` T016 / quickstart Q2: the grouping reviewer can only
//! remove members, repairs once, and falls back to the local result. A local stub stands in for
//! the provider; each test picks the stub's behaviour through a marker in a member title, and the
//! stub counts the requests each scenario received.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use lanrurugi_api::tankoubon_grouping::review::{
    review_group, MemberFacts, ReviewHints, ReviewStatus,
};
use lanrurugi_api::tankoubon_grouping::GroupingSettings;
use lanrurugi_core::ids::GroupSuggestionId;
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Stub {
    hits: Arc<Mutex<HashMap<String, usize>>>,
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

fn answer(verdicts: Value) -> Value {
    json!({
        "choices": [{ "message": { "role": "assistant", "content": json!({ "verdicts": verdicts }).to_string() } }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
    })
}

/// The scenario is the `scenario-<name>` marker in the first user message.
fn scenario_of(body: &Value) -> String {
    let user = body["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default();
    user.split("scenario-")
        .nth(1)
        .and_then(|rest| {
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
        })
        .unwrap_or_default()
        .to_string()
}

async fn chat(stub: Stub, body: Value) -> axum::response::Response {
    let scenario = scenario_of(&body);
    let n = {
        let mut hits = stub.hits.lock().unwrap();
        let n = hits.entry(scenario.clone()).or_default();
        *n += 1;
        *n
    };
    let good = json!([
        { "id": "m1", "keep": true, "reason": "same story" },
        { "id": "m2", "keep": false, "reason": "different work" },
    ]);
    let missing = json!([{ "id": "m1", "keep": true, "reason": "" }]);
    let intruder = json!([
        { "id": "m1", "keep": true, "reason": "" },
        { "id": "m2", "keep": true, "reason": "" },
        { "id": "m3", "keep": true, "reason": "an archive outside the group" },
    ]);
    let body = match (scenario.split('_').next().unwrap_or_default(), n) {
        ("reviewed", _) | ("cached", _) => answer(good),
        ("repaired", 1) => answer(missing),
        ("repaired", _) => answer(good),
        ("intruder", _) => answer(intruder),
        _ => return (StatusCode::NOT_FOUND, "{}").into_response(),
    };
    Json(body).into_response()
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

/// A pool whose server does not exist: no cache hit, cache writes fail quietly.
fn no_redis() -> deadpool_redis::Pool {
    deadpool_redis::Config::from_url("redis://127.0.0.1:9/")
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap()
}

/// Two members with different (unsettled) creators, the scenario marker in the first title.
fn members(scenario: &str) -> Vec<MemberFacts> {
    vec![
        MemberFacts {
            id: "arc-1".into(),
            title: format!("scenario-{scenario} Story"),
            tags: vec!["parody:p".into()],
        },
        MemberFacts {
            id: "arc-2".into(),
            title: "Another Story".into(),
            tags: vec!["parody:p".into()],
        },
    ]
}

async fn run(
    pool: &deadpool_redis::Pool,
    id: &str,
    scenario: &str,
) -> lanrurugi_api::tankoubon_grouping::review::Review {
    review_group(
        pool,
        &GroupSuggestionId(id.into()),
        &members(scenario),
        &ReviewHints::default(),
        None,
        &GroupingSettings::default(),
        true,
    )
    .await
}

#[tokio::test]
async fn a_valid_answer_is_one_call_and_only_removes() {
    let stub = stub();
    let review = run(&no_redis(), "s-reviewed", "reviewed").await;
    assert_eq!(review.status, ReviewStatus::Reviewed);
    assert_eq!(stub.hits("reviewed"), 1);
    let members = vec!["arc-1".to_string(), "arc-2".to_string()];
    assert_eq!(review.kept(&members), vec!["arc-1".to_string()]);
    // SC-005: every verdict names a member of the group.
    assert!(review
        .verdicts
        .iter()
        .all(|v| members.contains(&v.archive_id)));
}

#[tokio::test]
async fn an_invalid_answer_is_repaired_once() {
    let stub = stub();
    let review = run(&no_redis(), "s-repaired", "repaired").await;
    assert_eq!(review.status, ReviewStatus::Repaired);
    assert_eq!(stub.hits("repaired"), 2);
}

#[tokio::test]
async fn an_archive_outside_the_group_falls_back_after_one_repair() {
    let stub = stub();
    let review = run(&no_redis(), "s-intruder", "intruder").await;
    assert_eq!(review.status, ReviewStatus::FellBack);
    assert_eq!(stub.hits("intruder"), 2);
    assert!(
        review.verdicts.is_empty(),
        "a fallback keeps the local result"
    );
}

#[tokio::test]
async fn a_shared_creator_settles_without_a_call() {
    let stub = stub();
    let mut settled = members("settled");
    for m in &mut settled {
        m.tags.push("artist:same".into());
    }
    let review = review_group(
        &no_redis(),
        &GroupSuggestionId("s-settled".into()),
        &settled,
        &ReviewHints::default(),
        None,
        &GroupingSettings::default(),
        true,
    )
    .await;
    assert_eq!(review.status, ReviewStatus::Settled);
    assert_eq!(stub.hits("settled"), 0);
}

#[tokio::test]
async fn a_cached_review_is_not_asked_again() {
    let stub = stub();
    let Some(redis) = lanrurugi_storage::test_support::test_redis_dbs().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let id = format!(
        "s-cached-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let first = run(&redis.config, &id, "cached").await;
    assert_eq!(first.status, ReviewStatus::Reviewed);
    let second = run(&redis.config, &id, "cached").await;
    assert_eq!(second, first);
    assert_eq!(stub.hits("cached"), 1);
}
