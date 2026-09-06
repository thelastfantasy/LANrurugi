//! Archive split suggestions: detect multi-volume/nested-directory archives, ask the LLM for a
//! split/repackage plan, store it, and let the user execute it as a background job from the
//! reader page.
//!
//! The split is never executed automatically. The reader page explicitly starts a job; the job
//! reports progress over SSE while the page is open, and the final result is also persisted in
//! the normal `JobRegistry` so the user can inspect it after navigating away.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path as AxumPath, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_llm::LlmClient;
use lanrurugi_scanner::archive_format::{
    list_all_entries, list_pages, read_entries, ArchiveEntryInfo,
};
use lanrurugi_storage::archive_split_suggestions::{
    ArchiveSplitSuggestion, ArchiveSplitSuggestionsRepository, SplitGroupSuggestion,
};

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::AppState;

const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "cbz", "epub", "rar", "cbr", "7z", "cb7", "lzh", "lha", "tar", "gz", "bz2", "xz",
];

#[derive(Debug, Deserialize)]
struct LlmSplitGroup {
    zip_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    source_dirs: Vec<String>,
    #[serde(default)]
    source_files: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ExecuteSplitParams {
    #[serde(default)]
    delete_original: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct LlmSplitResponse {
    #[serde(default)]
    split_groups: Vec<LlmSplitGroup>,
    #[serde(default)]
    warnings: Vec<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/archives/{id}/split-suggestion",
            get(get_split_suggestion).post(analyze_split_suggestion),
        )
        .route("/archives/{id}/split/execute", post(execute_split))
        .route("/archives/{id}/split/stream", get(split_progress_stream))
        .route("/archives/{id}/split-tree", get(get_split_tree))
}

fn split_repo(state: &AppState) -> ArchiveSplitSuggestionsRepository {
    ArchiveSplitSuggestionsRepository::new(state.redis.config.clone())
}

fn resolve_multivolume_path(path: &Path, temp_dir: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_string_lossy().to_string();

    // Pattern A: `archive.7z.001`, `archive.rar.002`, ...
    let mut base_name = None;
    let mut parts = Vec::new();
    if let Some((base, suffix)) = file_name.rsplit_once('.') {
        if suffix.len() >= 2 && suffix.chars().all(|c| c.is_ascii_digit()) {
            let base_path = path.with_file_name(base);
            let dir = path.parent()?;
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut numbered: Vec<(u64, PathBuf)> = entries
                    .flatten()
                    .filter_map(|e| {
                        let p = e.path();
                        let n = p.file_name()?.to_string_lossy().to_string();
                        let (b, suffix) = n.rsplit_once('.')?;
                        if b == base && suffix.chars().all(|c| c.is_ascii_digit()) {
                            suffix.parse::<u64>().ok().map(|num| (num, p))
                        } else {
                            None
                        }
                    })
                    .collect();
                numbered.sort_by_key(|(num, _)| *num);
                if !numbered.is_empty() {
                    parts = numbered.into_iter().map(|(_, p)| p).collect();
                    base_name = Some(base_path);
                }
            }
        }
    }

    // Pattern B: `archive.part1.rar`, `archive.part00002.rar`
    if base_name.is_none() && is_archive_or_sfx_name(&file_name) {
        if let Some((before_part, ext)) = file_name.rsplit_once('.') {
            if let Some((prefix, num_str)) = before_part.rsplit_once(".part") {
                if num_str.chars().all(|c| c.is_ascii_digit()) {
                    let dir = path.parent()?;
                    let base_base = format!("{prefix}.{ext}");
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        let mut numbered: Vec<(u64, PathBuf)> = entries
                            .flatten()
                            .filter_map(|e| {
                                let p = e.path();
                                let n = p.file_name()?.to_string_lossy().to_string();
                                if let Some((p2, e2)) = n.rsplit_once('.') {
                                    if e2 != ext {
                                        return None;
                                    }
                                    let (pr, num_str) = p2.rsplit_once(".part")?;
                                    if pr == prefix && num_str.chars().all(|c| c.is_ascii_digit()) {
                                        return num_str.parse::<u64>().ok().map(|num| (num, p));
                                    }
                                }
                                None
                            })
                            .collect();
                        numbered.sort_by_key(|(num, _)| *num);
                        if !numbered.is_empty() {
                            parts = numbered.into_iter().map(|(_, p)| p).collect();
                            base_name = Some(path.with_file_name(base_base));
                        }
                    }
                }
            }
        }
    }

    // Pattern C: first file is a normal archive (e.g. `archive.zip` / `archive.rar`)
    // and the rest use `.z01/.z02...` (WinZip/Info-ZIP split ZIP) or `.r00/.r01...`
    // (legacy split RAR).
    if base_name.is_none() && is_archive_or_sfx_name(&file_name) {
        let stem = path.with_extension("");
        let stem_name = stem
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Ok(entries) = std::fs::read_dir(path.parent().unwrap_or(Path::new("."))) {
            let mut numbered: Vec<(u64, PathBuf)> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let n = p.file_name()?.to_string_lossy().to_string();
                    for prefix in ["z", "r"] {
                        if let Some(suffix) = n.strip_prefix(&format!("{stem_name}.{prefix}")) {
                            if suffix.len() >= 2 && suffix.chars().all(|c| c.is_ascii_digit()) {
                                if let Ok(num) = suffix.parse::<u64>() {
                                    return Some((num, p));
                                }
                            }
                        }
                    }
                    // Generic numeric continuation: `archive.001`, `archive.002`, ...
                    if let Some(suffix) = n.strip_prefix(&format!("{stem_name}.")) {
                        if suffix.len() >= 2 && suffix.chars().all(|c| c.is_ascii_digit()) {
                            if let Ok(num) = suffix.parse::<u64>() {
                                return Some((num, p));
                            }
                        }
                    }
                    // Same-name split: `archive.7z` + `archive.7z.001`, `archive.zip` + `.zip.001`.
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if let Some(suffix) = n.strip_prefix(&format!("{stem_name}.{ext}.")) {
                            if suffix.len() >= 2 && suffix.chars().all(|c| c.is_ascii_digit()) {
                                if let Ok(num) = suffix.parse::<u64>() {
                                    return Some((num, p));
                                }
                            }
                        }
                    }
                    // New-style WinRAR/SFX continuation: `base.part2.rar`, `base.part3.rar`.
                    if let Some(rest) = n.strip_prefix(&format!("{stem_name}.part")) {
                        if let Some((num_str, ext)) = rest.rsplit_once('.') {
                            if num_str.chars().all(|c| c.is_ascii_digit())
                                && ARCHIVE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                            {
                                if let Ok(num) = num_str.parse::<u64>() {
                                    return Some((num, p));
                                }
                            }
                        }
                    }
                    None
                })
                .collect();
            if !numbered.is_empty() {
                numbered.sort_by_key(|(num, _)| *num);
                let mut all_parts = vec![path.to_path_buf()];
                all_parts.extend(numbered.into_iter().map(|(_, p)| p));
                parts = all_parts;
                base_name = Some(path.to_path_buf());
            }
        }
    }

    if parts.is_empty() {
        return None;
    }

    let base = base_name?;
    let base_name_str = base.file_name().map(|s| s.to_string_lossy().to_string())?;
    // Stable cache key from the participating volume files' names+sizes, so re-analyzing the same
    // split set reuses the merged file instead of re-reading and re-concatenating hundreds of MB.
    let mut cache_key = String::new();
    for part in &parts {
        if let Ok(meta) = std::fs::metadata(part) {
            cache_key.push_str(&format!(
                "{}:{};",
                part.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                meta.len()
            ));
        }
    }
    let cache_dir = temp_dir.join("archive-split-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let temp_target = cache_dir.join(format!(
        "{}-{}",
        crc32fast::hash(cache_key.as_bytes()),
        base_name_str
    ));
    if temp_target.exists() {
        return Some(temp_target);
    }
    let mut out = std::fs::File::create(&temp_target).ok()?;
    for part in parts {
        let mut input = std::fs::File::open(&part).ok()?;
        if std::io::copy(&mut input, &mut out).is_err() {
            return None;
        }
    }
    drop(out);

    // If the merged bytes form a recognisable archive even when the original volume name had no
    // extension, give the temp file the right extension so libarchive can open it.
    if let Some(ext) = sniff_archive_extension(&temp_target) {
        let final_path = temp_target.with_extension(ext);
        if final_path != temp_target {
            let _ = std::fs::rename(&temp_target, &final_path);
            return Some(final_path);
        }
        return Some(temp_target);
    }
    None
}

fn is_supported_archive_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| ARCHIVE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_archive_or_sfx_name(name: &str) -> bool {
    is_supported_archive_name(name) || name.to_ascii_lowercase().ends_with(".exe")
}

fn sniff_archive_extension(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 512];
    let Ok(mut f) = std::fs::File::open(path) else {
        return None;
    };
    use std::io::Read;
    let n = f.read(&mut head).ok()?;
    let bytes = &head[..n];
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some("zip");
    }
    if bytes.starts_with(b"Rar!\x1a\x07") {
        return Some("rar");
    }
    if bytes.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some("7z");
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Some("gz");
    }
    if bytes.starts_with(b"BZh") {
        return Some("bz2");
    }
    if bytes.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        return Some("xz");
    }
    if bytes.starts_with(b"%PDF") {
        return Some("pdf");
    }
    if bytes.windows(5).any(|w| w == b"ustar") {
        return Some("tar");
    }
    None
}

fn is_suspicious_structure(entries: &[ArchiveEntryInfo]) -> bool {
    // Nested archive files are always a split-candidate trigger.
    if entries
        .iter()
        .any(|e| e.is_regular_file && is_supported_archive_name(&e.name))
    {
        return true;
    }

    let mut top_level_dirs = HashSet::new();
    let mut immediate_children: HashSet<String> = HashSet::new();
    for e in entries.iter().filter(|e| !e.is_regular_file) {
        let mut parts = e.name.split('/').filter(|s| !s.is_empty());
        if let Some(top) = parts.next() {
            top_level_dirs.insert(top.to_string());
            if let Some(second) = parts.next() {
                immediate_children.insert(format!("{top}/{second}"));
            }
        }
    }

    if top_level_dirs.len() >= 2 {
        return true;
    }

    // One top-level directory with multiple direct child directories.
    if top_level_dirs.len() == 1 {
        let top = top_level_dirs.into_iter().next().unwrap();
        let child_count = immediate_children
            .iter()
            .filter(|p| p.starts_with(&format!("{top}/")))
            .count();
        if child_count >= 2 {
            return true;
        }
    }

    false
}

fn build_tree_payload(
    path: &Path,
    entries: &[ArchiveEntryInfo],
    temp_dir: &Path,
) -> serde_json::Value {
    let mut dirs = Vec::new();
    for e in entries.iter().filter(|e| !e.is_regular_file) {
        let prefix = format!("{}/", e.name);
        let file_count = entries
            .iter()
            .filter(|f| f.is_regular_file && f.name.starts_with(&prefix))
            .count();
        let files: Vec<&str> = entries
            .iter()
            .filter(|f| f.is_regular_file && f.name.starts_with(&prefix))
            .map(|f| f.name.as_str())
            .collect();
        dirs.push(json!({
            "path": e.name,
            "file_count": file_count,
            "files": files,
        }));
    }

    let all_files: Vec<serde_json::Value> = entries
        .iter()
        .filter(|e| e.is_regular_file)
        .map(|e| {
            json!({
                "path": e.name,
                "is_image": e.is_page,
                "is_archive": is_supported_archive_name(&e.name),
            })
        })
        .collect();

    let mut nested_archives = Vec::new();
    for e in entries
        .iter()
        .filter(|e| e.is_regular_file && is_supported_archive_name(&e.name))
    {
        // Parse the nested archive so the LLM sees its internal files instead of just a marker.
        if let Ok(bytes) = lanrurugi_scanner::archive_format::read_entry(path, &e.name) {
            let safe_name = e.name.replace(['/', '\\'], "_");
            let nested_path = temp_dir.join(format!("archive-split-tree-{safe_name}"));
            if std::fs::write(&nested_path, bytes).is_ok() {
                if let Ok(inner_entries) = list_all_entries(&nested_path) {
                    let inner_dirs: Vec<&str> = inner_entries
                        .iter()
                        .filter(|x| !x.is_regular_file)
                        .map(|x| x.name.as_str())
                        .collect();
                    let inner_files: Vec<&str> = inner_entries
                        .iter()
                        .filter(|x| x.is_regular_file)
                        .map(|x| x.name.as_str())
                        .collect();
                    nested_archives.push(json!({
                        "path": e.name,
                        "inner_dirs": inner_dirs,
                        "inner_files": inner_files,
                    }));
                }
                let _ = std::fs::remove_file(&nested_path);
            }
        }
    }

    json!({
        "archive_name": path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "directories": dirs,
        "files": all_files,
        "nested_archives": nested_archives,
    })
}

/// Maximum number of nested archives to expand in the split-tree viewer. Beyond this depth a
/// nested archive is shown as a regular file (not an empty folder), so a maliciously deep archive
/// cannot make the endpoint recurse forever or build an unbounded JSON response.
const MAX_NESTED_TREE_DEPTH: usize = 6;

/// Recursively expands nested archives into a flat `ArchiveEntryInfo` list that the existing
/// frontend tree component can render. Each expanded archive is represented by a synthetic
/// directory entry with the archive's own path; its contents are prefixed with that path so
/// sub-sub-archives keep their full context.
fn collect_split_tree_entries(
    archive_path: &Path,
    entries: &[ArchiveEntryInfo],
    temp_dir: &Path,
    prefix: &str,
    depth: usize,
    out: &mut Vec<ArchiveEntryInfo>,
) {
    for e in entries {
        let name = if prefix.is_empty() {
            e.name.clone()
        } else {
            format!("{prefix}/{}", e.name)
        };

        if e.is_regular_file && is_supported_archive_name(&e.name) {
            if depth < MAX_NESTED_TREE_DEPTH {
                // Show the archive as an expandable folder, then attach its contents.
                out.push(ArchiveEntryInfo {
                    name: name.clone(),
                    is_regular_file: false,
                    is_page: false,
                });
                if let Ok(bytes) =
                    lanrurugi_scanner::archive_format::read_entry(archive_path, &e.name)
                {
                    let safe = format!("split-tree-{}-{}", depth, crc32fast::hash(name.as_bytes()));
                    let ext = Path::new(&e.name)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("zip");
                    let nested_path = temp_dir.join(format!("{safe}.{ext}"));
                    if std::fs::write(&nested_path, bytes).is_ok() {
                        if let Ok(inner_entries) = list_all_entries(&nested_path) {
                            collect_split_tree_entries(
                                &nested_path,
                                &inner_entries,
                                temp_dir,
                                &name,
                                depth + 1,
                                out,
                            );
                        }
                        let _ = std::fs::remove_file(&nested_path);
                    }
                }
            } else {
                out.push(ArchiveEntryInfo {
                    name,
                    is_regular_file: true,
                    is_page: false,
                });
            }
        } else {
            out.push(ArchiveEntryInfo {
                name,
                is_regular_file: e.is_regular_file,
                is_page: e.is_page,
            });
        }
    }
}

/// `GET /archives/{id}/split-tree` — returns the archive's internal directory tree with nested
/// archives recursively expanded. This is a viewer-only endpoint: it does not call the LLM and
/// does not create any output ZIPs.
async fn get_split_tree(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<ArchiveId>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return crate::common::not_found("get_split_tree", "Archive not found."),
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "get_split_tree",
                e.to_string(),
            )
        }
    };
    let original_path = PathBuf::from(&archive.file);
    let path = resolve_multivolume_path(&original_path, &state.library.temp_dir)
        .unwrap_or_else(|| original_path.clone());
    let entries = match list_all_entries(&path) {
        Ok(entries) => entries,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "get_split_tree",
                e.to_string(),
            )
        }
    };
    let nano = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_root = state.library.temp_dir.join(format!("split-tree-{}", nano));
    let _ = std::fs::create_dir_all(&temp_root);
    let mut flat = Vec::new();
    collect_split_tree_entries(&path, &entries, &temp_root, "", 0, &mut flat);
    let _ = std::fs::remove_dir_all(&temp_root);
    axum::Json(json!({
        "archive_name": path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "entries": flat,
    }))
    .into_response()
}

async fn generate_suggestion(
    state: &AppState,
    archive_id: &ArchiveId,
) -> Result<Option<ArchiveSplitSuggestion>, String> {
    if !crate::settings::read_archive_split_suggestions_enabled(state).await {
        return Err("Archive split suggestions are disabled.".to_string());
    }
    if state.redis.config.ensure_available().await.is_err() {
        return Err("DeepSeek API key is not configured.".to_string());
    }

    let archive = state
        .repos
        .archives
        .get(archive_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "Archive not found.".to_string())?;
    let original_path = PathBuf::from(&archive.file);
    let path = resolve_multivolume_path(&original_path, &state.library.temp_dir)
        .unwrap_or_else(|| original_path.clone());
    let entries = list_all_entries(&path).map_err(|e| e.to_string())?;
    if !is_suspicious_structure(&entries) {
        return Ok(None);
    }

    let tree = build_tree_payload(&path, &entries, &state.library.temp_dir);
    let system = crate::llm_prompts::archive_split_suggestion_system();
    let user = serde_json::to_string_pretty(&tree).unwrap_or_default();
    tracing::info!(id = %archive_id, "archive_split: LLM request user payload:\n{user}");
    let response: LlmSplitResponse = state
        .redis
        .config
        .json_chat(&system, &user, 0.2, 3000)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(id = %archive_id, response = ?response, "archive_split: LLM response");

    let split_groups: Vec<SplitGroupSuggestion> = response
        .split_groups
        .into_iter()
        .map(|g| SplitGroupSuggestion {
            zip_name: g.zip_name,
            description: g.description,
            source_dirs: g.source_dirs,
            source_files: g.source_files,
        })
        .collect();

    let all_files: HashSet<&str> = entries
        .iter()
        .filter(|e| e.is_regular_file)
        .map(|e| e.name.as_str())
        .collect();
    validate_coverage(&all_files, &split_groups)?;

    let suggestion = ArchiveSplitSuggestion {
        archive_id: archive_id.0.clone(),
        suggestion_version: 1,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        split_groups,
        warnings: response.warnings,
    };
    split_repo(state)
        .save(&suggestion)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(suggestion))
}

fn validate_coverage(
    all_files: &HashSet<&str>,
    groups: &[SplitGroupSuggestion],
) -> Result<(), String> {
    let all_files: Vec<&str> = all_files.iter().copied().collect();
    let mut covered = HashSet::new();
    for group in groups {
        for dir in &group.source_dirs {
            let prefix = format!("{dir}/");
            for f in &all_files {
                if f.starts_with(&prefix) {
                    covered.insert(*f);
                }
            }
        }
        for f in &group.source_files {
            covered.insert(f.as_str());
        }
    }
    let missing: Vec<&str> = all_files
        .iter()
        .filter(|f| !covered.contains(*f))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "LLM split suggestion is incomplete, {} file(s) unassigned: first missing = {:?}",
            missing.len(),
            missing.first().copied().unwrap_or("")
        ));
    }
    Ok(())
}

/// Manual "analyze this archive now" endpoint.
async fn analyze_split_suggestion(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<ArchiveId>,
) -> Response {
    match generate_suggestion(&state, &id).await {
        Ok(Some(suggestion)) => axum::Json(json!({
            "operation": "analyze_split_suggestion",
            "success": 1,
            "suggestion": suggestion,
        }))
        .into_response(),
        Ok(None) => axum::Json(json!({
            "operation": "analyze_split_suggestion",
            "success": 1,
            "suggestion": null,
            "reason": "archive does not need splitting",
        }))
        .into_response(),
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "analyze_split_suggestion",
            e,
        ),
    }
}

async fn get_split_suggestion(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<ArchiveId>,
) -> Response {
    match split_repo(&state).get(&id.0).await {
        Ok(Some(suggestion)) => axum::Json(json!({ "suggestion": suggestion })).into_response(),
        Ok(None) => match generate_suggestion(&state, &id).await {
            Ok(Some(suggestion)) => axum::Json(json!({ "suggestion": suggestion })).into_response(),
            Ok(None) => crate::common::not_found(
                "get_split_suggestion",
                "No split suggestion for this archive.",
            ),
            Err(e) => {
                tracing::warn!(id = %id, error = %e, "archive_split: on-read analysis failed");
                crate::common::not_found(
                    "get_split_suggestion",
                    "No split suggestion for this archive.",
                )
            }
        },
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "get_split_suggestion",
            e.to_string(),
        ),
    }
}

async fn execute_split(
    State(state): State<AppState>,
    Query(q): Query<ExecuteSplitParams>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    AxumPath(id): AxumPath<ArchiveId>,
) -> Response {
    let suggestion = match split_repo(&state).get(&id.0).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return crate::common::not_found(
                "execute_split",
                "No split suggestion for this archive.",
            )
        }
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "execute_split",
                e.to_string(),
            )
        }
    };

    let job_id = state.jobs.create("archive-split").await;
    let (tx, _rx) = broadcast::channel(64);
    {
        let mut map = state.split_progress_tx.lock().await;
        map.insert(id.0.clone(), tx);
    }
    let job_id_for_task = job_id.clone();
    let state = state.clone();
    tokio::spawn(async move {
        let result = run_split_job(
            &state,
            &id,
            suggestion,
            auth.map(|e| e.0),
            q.delete_original,
        )
        .await;
        let _ = state.split_progress_tx.lock().await.remove(&id.0);
        if let Ok(value) = &result {
            let _ = state.jobs.set_progress(&job_id_for_task, 1.0).await;
            let _ = state.jobs.finish(&job_id_for_task, value.clone()).await;
        } else {
            let _ = state
                .jobs
                .finish(
                    &job_id_for_task,
                    json!({ "success": false, "error": result.err().unwrap_or_default() }),
                )
                .await;
        }
    });

    axum::Json(json!({ "job_id": job_id, "success": 1 })).into_response()
}

async fn split_progress_stream(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<ArchiveId>,
) -> Response {
    let rx = state.split_progress_tx.lock().await.get(&id.0).cloned();
    let Some(tx) = rx else {
        return crate::common::not_found(
            "split_progress_stream",
            "No active split job for this archive.",
        );
    };
    let rx = tx.subscribe();
    let events = BroadcastStream::new(rx).filter_map(|r| {
        let v = r.ok()?;
        Some(Ok::<_, std::convert::Infallible>(
            Event::default().json_data(v).unwrap_or_default(),
        ))
    });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn read_proc_status_kb(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start();
            let value = rest.split_whitespace().next()?;
            return value.parse().ok();
        }
    }
    None
}

fn current_memory_kb() -> Option<u64> {
    read_proc_status_kb("VmRSS:")
}

fn peak_memory_kb() -> Option<u64> {
    read_proc_status_kb("VmHWM:")
}

async fn emit_progress(
    state: &AppState,
    archive_id: &ArchiveId,
    progress: f32,
    message: &str,
    memory_kb: Option<u64>,
) {
    if let Some(tx) = state
        .split_progress_tx
        .lock()
        .await
        .get(&archive_id.0)
        .cloned()
    {
        let _ = tx.send(json!({
            "progress": progress,
            "message": message,
            "memory_kb": memory_kb,
        }));
    }
}

fn write_zip_entry<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    name: &str,
    data: &[u8],
    seen: &mut HashSet<String>,
) -> Result<String, String> {
    let mut out_name = name.to_string();
    if seen.contains(&out_name) {
        let crc = crc32fast::hash(name.as_bytes());
        let stem = Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = Path::new(name)
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "bin".to_string());
        out_name = format!("{stem}_{crc:08x}.{ext}");
    }
    seen.insert(out_name.clone());
    let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    writer
        .start_file(&out_name, options)
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(writer, data).map_err(|e| e.to_string())?;
    Ok(out_name)
}

fn stamp_page_from_id(stamp_id: &str) -> Option<u32> {
    let rest = stamp_id.strip_prefix("STAMPS_")?;
    let (page, _) = rest.split_once('_')?;
    page.parse().ok()
}

async fn inherit_metadata_to_split_archives(
    state: &AppState,
    original: &lanrurugi_core::entities::Archive,
    new_ids: &[ArchiveId],
    page_maps: &[(ArchiveId, HashMap<u32, u32>)],
) {
    if new_ids.is_empty() {
        return;
    }

    // Tags
    for new_id in new_ids {
        if let Some(mut a) = state.repos.archives.get(new_id).await.ok().flatten() {
            let old_tags = a.tags.clone();
            a.tags = original.tags.clone();
            if a.tags != old_tags {
                let _ = state.repos.archives.save(&a).await;
                let _ = lanrurugi_search::indexer::update_tag_indexes(
                    &state.redis.search,
                    new_id.as_str(),
                    &old_tags,
                    &a.tags,
                )
                .await;
            }
        }
    }

    // Categories
    if let Ok(categories) = state.repos.categories.list_all().await {
        for category in categories {
            if !category.archives.contains(&original.id) {
                continue;
            }
            let mut category = category;
            for new_id in new_ids {
                if !category.archives.contains(new_id) {
                    category.archives.push(new_id.clone());
                }
            }
            let _ = state.repos.categories.save(&category).await;
        }
    }

    // Bookmarks: map original page numbers to each split archive's local page numbers.
    if let Ok(bookmarks) = state.bookmarks.list_for_archive(original.id.as_str()).await {
        for (new_id, page_map) in page_maps {
            let mut original_to_local = HashMap::new();
            for (local, original_page) in page_map {
                original_to_local.insert(*original_page, *local);
            }
            for bookmark in &bookmarks {
                let Some(local_page) = original_to_local.get(&bookmark.page) else {
                    continue;
                };
                let _ = state
                    .bookmarks
                    .add(new_id.as_str(), *local_page, bookmark.bookmarked_at)
                    .await;
                if let Some(name) = &bookmark.name {
                    let _ = state
                        .bookmarks
                        .set_name(new_id.as_str(), *local_page, Some(name))
                        .await;
                }
            }
        }
    }

    // Stamps: same mapping.
    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    for (new_id, page_map) in page_maps {
        let mut original_to_local = HashMap::new();
        for (local, original_page) in page_map {
            original_to_local.insert(*original_page, *local);
        }
        let mut stamp_salt = 0u64;
        for stamp_id in &original.stamp_ids {
            if let Ok(Some(stamp)) = state.repos.stamps.get(stamp_id).await {
                let Some(original_page) = stamp_page_from_id(stamp_id.as_str()) else {
                    continue;
                };
                let Some(local_page) = original_to_local.get(&original_page) else {
                    continue;
                };
                let _ = state
                    .repos
                    .stamps
                    .create(
                        new_id,
                        *local_page,
                        &stamp.content,
                        &stamp.position,
                        &stamp.icon,
                        &stamp.rect,
                        now_millis + u64::from(*local_page) + stamp_salt,
                    )
                    .await;
                stamp_salt += 1;
            }
        }
    }
}

async fn run_split_job(
    state: &AppState,
    archive_id: &ArchiveId,
    suggestion: ArchiveSplitSuggestion,
    auth: Option<AuthContext>,
    delete_original_override: Option<bool>,
) -> Result<serde_json::Value, String> {
    let archive = state
        .repos
        .archives
        .get(archive_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "Archive not found.".to_string())?;
    emit_progress(state, archive_id, 0.05, "started", current_memory_kb()).await;
    let original_file_path = PathBuf::from(&archive.file);
    let original_path = resolve_multivolume_path(&original_file_path, &state.library.temp_dir)
        .unwrap_or_else(|| original_file_path.clone());
    let entries = list_all_entries(&original_path).map_err(|e| e.to_string())?;
    let original_pages = list_pages(&original_path).map_err(|e| e.to_string())?;
    let original_page_by_path: HashMap<String, u32> = original_pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), i as u32 + 1))
        .collect();
    let all_files: Vec<String> = entries
        .iter()
        .filter(|e| e.is_regular_file)
        .map(|e| e.name.clone())
        .collect();
    let total_groups = suggestion.split_groups.len().max(1);
    let mut created_groups = 0usize;
    let mut max_memory_kb = current_memory_kb().unwrap_or(0);
    emit_progress(state, archive_id, 0.3, "reading files", current_memory_kb()).await;

    let temp_root = state.library.temp_dir.join(format!(
        "archive-split-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp_root).map_err(|e| e.to_string())?;

    let output_dir = original_file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| state.library.archive_dir.clone());

    // Stream entries into each output ZIP one file at a time. We must NOT load every page's
    // bytes into a HashMap at once: a 500MB+ RAR would OOM the browser/dev machine. `read_entry`
    // reopens the archive per file, which is slower but keeps memory bounded.
    let mut created_files = Vec::new();
    let mut source_by_output: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    for group in &suggestion.split_groups {
        let mut target = output_dir.join(&group.zip_name);
        if target.exists() {
            let crc = crc32fast::hash(group.zip_name.as_bytes());
            let stem = Path::new(&group.zip_name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = Path::new(&group.zip_name)
                .extension()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "zip".to_string());
            target = output_dir.join(format!("{stem}_{crc:08x}.{ext}"));
        }
        let mut target_sources: HashMap<String, String> = HashMap::new();
        let file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        let mut writer = zip::ZipWriter::new(file);
        let mut seen = HashSet::new();

        let group_files: Vec<String> = all_files
            .iter()
            .filter(|path| {
                group
                    .source_dirs
                    .iter()
                    .any(|d| path.starts_with(&format!("{d}/")) || *path == d)
                    || group.source_files.iter().any(|f| f == *path)
            })
            .cloned()
            .collect();
        if group_files.is_empty() {
            writer.finish().map_err(|e| e.to_string())?;
            created_files.push(target.clone());
            created_groups += 1;
            let progress = 0.3 + 0.55 * (created_groups as f32 / total_groups as f32);
            let message = format!("生成压缩包 {created_groups} / {total_groups}");
            emit_progress(state, archive_id, progress, &message, current_memory_kb()).await;
            continue;
        }

        // Read one output group's files in a single libarchive pass (5 opens for a 5-volume
        // archive instead of 922), but keep only that group's bytes in memory at a time.
        let group_bytes = read_entries(&original_path, &group_files).map_err(|e| e.to_string())?;
        let mut bytes_by_path: HashMap<String, Vec<u8>> = HashMap::new();
        for (path, bytes) in group_files.iter().cloned().zip(group_bytes) {
            if let Some(bytes) = bytes {
                bytes_by_path.insert(path, bytes);
            }
        }

        for path in &group_files {
            let Some(bytes) = bytes_by_path.get(path) else {
                continue;
            };
            if is_supported_archive_name(path) {
                // Flatten nested archive contents. Nested archives are small compared to the
                // whole outer archive, so reading them individually is fine.
                let nested_path = temp_root.join(format!("nested-{}", path.replace('/', "-")));
                std::fs::write(&nested_path, bytes).map_err(|e| e.to_string())?;
                let nested_entries = list_all_entries(&nested_path).map_err(|e| e.to_string())?;
                let inner_files: Vec<String> = nested_entries
                    .iter()
                    .filter(|e| e.is_regular_file)
                    .map(|e| e.name.clone())
                    .collect();
                let inner_bytes =
                    read_entries(&nested_path, &inner_files).map_err(|e| e.to_string())?;
                for (inner_name, inner_data) in inner_files.iter().cloned().zip(inner_bytes) {
                    if let Some(inner_data) = inner_data {
                        let output_name = Path::new(&inner_name)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| inner_name.clone());
                        let written_name =
                            write_zip_entry(&mut writer, &output_name, &inner_data, &mut seen)?;
                        target_sources.insert(written_name, path.clone());
                    }
                }
                let _ = std::fs::remove_file(&nested_path);
            } else {
                let output_name = Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let written_name = write_zip_entry(&mut writer, &output_name, bytes, &mut seen)?;
                target_sources.insert(written_name, path.clone());
            }
            if let Some(mem_kb) = current_memory_kb() {
                max_memory_kb = max_memory_kb.max(mem_kb);
            }
        }
        // Drop the group's byte map before opening the next ZIP writer.
        // `group_bytes` was already consumed into `bytes_by_path`; dropping the map is enough.
        drop(bytes_by_path);
        writer.finish().map_err(|e| e.to_string())?;
        source_by_output.insert(target.clone(), target_sources);
        created_files.push(target.clone());
        created_groups += 1;
        let progress = 0.3 + 0.55 * (created_groups as f32 / total_groups as f32);
        let message = format!("生成压缩包 {created_groups} / {total_groups}");
        emit_progress(state, archive_id, progress, &message, current_memory_kb()).await;
    }
    let mut page_maps: HashMap<PathBuf, HashMap<u32, u32>> = HashMap::new();
    for target in &created_files {
        let Ok(pages) = list_pages(target) else {
            continue;
        };
        let sources = source_by_output.get(target).cloned().unwrap_or_default();
        let mut map = HashMap::new();
        for (i, page_name) in pages.iter().enumerate() {
            if let Some(source_path) = sources.get(page_name) {
                if let Some(original_page) = original_page_by_path.get(source_path) {
                    map.insert(i as u32 + 1, *original_page);
                }
            }
        }
        page_maps.insert(target.clone(), map);
    }
    let _ = std::fs::remove_dir_all(&temp_root);
    emit_progress(
        state,
        archive_id,
        0.85,
        "ingesting generated archives",
        current_memory_kb(),
    )
    .await;

    // Ingest every generated archive so they enter the library.
    let mut new_ids = Vec::new();
    let mut new_id_by_path: HashMap<PathBuf, ArchiveId> = HashMap::new();
    for path in &created_files {
        match lanrurugi_scanner::pipeline::ingest_file(
            state.repos.archives.as_ref(),
            &state.redis.config,
            &state.redis.search,
            &state.library.thumb_dir,
            path,
        )
        .await
        {
            Ok(lanrurugi_scanner::pipeline::IngestOutcome::Catalogued { id }) => {
                if !new_ids.contains(&id) {
                    new_ids.push(id.clone());
                }
                new_id_by_path.insert(path.clone(), id.clone());
                let _ = lanrurugi_search::indexer::index_new_archive(
                    &state.redis.search,
                    id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Ok(lanrurugi_scanner::pipeline::IngestOutcome::Unchanged { id }) => {
                if !new_ids.contains(&id) {
                    new_ids.push(id.clone());
                }
                new_id_by_path.insert(path.clone(), id.clone());
                let _ = lanrurugi_search::indexer::index_new_archive(
                    &state.redis.search,
                    id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Ok(lanrurugi_scanner::pipeline::IngestOutcome::Rekeyed { new_id, .. }) => {
                if !new_ids.contains(&new_id) {
                    new_ids.push(new_id.clone());
                }
                new_id_by_path.insert(path.clone(), new_id.clone());
                let _ = lanrurugi_search::indexer::index_new_archive(
                    &state.redis.search,
                    new_id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Ok(lanrurugi_scanner::pipeline::IngestOutcome::Rejected { existing_id, .. }) => {
                if !new_ids.contains(&existing_id) {
                    new_ids.push(existing_id.clone());
                }
                new_id_by_path.insert(path.clone(), existing_id.clone());
                let _ = lanrurugi_search::indexer::index_new_archive(
                    &state.redis.search,
                    existing_id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(?path, error = %e, "archive_split: failed to ingest generated archive");
            }
        }
    }

    // Build per-new-archive page maps (new local page -> original archive page) so bookmarks
    // and stamps can be transferred to the correct positions.
    let mut page_maps_for_ids = Vec::new();
    for path in &created_files {
        if let Some(id) = new_id_by_path.get(path) {
            if let Some(map) = page_maps.get(path) {
                page_maps_for_ids.push((id.clone(), map.clone()));
            }
        }
    }

    // Inherit original metadata: tags, categories, bookmarks, stamps.
    inherit_metadata_to_split_archives(state, &archive, &new_ids, &page_maps_for_ids).await;

    // Move generated archives into the same Tankoubon(s) as the original after ingest.
    let original_tanks: Vec<lanrurugi_core::entities::Grouping> = state
        .repos
        .groupings
        .for_archive(archive_id)
        .await
        .unwrap_or_default();
    for tank in original_tanks {
        let mut updated = tank.clone();
        let old_first = updated.archives.first().cloned();
        updated.archives.retain(|a| a != archive_id);
        for n in &new_ids {
            if !updated.archives.contains(n) {
                updated.archives.push(n.clone());
            }
        }
        let _ = state.repos.groupings.save(&updated).await;
        crate::tankoubons::sync_tankoubon_thumbnail_with_first_archive(
            state,
            &updated,
            old_first.as_ref(),
        )
        .await;
        let joined: Vec<String> = new_ids.iter().map(|n| n.0.clone()).collect();
        if !joined.is_empty() {
            let _ =
                lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &joined, &[])
                    .await;
        }
    }

    let delete_original = match delete_original_override {
        Some(v) => v,
        None => crate::settings::read_archive_split_delete_original_enabled(state).await,
    };

    // Optionally delete the original.
    if delete_original {
        if let Ok(tanks) = state.repos.groupings.for_archive(archive_id).await {
            let others: Vec<lanrurugi_core::entities::Grouping> =
                state.repos.groupings.list_all().await.unwrap_or_default();
            let other_tanks: Vec<String> = others
                .iter()
                .filter(|g| !tanks.iter().any(|t| t.tankid == g.tankid))
                .flat_map(|g| g.archives.iter().map(|a| a.0.clone()))
                .collect();
            let left: Vec<String> = if other_tanks.contains(&archive_id.0) {
                vec![]
            } else {
                vec![archive_id.0.clone()]
            };
            if !left.is_empty() {
                let _ = lanrurugi_search::indexer::sync_tank_membership(
                    &state.redis.search,
                    &[],
                    &left,
                )
                .await;
            }
        }
        let _ = state.repos.archives.delete(archive_id).await;
        // The original archive is gone; drop its id from every category it belonged to. New split
        // archives were already added above by `inherit_metadata_to_split_archives`.
        if let Ok(categories) = state.repos.categories.list_all().await {
            for mut category in categories {
                if category.archives.iter().any(|a| a == archive_id) {
                    category.archives.retain(|a| a != archive_id);
                    let _ = state.repos.categories.save(&category).await;
                }
            }
        }
        let _ = split_repo(state).delete(&archive_id.0).await;
        let _ = std::fs::remove_file(&original_path);
        let _ = lanrurugi_search::indexer::remove_archive_index(
            &state.redis.search,
            &archive_id.0,
            &archive.title,
            &archive.tags,
        )
        .await;
    }

    // Activity: one record for the split operation, plus one record per generated ZIP archive.
    {
        let target = lanrurugi_storage::activity::ActivityTarget {
            id: Some(archive_id.0.clone()),
            label: Some(archive.title.clone()),
            kind: Some("archive".to_string()),
        };
        crate::activity::record_manual(
            state,
            auth.as_ref(),
            lanrurugi_storage::activity::action_types::ARCHIVE_SPLIT_EXECUTE,
            target,
            lanrurugi_storage::activity::Outcome::Success,
            None,
            Some(json!({
                "created_files": created_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                "new_archive_ids": new_ids.iter().map(|n| n.0.clone()).collect::<Vec<_>>(),
                "deleted_original": delete_original,
            })),
        )
        .await;
        for new_id in &new_ids {
            let new_archive = state.repos.archives.get(new_id).await.ok().flatten();
            crate::activity::record_manual(
                state,
                auth.as_ref(),
                lanrurugi_storage::activity::action_types::ARCHIVE_SPLIT_ZIP_CREATED,
                lanrurugi_storage::activity::ActivityTarget {
                    id: Some(new_id.0.clone()),
                    label: new_archive.as_ref().map(|a| a.title.clone()),
                    kind: Some("archive".to_string()),
                },
                lanrurugi_storage::activity::Outcome::Success,
                None,
                Some(json!({
                    "source_archive_id": archive_id.0.clone(),
                })),
            )
            .await;
        }
    }

    Ok(json!({
        "success": true,
        "memory_kb": {
            "peak_kb": max_memory_kb,
            "hwm_kb": peak_memory_kb().unwrap_or(0),
            "final_kb": current_memory_kb().unwrap_or(0),
        },
        "created_files": created_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "new_archive_ids": new_ids.iter().map(|n| n.0.clone()).collect::<Vec<_>>(),
        "deleted_original": delete_original,
    }))
}

/// Called from the server's `new_archive_tx` consumer after metadata plugins run. Cheap local
/// structural pre-filter first; only suspicious archives trigger an LLM call.
pub async fn maybe_auto_analyze(state: AppState, archive_id: String) {
    if !crate::settings::read_archive_split_suggestions_enabled(&state).await {
        return;
    }
    if state.redis.config.ensure_available().await.is_err() {
        return;
    }
    let archive = match state
        .repos
        .archives
        .get(&ArchiveId(archive_id.clone()))
        .await
    {
        Ok(Some(a)) => a,
        _ => return,
    };
    let path = Path::new(&archive.file);
    let Ok(entries) = list_all_entries(path) else {
        return;
    };
    if !is_suspicious_structure(&entries) {
        return;
    }

    let archive_id = ArchiveId(archive_id);
    tokio::spawn(async move {
        let state = Arc::new(state);
        if let Err(e) = generate_suggestion(&state, &archive_id).await {
            tracing::warn!(id = %archive_id, error = %e, "archive_split: auto analysis failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_sfx_split_and_libarchive_reads_merged_zip() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = manifest_dir
            .join("../../test-fixtures/archives/sfx-split/sfxsplit.exe.001")
            .canonicalize()
            .unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "archive-split-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&temp_root).unwrap();

        let resolved = resolve_multivolume_path(&fixture, &temp_root)
            .expect("SFX split volume should be recognized and merged");
        assert!(resolved.exists(), "merged temp archive must exist");
        assert_eq!(resolved.extension().and_then(|e| e.to_str()), Some("zip"));

        let entries =
            list_all_entries(&resolved).expect("libarchive should read the merged SFX ZIP");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.ends_with("page1.png")),
            "merged archive should contain page1.png, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n.ends_with("readme.txt")),
            "merged archive should contain readme.txt, got {names:?}"
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
