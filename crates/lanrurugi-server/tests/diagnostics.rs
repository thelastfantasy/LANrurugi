//! End-to-end coverage for the diagnostics feature (specs/018): capture to issue, bundle contents
//! and scrubbing, the forwarding window, and offline replay.
//!
//! The tracker is a real HTTP server on a loopback port, so "nothing was sent" is an observation
//! rather than a mock's opinion — which is the whole claim the window makes.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use lanrurugi_api::diagnostics::{bundle, forward, ingest, issue};
use lanrurugi_storage::diagnostics::{DiagnosticEvent, Fingerprint, Level, Source};

/// A tracker that records how many envelopes reached it.
async fn fake_tracker() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let received = Arc::new(AtomicUsize::new(0));
    let counter = received.clone();
    let app = Router::new().route(
        "/api/{project}/envelope/",
        post(move |body: axum::body::Bytes| {
            let counter = counter.clone();
            async move {
                if !body.is_empty() {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://key@{addr}/7"), received, handle)
}

fn envelope(id: &str, message: &str) -> Vec<u8> {
    let header = format!(r#"{{"event_id":"{id}"}}"#);
    let item = r#"{"type":"event"}"#;
    let payload = format!(
        r#"{{"event_id":"{id}","timestamp":1793156000.0,"level":"error","platform":"javascript","release":"abc123","message":"{message}","tags":{{"request_id":"7fa3"}}}}"#
    );
    format!("{header}\n{item}\n{payload}\n").into_bytes()
}

fn nonce() -> String {
    format!("{}", std::process::id())
}

fn event_from(id: &str, message: &str) -> DiagnosticEvent {
    let mut event = ingest::decode(
        &envelope(id, message),
        lanrurugi_api::diagnostics::scrub::Profile::WithPrivateData,
    )
    .expect("a decodable envelope")
    .expect("an event item");
    event.fingerprint = issue::fingerprint(&event);
    event
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capture_groups_repeats_and_survives_eviction() {
    let Some((state, _redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let tag = nonce();
    let first = event_from("e1", &format!("cannot decode archive: bad header {tag}"));
    let second = event_from("e2", &format!("cannot decode archive: bad header {tag}"));
    state.repos.diagnostics.bump_drop("rate_limit").await;
    for event in [&first, &second] {
        state.repos.diagnostics.save_event(event).await.unwrap();
    }
    // The cap is enforced by the eviction pass, exactly as the writer runs it after a store.
    state
        .repos
        .diagnostics
        .evict(
            lanrurugi_storage::diagnostics::Retention {
                days: 0,
                max_events: 1,
                max_bytes: 0,
            },
            lanrurugi_api::diagnostics::now_ms(),
        )
        .await
        .unwrap();

    let fingerprint = first.fingerprint.clone();
    let issue = state
        .repos
        .diagnostics
        .issue(&fingerprint)
        .await
        .unwrap()
        .expect("the issue exists");
    assert_eq!(issue.count, 2, "two occurrences are one issue");
    assert!(issue.title.starts_with("cannot decode archive: bad header"));
    assert_eq!(issue.request_id.as_deref(), Some("7fa3"));

    // The shared test Redis holds other suites' events too, so the cap is asserted on the whole
    // store: it must have shrunk to the cap (allowing for a concurrent suite's write), and this
    // issue must keep its history regardless.
    let usage = state.repos.diagnostics.usage().await.unwrap();
    assert!(
        usage.events <= 2,
        "the cap must be enforced, saw {}",
        usage.events
    );
    let events = state
        .repos
        .diagnostics
        .events_for_issue(&fingerprint, 10)
        .await
        .unwrap();
    assert!(
        events.len() <= 1,
        "at most one of this issue's events survives"
    );

    // The issue survives its events, which is what keeps the list's history readable.
    assert!(state
        .repos
        .diagnostics
        .issue(&fingerprint)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_carries_events_and_no_secrets() {
    let Some((state, _redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // A message containing a share code and an absolute path: both must be gone after storage.
    let event = event_from(
        "b1",
        &format!(
            "import of lrrg:VXR29-J5RNH?r=abcdef123456 failed at /home/jade/manga/x.zip {}",
            nonce()
        ),
    );
    assert!(!event.message.contains("VXR29-J5RNH"), "{}", event.message);
    state.repos.diagnostics.save_event(&event).await.unwrap();

    let selection = bundle::Selection {
        from: event.at - 3_600_000,
        to: event.at + 3_600_000,
        levels: vec!["error".into()],
        sources: vec![],
        issues: vec![],
        include_ids: vec![],
        exclude_ids: vec![],
        categories: bundle::Categories {
            server_logs: false,
            frontend_events: true,
            environment: true,
            config: false,
        },
        truncate: false,
        resolve_stacks: false,
    };
    let plan = bundle::plan(&state, &selection).await.expect("a plan");
    assert!(plan.summary.events >= 1);

    let facts = lanrurugi_api::diagnostics::text::Facts {
        version: "0.1.0".into(),
        release: "abc123".into(),
        engine: "v3".into(),
        platform: "linux/x86_64".into(),
        bundle_name: "lrrg-diagnostics-test.zip".into(),
    };
    let rendered = bundle::feedback(&plan, None, &facts);
    let bytes = bundle::archive(&plan, &selection, &facts, &rendered, &serde_json::json!({}))
        .expect("an archive");
    let entries = bundle::entries(&bytes).expect("readable entries");
    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "manifest.json",
        "README.txt",
        "feedback.zh.md",
        "feedback.en.md",
        "envelopes.ndjson.gz",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    assert!(
        !names.iter().any(|name| name.ends_with(".map")),
        "source maps must never be bundled: {names:?}"
    );
    let all: String = entries
        .iter()
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
        .collect();
    assert!(!all.contains("VXR29-J5RNH"), "the share code leaked");
    assert!(!all.contains("/home/jade"), "the path leaked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarding_happens_only_while_the_window_is_open() {
    let Some((state, _redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (dsn, received, tracker) = fake_tracker().await;
    // A previous run may have left a window open in this shared test Redis.
    forward::close_window(&state).await.expect("a clean window");

    // No endpoint configured: an open attempt is refused and nothing is sent.
    let event = event_from("f0", "before the endpoint");
    forward::maybe_forward(&state, &event).await;
    assert_eq!(received.load(Ordering::SeqCst), 0);

    let mut body = serde_json::Map::new();
    body.insert("tracker_dsn".into(), serde_json::json!(dsn));
    lanrurugi_api::diagnostics_settings::save(&state, &body)
        .await
        .expect("the DSN saves");

    // Configured but closed: still nothing leaves.
    let event = event_from("f1", "window closed");
    forward::maybe_forward(&state, &event).await;
    assert_eq!(
        received.load(Ordering::SeqCst),
        0,
        "closed window must not send"
    );

    let window = forward::open_window(&state, 300).await.expect("opens");
    assert!(window.open);

    let event = event_from("f2", "window open");
    forward::maybe_forward(&state, &event).await;
    assert_eq!(
        received.load(Ordering::SeqCst),
        1,
        "an open window forwards"
    );

    forward::close_window(&state).await.expect("closes");
    let event = event_from("f3", "after close");
    forward::maybe_forward(&state, &event).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        received.load(Ordering::SeqCst),
        1,
        "after closing, nothing more may be sent"
    );

    tracker.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_window_reads_as_closed_and_does_not_renew() {
    let Some((state, _redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let mut body = serde_json::Map::new();
    body.insert(
        "tracker_dsn".into(),
        serde_json::json!("http://key@127.0.0.1:9/7"),
    );
    lanrurugi_api::diagnostics_settings::save(&state, &body)
        .await
        .expect("the DSN saves");

    forward::close_window(&state).await.expect("a clean window");
    forward::open_window(&state, 300).await.expect("opens");
    // Move the deadline into the past, which is what a restart after expiry looks like.
    let mut conn = state.redis.config.get().await.unwrap();
    let _: () = deadpool_redis::redis::AsyncCommands::hset(
        &mut conn,
        lanrurugi_storage::keys::CONFIG_KEY,
        "diagnostics_window_deadline",
        "1",
    )
    .await
    .unwrap();

    let window = forward::window(&state).await;
    assert!(!window.open, "an expired window is closed");
    assert_eq!(window.remaining_secs, 0);

    // Reopening records a fresh deadline rather than reviving the old one.
    let reopened = forward::open_window(&state, 300).await.expect("reopens");
    assert!(reopened.deadline > window.deadline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_can_be_replayed_into_a_tracker() {
    let Some((state, _redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (dsn, received, tracker) = fake_tracker().await;
    forward::close_window(&state).await.expect("a clean window");
    let mut body = serde_json::Map::new();
    body.insert("tracker_dsn".into(), serde_json::json!(dsn));
    lanrurugi_api::diagnostics_settings::save(&state, &body)
        .await
        .expect("the DSN saves");

    let event = event_from("r1", &format!("collected offline {}", nonce()));
    state.repos.diagnostics.save_event(&event).await.unwrap();

    let selection = bundle::Selection {
        from: event.at - 3_600_000,
        to: event.at + 3_600_000,
        levels: vec![],
        sources: vec![],
        // Only this suite's event: the test Redis is shared with the other tests.
        issues: vec![event.fingerprint.0.clone()],
        include_ids: vec![],
        exclude_ids: vec![],
        categories: bundle::Categories::default(),
        truncate: false,
        resolve_stacks: false,
    };
    let plan = bundle::plan(&state, &selection).await.expect("a plan");
    let facts = lanrurugi_api::diagnostics::text::Facts {
        version: "0.1.0".into(),
        release: "abc".into(),
        engine: "v3".into(),
        platform: "linux/x86_64".into(),
        bundle_name: "b.zip".into(),
    };
    let rendered = bundle::feedback(&plan, None, &facts);
    let bytes = bundle::archive(&plan, &selection, &facts, &rendered, &serde_json::json!({}))
        .expect("an archive");

    let envelopes = bundle::envelopes_from_archive(&bytes).expect("envelopes");
    assert_eq!(envelopes.len(), 1, "the stored envelope is in the archive");
    let dsn_parts = forward::TrackerDsn::parse(&format!(
        "http://key@{}",
        dsn.trim_start_matches("http://key@")
    ))
    .expect("a DSN");
    let client = reqwest::Client::new();
    for envelope in &envelopes {
        let response = client
            .post(&dsn_parts.envelope_url)
            .header("Content-Type", "application/x-sentry-envelope")
            .body(envelope.clone())
            .send()
            .await
            .expect("the tracker answers");
        assert!(response.status().is_success());
    }
    assert_eq!(received.load(Ordering::SeqCst), 1);
    tracker.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_envelope_projects_the_fields_grouping_needs() {
    let mut event = event_from("p1", "boom");
    event.fingerprint = issue::fingerprint(&event);
    assert_eq!(event.level, Level::Error);
    assert_eq!(event.source, Source::Web);
    assert_eq!(event.release.as_deref(), Some("abc123"));
    assert_eq!(event.request_id.as_deref(), Some("7fa3"));
    assert!(
        !event.envelope.is_empty(),
        "the envelope is kept for replay"
    );
    assert_ne!(event.fingerprint, Fingerprint(String::new()));
}
