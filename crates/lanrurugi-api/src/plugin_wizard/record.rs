//! The generation record (`specs/014-ai-harness-hardening/contracts/generation-record.md`): what
//! the model was shown, round by round, for one plugin-wizard generation, kept 30 days so a "the
//! wizard wrote a plugin that does not work" report is reproducible after a restart.
//!
//! Elements, in append order:
//! - `header`: the generation request (credential fields in placeholder form), model, request
//!   parameters and system prompt.
//! - `request`: written *before* a round is sent — the messages added since the previous round
//!   (the header's prompt plus every `request`'s `messages`, in order, is the full conversation).
//! - `tool_round` / `final`: the round's response — tool calls with the results the model was
//!   shown, or the final answer.
//! - `failed_attempt`: a round that failed or was retried; never content the model was shown.
//! - `trial_run`: a trial run of this generation's draft, with its attribution.
//!
//! Every element passes through [`redact`] before it is stored, so a credential value that
//! surfaced anywhere (a page echoing request headers back) is stored as its placeholder.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::ids::GenerationId;
use lanrurugi_storage::keys::{generation_record_key, GENERATION_RECORD_TTL_SECS};
use serde_json::{json, Value};

use crate::AppState;

/// Request fields whose values are credentials or plugin secrets.
const SECRET_FIELDS: [&str; 3] = [
    "credential_values",
    "login_credentials",
    "plugin_parameter_values",
];

/// Replaces every known credential value in `text` with its `{{name}}` placeholder.
pub(super) fn redact(text: &str, credential_values: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    for (name, value) in credential_values {
        if !value.is_empty() {
            out = out.replace(value.as_str(), &format!("{{{{{name}}}}}"));
        }
    }
    out
}

/// The generation request with every secret field reduced to placeholders plus a
/// "was supplied" flag per field.
pub(super) fn request_placeholder_form(body: &Value) -> Value {
    let mut body = body.clone();
    let Some(map) = body.as_object_mut() else {
        return body;
    };
    for field in SECRET_FIELDS {
        let Some(value) = map.get_mut(field) else {
            continue;
        };
        *value = placeholder_object(value);
    }
    body
}

fn placeholder_object(value: &Value) -> Value {
    match value {
        // `{name: value}` maps and `{fields: {name: value}}` credential objects.
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| {
                    let replaced = match v {
                        Value::Object(_) => placeholder_object(v),
                        Value::String(s) => {
                            json!({ "value": format!("{{{{{k}}}}}"), "supplied": !s.is_empty() })
                        }
                        Value::Null => json!({ "value": Value::Null, "supplied": false }),
                        _ => json!({ "value": format!("{{{{{k}}}}}"), "supplied": true }),
                    };
                    (k.clone(), replaced)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Appends one element to a generation's record; sets the TTL on first append. Best effort: a
/// Redis failure is logged and never fails the generation it describes.
pub(super) async fn append(
    state: &AppState,
    id: &GenerationId,
    mut element: Value,
    credential_values: &HashMap<String, String>,
) {
    if let Some(map) = element.as_object_mut() {
        map.insert("at".into(), json!(chrono::Utc::now().timestamp_millis()));
    }
    let text = redact(&element.to_string(), credential_values);
    let key = generation_record_key(id);
    let result = async {
        let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
        let len: i64 = conn.rpush(&key, text).await.map_err(|e| e.to_string())?;
        if len == 1 {
            let _: () = conn
                .expire(&key, GENERATION_RECORD_TTL_SECS)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(generation_id = %id, error = %e, "generation record append failed");
    }
}

/// `GET /plugin-wizard/generate/{id}/record` — the whole read surface; no UI.
pub(super) async fn get_record(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let id = GenerationId(id);
    let key = generation_record_key(&id);
    let items: Result<Vec<String>, String> = async {
        let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
        conn.lrange(&key, 0, -1).await.map_err(|e| e.to_string())
    }
    .await;
    match items {
        Ok(items) if items.is_empty() => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "generation_record_not_found" })),
        )
            .into_response(),
        Ok(items) => {
            let attempts: Vec<Value> = items
                .iter()
                .enumerate()
                .map(|(index, raw)| {
                    let mut v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
                    if let Some(map) = v.as_object_mut() {
                        map.insert("index".into(), json!(index));
                    }
                    v
                })
                .collect();
            Json(json!({ "generation_id": id, "attempts": attempts })).into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "generation_record_unavailable", "detail": e })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_request_fields_become_placeholders() {
        let body = json!({
            "plugin_type": "metadata",
            "credential_values": { "token": "SECRET-1", "empty": "" },
            "login_credentials": { "fields": { "password": "hunter2" } },
            "plugin_parameter_values": { "api_key": "K-9" },
            "test_links": ["https://site-a.example/x"],
        });
        let form = request_placeholder_form(&body);
        let text = form.to_string();
        for secret in ["SECRET-1", "hunter2", "K-9"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(form["credential_values"]["token"]["value"], "{{token}}");
        assert_eq!(form["credential_values"]["empty"]["supplied"], false);
        assert_eq!(
            form["login_credentials"]["fields"]["password"]["supplied"],
            true
        );
        assert_eq!(form["test_links"][0], "https://site-a.example/x");
    }

    /// Quickstart S6: the record survives a "restart" (a fresh state on the same Redis), keeps its
    /// order and kinds, never holds the sentinel credential, and an unknown id is a 404.
    #[tokio::test]
    async fn s6_record_round_trips_in_order_with_no_cleartext_secret() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let sentinel = format!("SENTINEL-{}", uuid::Uuid::new_v4());
        let id = GenerationId(format!("test-{}", uuid::Uuid::new_v4()));
        let creds = HashMap::from([("token".to_string(), sentinel.clone())]);
        let request = json!({ "credential_values": { "token": sentinel } });
        for element in [
            json!({ "kind": "header", "generation_request": request_placeholder_form(&request) }),
            json!({ "kind": "request", "messages": [{ "role": "user", "content": "go" }] }),
            json!({ "kind": "tool_round", "tool_results": [{ "result": format!("echo: Key {sentinel}") }] }),
            json!({ "kind": "failed_attempt", "outcome": "retried" }),
            json!({ "kind": "trial_run", "trial": { "plugin_type": "metadata" } }),
        ] {
            append(&state, &id, element, &creds).await;
        }

        let fresh = crate::plugins::tests::test_state().await.unwrap();
        let response = get_record(State(fresh.clone()), Path(id.0.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !text.contains(&sentinel),
            "cleartext credential in record: {text}"
        );
        let body: Value = serde_json::from_str(&text).unwrap();
        let kinds: Vec<&str> = body["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "header",
                "request",
                "tool_round",
                "failed_attempt",
                "trial_run"
            ]
        );

        let mut conn = fresh.redis.config.get().await.unwrap();
        let ttl: i64 = conn.ttl(generation_record_key(&id)).await.unwrap();
        assert!(ttl > 0 && ttl <= GENERATION_RECORD_TTL_SECS);
        let _: () = conn.del(generation_record_key(&id)).await.unwrap();

        let missing = get_record(State(fresh), Path("no-such-generation".into())).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn redaction_replaces_values_that_surfaced_anywhere() {
        let creds = HashMap::from([("token".to_string(), "SECRET-1".to_string())]);
        let echoed = r#"{"content":"Authorization: Key SECRET-1"}"#;
        assert_eq!(
            redact(echoed, &creds),
            r#"{"content":"Authorization: Key {{token}}"}"#
        );
    }
}
