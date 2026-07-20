//! Newline-delimited JSON request/response protocol client, per
//! `contracts/plugin-protocol.md` (constitution Principle IV).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub request_id: String,
    pub plugin: String,
    pub method: String,
    pub args: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub request_id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseError {
    pub message: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeclaredPermissions {
    #[serde(default)]
    pub net: Vec<String>,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginParameter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginInfo {
    pub namespace: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub parameters: Vec<PluginParameter>,
    pub declared_permissions: DeclaredPermissions,
    /// Namespace of another (`login`-type) plugin to run before this one — legacy's
    /// `exec_login_plugin($pluginfo{login_from})`
    /// (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:107-135`), re-run fresh before *every* call
    /// rather than a cached session. `None` when the plugin needs no login (most metadata/script
    /// plugins) or is itself a `login`-type plugin (which never chases another login).
    #[serde(default)]
    pub login_from: Option<String>,
    // Display-only fields, matching legacy's `PluginInfo` schema (`tools/openapi.yaml`) — not part
    // of `contracts/plugin-protocol.md`'s minimal wire contract, but needed to answer
    // `GET /plugins/{type}` with the shape existing UI/tooling expects (Principle II).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub oneshot_arg: Option<String>,
    /// A regex (source only, no delimiters) matched case-insensitively against a full candidate
    /// URL to decide whether this plugin should handle it — display-only on the host side
    /// (`lanrurugi-api::plugins::list_plugins` just echoes it back); the actual matching happens
    /// entirely client-side (Upload page URL-queue grouping, metadata-preview-by-URL routing).
    #[serde(default)]
    pub url_pattern: Option<String>,
    /// Sidecar metadata filenames (basename suffixes, e.g. `"api.json"`, `"ComicInfo.xml"`) this
    /// plugin wants read out of the archive it's currently processing — `lanrurugi-plugin-converter`
    /// populates this automatically from every `is_file_in_archive(...)` call it finds in a
    /// converted plugin's source (see its own `SIDECAR_FILE_RE` docs). The host resolves each name
    /// against the current archive *before* invoking `exec_metadata` (see
    /// `lanrurugi-api::plugins::with_sidecar_files`) and hands back whatever it found as
    /// `hostArgs.sidecar_files: Record<string, string>` — content, not a path, since there's no
    /// real temp-file extraction step to hand a path *to* anymore (constitution Principle IV: the
    /// plugin itself never gets raw filesystem access for this, only these specific,
    /// host-mediated file contents).
    #[serde(default)]
    pub sidecar_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetadataResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_tags: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Mirrors `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`'s `DomainRule` field-for-field —
/// a plugin's own declared default (`pluginOptions()`) for one domain pattern's concurrency/
/// rate-limit caps (`specs/005-download-plugin-progress/data-model.md`'s `Domain Rule`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DomainRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes_per_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Mirrors `plugin-sdk.ts`'s `PluginOptionsResult.bundle_as_archive` field-for-field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BundleAsArchiveOption {
    pub default: bool,
    pub description: String,
}

/// Mirrors `plugin-sdk.ts`'s `PluginOptionsResult.overwrite_on_duplicate` field-for-field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverwriteOnDuplicateOption {
    pub default: bool,
    pub description: String,
}

/// A download plugin's `pluginOptions()` response (spec FR-015) — absent/`null` when the plugin
/// exports no such function (the common case: every non-download plugin, and a download plugin
/// with nothing to configure). Mirrors `plugin-sdk.ts`'s `PluginOptionsResult` field-for-field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginOptionsResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_rules: Vec<DomainRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_as_archive: Option<BundleAsArchiveOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite_on_duplicate: Option<OverwriteOnDuplicateOption>,
}
