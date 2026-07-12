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
