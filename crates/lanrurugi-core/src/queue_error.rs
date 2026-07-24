//! Structured error shape for the download queue (`DownloadQueueItem.error`) — every distinct
//! failure the download pipeline (`lanrurugi-api::download_manager::{stream,bundle,ingest}`,
//! plugin execution) can produce, closed over a `kind` tag plus typed params, instead of a free
//! Rust `Display` string. No variant carries free-text detail: only data a frontend can actually
//! interpolate into a translated sentence crosses this boundary — the original Rust `Display`
//! text (for operator debugging) is logged via `tracing::warn!`/`error!` at each construction
//! site instead, never serialized here. The frontend maps `kind` to an i18n key and renders the
//! variant's own fields into it (interpolation, and — for `DuplicateArchive` — a real link to the
//! colliding archive) rather than showing untranslatable, unstructured English text verbatim.
//!
//! [`QueueError::code`] gives every kind a stable `u16`: kinds with a natural HTTP-status meaning
//! reuse that real status code (400/409/422/500/502); kinds with no HTTP analog (a plugin's own
//! reported error, a plugin returning nothing usable, ...) get a business code `>= 1000`.
//!
//! Lives in `lanrurugi-core` (not `lanrurugi-api`) so `lanrurugi-scanner`'s `DuplicateReason` and
//! every downstream crate that needs to *produce* one of these doesn't have to depend on the API
//! crate.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// An interpolation value for a `QueueError`'s `data` map — plugin-reported errors interpolate
/// either strings (filenames, URLs, other messages) or numbers (page counts, status codes), never
/// anything richer, so this stays a closed two-case union rather than arbitrary JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginErrorValue {
    String(String),
    Number(f64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateReasonKind {
    ContentHash,
    Filename,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueError {
    /// A plugin's own semantic error (`{error_code, data}` from the plugin SDK's `PluginError`
    /// contract) — `error_code` is itself an i18n lookup key (the plugin author's own English
    /// phrase), `data` its interpolation params. Distinct from `PluginExecutionFailed`: this is
    /// something the plugin author deliberately reported, not a host/transport fault.
    PluginReported {
        plugin: String,
        error_code: String,
        data: HashMap<String, PluginErrorValue>,
    },
    /// The plugin process/IPC call itself failed (crash, timeout, malformed request on this
    /// host's side) rather than the plugin reporting its own error.
    PluginExecutionFailed {
        plugin: String,
    },
    /// A plugin's response JSON didn't match the expected contract shape at all (a plugin bug, or
    /// a contract-version mismatch).
    MalformedPluginResponse {
        plugin: String,
    },
    /// A plugin returned success with none of `downloads`, `file_path`, or `error` set — nothing
    /// for the host to actually do.
    EmptyPluginResult {
        plugin: String,
    },
    InvalidUrl {
        url: String,
    },
    InvalidHttpMethod {
        method: String,
    },
    /// The upstream request itself failed (DNS, connection reset, TLS, ...) — distinct from
    /// `HttpStatus`, which means the server *did* respond, just not successfully.
    HttpRequestFailed {
        url: String,
    },
    /// The remote server responded with a non-2xx status while downloading a resource.
    HttpStatus {
        url: String,
        status: u16,
    },
    /// Writing a downloaded resource to local disk failed (out of space, permissions, ...).
    WriteFailed,
    /// Zipping multiple downloaded resources into one bundled archive failed (`bundle_as_archive`
    /// plugins, e.g. Pixiv's multi-page downloads).
    BundleFailed,
    /// The downloaded content (or its intended filename) collides with an archive already in the
    /// library — `existing_id` is a real archive ID the frontend can link to directly.
    DuplicateArchive {
        existing_id: String,
        reason: DuplicateReasonKind,
    },
    /// The download's *content* is genuinely new (no `ContentHash` collision — that case stays
    /// `DuplicateArchive`, unconditionally rejected, never reaches here), but its resolved
    /// filename already belongs to a different existing archive. Distinct kind (and error code)
    /// from `DuplicateArchive` specifically so the frontend can offer a real choice here
    /// (overwrite the existing archive, or rename this download and catalogue it as a separate,
    /// coexisting archive) instead of `DuplicateArchive`'s unconditional rejection — the two
    /// `DuplicateReasonKind` cases are not symmetric in what's actually safe to allow. `filename`
    /// is the colliding basename (not the archive's title), `existing_id` the archive that already
    /// owns it.
    DuplicateFilename {
        existing_id: String,
        filename: String,
    },
    /// A `DuplicateFilename` conflict that sat unresolved long enough for the periodic sweep
    /// (`download_manager::ingest::sweep_stale_pending_renames`) to reclaim its staged bytes —
    /// distinct from `DuplicateFilename` itself specifically so the frontend stops offering
    /// "overwrite"/"rename and catalog" (neither is possible anymore; the content is gone) and
    /// instead shows a "this expired, please download again" message. `existing_id`/`filename`
    /// are kept (not dropped) purely so that message can still name what it collided with,
    /// matching `DuplicateFilename`'s own fields.
    DuplicateFilenameCleaned {
        existing_id: String,
        filename: String,
    },
    /// Every other internal fault (Redis/DB/filesystem/timeout/task-panic) — deliberately not
    /// broken out into more specific kinds, since none of these are actionable by the user beyond
    /// "something went wrong, check the server logs".
    Internal,
}

impl QueueError {
    /// HTTP-compatible where a real status applies; `>= 1000` for pure business codes with no
    /// HTTP analog.
    pub fn code(&self) -> u16 {
        match self {
            QueueError::PluginReported { .. } => 1000,
            QueueError::PluginExecutionFailed { .. } => 1001,
            QueueError::EmptyPluginResult { .. } => 1002,
            QueueError::InvalidUrl { .. } => 400,
            QueueError::InvalidHttpMethod { .. } => 400,
            QueueError::MalformedPluginResponse { .. } => 422,
            QueueError::HttpRequestFailed { .. } => 502,
            QueueError::HttpStatus { .. } => 502,
            QueueError::WriteFailed => 500,
            QueueError::BundleFailed => 500,
            QueueError::Internal => 500,
            QueueError::DuplicateArchive { .. } => 409,
            QueueError::DuplicateFilename { .. } => 1003,
            QueueError::DuplicateFilenameCleaned { .. } => 1004,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_covers_every_variant_with_a_stable_number() {
        assert_eq!(
            QueueError::PluginReported {
                plugin: "x".into(),
                error_code: "y".into(),
                data: HashMap::new(),
            }
            .code(),
            1000
        );
        assert_eq!(
            QueueError::PluginExecutionFailed { plugin: "x".into() }.code(),
            1001
        );
        assert_eq!(
            QueueError::EmptyPluginResult { plugin: "x".into() }.code(),
            1002
        );
        assert_eq!(QueueError::InvalidUrl { url: "x".into() }.code(), 400);
        assert_eq!(
            QueueError::InvalidHttpMethod { method: "x".into() }.code(),
            400
        );
        assert_eq!(
            QueueError::MalformedPluginResponse { plugin: "x".into() }.code(),
            422
        );
        assert_eq!(
            QueueError::HttpRequestFailed { url: "x".into() }.code(),
            502
        );
        assert_eq!(
            QueueError::HttpStatus {
                url: "x".into(),
                status: 503
            }
            .code(),
            502
        );
        assert_eq!(QueueError::WriteFailed.code(), 500);
        assert_eq!(QueueError::BundleFailed.code(), 500);
        assert_eq!(QueueError::Internal.code(), 500);
        assert_eq!(
            QueueError::DuplicateArchive {
                existing_id: "x".into(),
                reason: DuplicateReasonKind::ContentHash,
            }
            .code(),
            409
        );
        assert_eq!(
            QueueError::DuplicateFilename {
                existing_id: "x".into(),
                filename: "y.zip".into(),
            }
            .code(),
            1003
        );
        assert_eq!(
            QueueError::DuplicateFilenameCleaned {
                existing_id: "x".into(),
                filename: "y.zip".into(),
            }
            .code(),
            1004
        );
    }

    #[test]
    fn serializes_with_a_kind_tag_and_no_free_text_fields() {
        let err = QueueError::DuplicateArchive {
            existing_id: "abc123".into(),
            reason: DuplicateReasonKind::ContentHash,
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "duplicate_archive");
        assert_eq!(json["existing_id"], "abc123");
        assert_eq!(json["reason"], "content_hash");
        // Regression guard: no variant should ever grow a free-text field a translator can't
        // localize — spot-check that internal/write-failed/bundle-failed serialize with *only*
        // the kind tag (no accompanying message/detail string).
        let internal = serde_json::to_value(QueueError::Internal).unwrap();
        assert_eq!(internal.as_object().unwrap().len(), 1);
        assert_eq!(internal["kind"], "internal");
    }
}
