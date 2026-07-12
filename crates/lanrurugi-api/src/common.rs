//! Shared response helpers matching legacy `OperationResponse`/`MinionJobResponse` shapes
//! (`tools/openapi.yaml` components), used across every endpoint-group module.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// `{operation, success: 1, ...extra}` — HTTP 200.
pub fn ok(operation: &str, extra: impl IntoIterator<Item = (&'static str, Value)>) -> Response {
    let mut body = json!({ "operation": operation, "success": 1 });
    merge_extra(&mut body, extra);
    Json(body).into_response()
}

/// `{operation, error, success: 0}` — legacy `OperationResponse` error shape, at the given status.
pub fn error(status: StatusCode, operation: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "operation": operation,
            "error": message.into(),
            "success": 0,
        })),
    )
        .into_response()
}

pub fn not_found(operation: &str, message: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, operation, message)
}

fn merge_extra(body: &mut Value, extra: impl IntoIterator<Item = (&'static str, Value)>) {
    let Value::Object(map) = body else {
        unreachable!("ok() always builds an object")
    };
    for (k, v) in extra {
        map.insert(k.to_string(), v);
    }
}
