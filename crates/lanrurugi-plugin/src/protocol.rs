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
    /// A plugin-authored structured error's `error_code` (`plugin-sdk.ts`'s `PluginError`/
    /// `PluginErrorException`) — an i18n lookup key, present only when the plugin actually threw
    /// or returned one; `None` for an unexpected/unstructured fault (a genuine bug, a network
    /// library's own exception), which `lanrurugi-api::plugins` maps to
    /// `QueueError::PluginExecutionFailed` instead of `QueueError::PluginReported`.
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub data:
        Option<std::collections::HashMap<String, lanrurugi_core::queue_error::PluginErrorValue>>,
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
    /// Matches `plugin-sdk.ts`'s own `PluginParameter.type` docs — see that comment for why
    /// `Some("color")`, though technically valid on the wire, is never emitted by any real plugin
    /// in this corpus and legacy's own equivalent default is a quirk this SDK deliberately doesn't
    /// mirror. `None` (absent `type`) is treated the same as `Some("string")` by the frontend.
    #[serde(default, rename = "type")]
    pub param_type: Option<String>,
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
    /// URL to decide whether this plugin should handle it — the precise trigger condition for a
    /// real download/metadata fetch (`lanrurugi-api::plugins::find_matching_plugin`, the Upload
    /// page's own URL-queue grouping). NOT what a domain-ownership lookup should match against —
    /// see `domain_match` below for that.
    #[serde(default)]
    pub url_pattern: Option<String>,
    /// Bare domains (no scheme, no path, e.g. `["e-hentai.org", "exhentai.org"]`) this plugin
    /// considers itself the owner of — used only for "does this domain belong to this plugin"
    /// lookups (Upload page's metadata-button enablement, the AI wizard's domain-coverage check),
    /// never for real download/metadata-fetch dispatch (still `url_pattern`'s job, unchanged).
    /// Empty when undeclared — the host then falls back to treating `url_pattern` itself as a
    /// loose domain regex (see `plugins.rs::domain_covers`), so a plugin that never declares this
    /// keeps working exactly as before.
    #[serde(default)]
    pub domain_match: Vec<String>,
    /// `true` only for a plugin the AI plugin creation wizard generated (`specs/
    /// 006-ai-plugin-wizard` FR-026/FR-027) — declared by the plugin's own `pluginInfo()`, never
    /// inferred by the host. Display-only, same category as the other fields below `login_from`.
    #[serde(default)]
    pub generated_by_wizard: bool,
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

/// A plugin-authored error — mirrors `plugin-sdk.ts`'s `PluginError` field-for-field. `error_code`
/// doubles as an i18n lookup key (see that interface's own docs for the full naming convention);
/// `data` is its interpolation params.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginError {
    pub error_code: String,
    #[serde(default)]
    pub data:
        Option<std::collections::HashMap<String, lanrurugi_core::queue_error::PluginErrorValue>>,
}

/// A single cookie exactly as `plugin-sdk.ts`'s `LegacyCookie`/`plugins/legacy-globals.d.ts`
/// shapes it — round-tripped host-side between `exec_login`'s result and the next
/// `exec_metadata`/`exec_download`/`exec_script` call's `hostArgs.user_agent_cookies` (see
/// `lanrurugi-api::plugins::with_login_cookies`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
}

/// `execMetadata`'s return shape — mirrors `plugin-sdk.ts`'s `MetadataResult` field-for-field.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataResult {
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub error: Option<PluginError>,
}

/// `execLogin`'s return shape — mirrors `plugin-sdk.ts`'s `LoginResult` field-for-field. Only
/// `cookies`/`headers` are ever read by the host (`lanrurugi-api::plugins::with_login_cookies`).
#[derive(Debug, Clone, Deserialize)]
pub struct LoginResult {
    #[serde(default)]
    pub cookies: Option<Vec<PluginCookie>>,
    /// Header/token-based credentials (e.g. `Authorization`) — the only way a login plugin that
    /// authenticates via a header rather than a cookie can pass that credential to a downstream
    /// metadata/download call at all (issue #78/#93: `plugins/login/nhentai.ts`'s API-Key
    /// `Authorization` header used to have no way to cross this boundary before this field
    /// existed).
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub error: Option<PluginError>,
}

/// One archive's `id`/`tags`, as passed into or returned from a `"script"`-type plugin — mirrors
/// `plugin-sdk.ts`'s `ScriptArchiveTags` field-for-field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptArchiveTags {
    pub id: String,
    pub tags: String,
}

/// `runScript`'s return shape — mirrors `plugin-sdk.ts`'s `ScriptResult` field-for-field. A
/// deliberately loose bag of fields (see that interface's own docs for why): each real script
/// plugin only ever populates the subset it needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScriptResult {
    #[serde(default)]
    pub total: Option<u32>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub error: Option<PluginError>,
    #[serde(default)]
    pub modified: Option<u32>,
    #[serde(default)]
    pub updates: Option<Vec<ScriptArchiveTags>>,
    #[serde(default)]
    pub categories_to_create: Option<Vec<ScriptCategoryToCreate>>,
    #[serde(default)]
    pub delete_old_categories: Option<bool>,
    #[serde(default)]
    pub elapsed_ms: Option<f64>,
}

/// One static Category `runScript`'s result asks the host to create — mirrors `plugin-sdk.ts`'s
/// `ScriptResult.categories_to_create` element shape field-for-field (`foldertocat`).
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptCategoryToCreate {
    pub name: String,
    pub archive_ids: Vec<String>,
}

/// One `downloads[]` entry as `execDownload` returns it — mirrors `plugin-sdk.ts`'s
/// `DownloadRequest` and `contracts/plugin-download-protocol.md`'s wire shape field-for-field.
#[derive(Debug, Clone, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub filename_hint: Option<String>,
}

/// `execDownload`'s full return shape — mirrors `plugin-sdk.ts`'s `DownloadResult` and
/// `contracts/plugin-download-protocol.md`'s extended `DownloadResult`. Exactly one of
/// `downloads`/`file_path`/`error` is expected to be present.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DownloadResult {
    #[serde(default)]
    pub downloads: Option<Vec<DownloadRequest>>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub error: Option<PluginError>,
}
