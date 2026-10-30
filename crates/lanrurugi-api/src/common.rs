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
    error(StatusCode::NOT_FOUND, operation, message)
}

/// HTTP status for a failed model call, decided by its class rather than by each endpoint.
pub fn llm_error_status(class: lanrurugi_llm::LlmErrorClass) -> StatusCode {
    use lanrurugi_llm::LlmErrorClass as C;
    match class {
        C::NotConfigured | C::Transient | C::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
        C::Auth | C::Balance | C::Malformed => StatusCode::BAD_GATEWAY,
        C::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        C::Timeout => StatusCode::GATEWAY_TIMEOUT,
        C::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

/// A body streaming an existing file, without owning its lifetime — a finished page variant in
/// the resize cache, so serving a cache hit never has to hold the whole page in memory.
pub(crate) fn stream_file(path: &std::path::Path) -> std::io::Result<axum::body::Body> {
    let file = std::fs::File::open(path)?;
    Ok(axum::body::Body::from_stream(
        tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file)),
    ))
}

/// A body streaming `path`, deleting the file once the body is dropped.
///
/// For artifacts too large to buffer but cheap to rebuild (a patched archive's merged zip): the
/// request never learns when the last byte went out, so the file's lifetime is tied to the body
/// itself rather than to the handler.
pub(crate) fn stream_temp_file(path: std::path::PathBuf) -> std::io::Result<axum::body::Body> {
    use futures_util::StreamExt as _;

    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let file = std::fs::File::open(&path)?;
    let guard = RemoveOnDrop(path);
    let stream =
        tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file)).map(move |chunk| {
            // Keeps the guard alive for exactly as long as the stream exists.
            let _ = &guard;
            chunk
        });
    Ok(axum::body::Body::from_stream(stream))
}

/// The `OperationResponse` error shape for a failed model call, plus its machine-readable
/// `class` so a client can tell a timeout from a provider error from a validator rejection.
pub fn llm_error(operation: &str, err: &lanrurugi_llm::LlmError) -> Response {
    (
        llm_error_status(err.class()),
        Json(json!({
            "operation": operation,
            "error": err.message(),
            "class": err.class().as_str(),
            "success": 0,
        })),
    )
        .into_response()
}

fn merge_extra(body: &mut Value, extra: impl IntoIterator<Item = (&'static str, Value)>) {
    let Value::Object(map) = body else {
        unreachable!("ok() always builds an object")
    };
    for (k, v) in extra {
        map.insert(k.to_string(), v);
    }
}
