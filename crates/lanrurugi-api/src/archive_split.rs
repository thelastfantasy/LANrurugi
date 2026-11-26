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
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use lanrurugi_core::entities::TocEntry;
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

#[derive(Debug, Deserialize, Serialize)]
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
    /// Put every generated volume into one new tankoubon, in the order the split produced them.
    #[serde(default)]
    create_tankoubon: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
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
    // An e-book has no inner files to regroup — its "entries" are one generated name per page.
    if lanrurugi_scanner::archive_format::is_book(&original_path) {
        return Ok(None);
    }
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
    let spec = lanrurugi_llm::CallSpec::new(lanrurugi_llm::FeatureId::ARCHIVE_SPLIT);
    let all_files: HashSet<&str> = entries
        .iter()
        .filter(|e| e.is_regular_file)
        .map(|e| e.name.as_str())
        .collect();
    // The coverage check is deterministic, so a gap gets one repair naming the unassigned files.
    let response = crate::repair::json_chat_checked::<LlmSplitResponse>(
        &state.redis.config,
        &spec,
        &system,
        &user,
        0.2,
        3000,
        |r| validate_coverage(&all_files, &to_suggestions(&r.split_groups)),
    )
    .await?
    .value;
    tracing::info!(id = %archive_id, response = ?response, "archive_split: LLM response");

    let split_groups = to_suggestions(&response.split_groups);

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

fn to_suggestions(groups: &[LlmSplitGroup]) -> Vec<SplitGroupSuggestion> {
    groups
        .iter()
        .map(|g| SplitGroupSuggestion {
            zip_name: g.zip_name.clone(),
            description: g.description.clone(),
            source_dirs: g.source_dirs.clone(),
            source_files: g.source_files.clone(),
        })
        .collect()
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
        let mut missing = missing;
        missing.sort_unstable();
        let shown: Vec<&str> = missing.iter().take(20).copied().collect();
        return Err(format!(
            "LLM split suggestion is incomplete, {} file(s) unassigned: {:?}{}",
            missing.len(),
            shown,
            if missing.len() > shown.len() {
                " …"
            } else {
                ""
            }
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
            q.create_tankoubon.unwrap_or(false),
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

/// Resident memory of this process, in KiB.
fn current_memory_kb() -> Option<u64> {
    if cfg!(target_os = "linux") {
        read_proc_status_kb("VmRSS:")
    } else {
        sysinfo_rss_kb()
    }
}

/// Highest resident memory this process has reached, in KiB.
fn peak_memory_kb() -> Option<u64> {
    if cfg!(target_os = "linux") {
        read_proc_status_kb("VmHWM:")
    } else {
        rusage_peak_kb()
    }
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

fn sysinfo_rss_kb() -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new().ok()?;
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    sys.process(pid).map(|p| p.memory() / 1024)
}

fn rusage_peak_kb() -> Option<u64> {
    // SAFETY: getrusage only writes into the zeroed struct it is given.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)?
    };
    let max_rss = u64::try_from(usage.ru_maxrss).ok()?;
    // macOS reports bytes; Linux and the BSDs report KiB.
    Some(if cfg!(target_os = "macos") {
        max_rss / 1024
    } else {
        max_rss
    })
}

/// Stages the panel renders for itself — the server sends the code, never a sentence, so a split
/// reads the same in every language.
async fn emit_progress(
    state: &AppState,
    archive_id: &ArchiveId,
    progress: f32,
    stage: &str,
    done: Option<usize>,
    total: Option<usize>,
    volume: Option<&str>,
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
            "stage": stage,
            "done": done,
            "total": total,
            // Which volume the fields above describe — the panel prints it next to the stage so a
            // five-volume split shows which one is in flight instead of one anonymous percentage.
            "volume": volume,
            "memory_kb": current_memory_kb(),
        }));
    }
}

/// Drives the bar from work that actually happened, and never lets it walk backwards.
///
/// The count it reports is pages written (out of every page the suggestion claims) while the
/// volumes are being built, then volumes ingested (out of the groups asked for). Reading a group
/// back out of the source archive is reported under `reading` but keeps the page count, because
/// that is still the same unit of work in flight.
struct SplitProgress<'a> {
    state: &'a AppState,
    archive_id: &'a ArchiveId,
    shown: f32,
}

impl<'a> SplitProgress<'a> {
    fn new(state: &'a AppState, archive_id: &'a ArchiveId) -> Self {
        Self {
            state,
            archive_id,
            shown: 0.0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn emit(
        &mut self,
        progress: f32,
        stage: &str,
        done: Option<usize>,
        total: Option<usize>,
        volume: Option<&str>,
    ) {
        self.shown = self.shown.max(progress.clamp(0.0, 1.0));
        emit_progress(
            self.state,
            self.archive_id,
            self.shown,
            stage,
            done,
            total,
            volume,
        )
        .await;
    }
}

/// The one frame the client acts on: how many volumes were created, and whether the source is
/// really gone (the reader page is only closed when it is).
async fn emit_finished(
    state: &AppState,
    archive_id: &ArchiveId,
    progress: f32,
    created: usize,
    total: usize,
    source_deleted: bool,
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
            "stage": "finished",
            "done": created,
            "total": total,
            "memory_kb": memory_kb,
            "source_deleted": source_deleted,
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
    let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
    writer
        .start_file(&out_name, options)
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(writer, data).map_err(|e| e.to_string())?;
    Ok(out_name)
}

/// A staging file name no earlier group took: `第02巻.zip`, `第02巻 (2).zip`, …
fn unique_zip_name(requested: &str, used: &mut HashSet<String>) -> String {
    if used.insert(requested.to_string()) {
        return requested.to_string();
    }
    let path = Path::new(requested);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| requested.to_string());
    let extension = path.extension().map(|e| e.to_string_lossy().to_string());
    for suffix in 2..u32::MAX {
        let candidate = match &extension {
            Some(ext) => format!("{stem} ({suffix}).{ext}"),
            None => format!("{stem} ({suffix})"),
        };
        if used.insert(candidate.clone()) {
            tracing::warn!(
                requested,
                renamed = %candidate,
                "archive_split: two groups were named the same; the second one was renamed"
            );
            return candidate;
        }
    }
    unreachable!("u32 worth of suffixes is not going to be exhausted")
}

fn stamp_page_from_id(stamp_id: &str) -> Option<u32> {
    let rest = stamp_id.strip_prefix("STAMPS_")?;
    let (page, _) = rest.split_once('_')?;
    page.parse().ok()
}

/// The source archive's tags with `date_added:`/`timestamp:` moved forward `index` seconds.
/// Tags without either namespace are returned untouched — those archives sort last either way.
fn stamped_tags(tags: &str, index: usize) -> String {
    if index == 0 || tags.is_empty() {
        return tags.to_string();
    }
    let shift = i64::try_from(index).unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for part in tags.split(',') {
        let trimmed = part.trim();
        let bumped = ["date_added:", "timestamp:"].iter().find_map(|prefix| {
            trimmed
                .strip_prefix(prefix)
                .and_then(|v| v.parse::<i64>().ok())
                .map(|v| format!("{prefix}{}", v.saturating_add(shift)))
        });
        out.push(bumped.unwrap_or_else(|| trimmed.to_string()));
    }
    out.join(",")
}

/// The chapters of `toc` that fall inside one volume, renumbered to that volume's own pages.
fn localise_toc(toc: &[TocEntry], original_to_local: &HashMap<u32, u32>) -> Vec<TocEntry> {
    let mut entries: Vec<TocEntry> = toc
        .iter()
        .filter_map(|entry| {
            original_to_local.get(&entry.page).map(|local| TocEntry {
                page: *local,
                name: entry.name.clone(),
            })
        })
        .collect();
    entries.sort_by_key(|entry| entry.page);
    entries
}

/// Puts a split volume's own name on its record when the pipeline left the id there instead.
///
/// `ingest_file_with_policy` applies `title_filename` only when it actually catalogues the file; a
/// scan that won the race leaves the record with the hash as its title, and `Unchanged` means the
/// split never gets a second chance. Only a title that is still the record's own id is replaced.
async fn ensure_readable_title(state: &AppState, id: &ArchiveId, zip_name: &str) {
    let Some(stem) = Path::new(zip_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
    else {
        return;
    };
    let Ok(Some(record)) = state.repos.archives.get(id).await else {
        return;
    };
    if record.title != id.as_str() {
        return;
    }
    let updated = state
        .repos
        .archives
        .update(id, |archive| {
            archive.title = stem.clone();
            archive.name = stem.clone();
        })
        .await;
    if let Ok(Some(archive)) = updated {
        crate::archives::reindex_title_and_tags(state, record, archive).await;
    }
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

    // Tags — with `date_added`/`timestamp` nudged forward one second per volume, so the split's
    // outputs keep their volume order under any date sort. Inheriting the source's own second for
    // all of them left their order to Redis's set iteration, so the same five volumes came back
    // in a different order on every refresh (reported 2026-11-26).
    for (index, new_id) in new_ids.iter().enumerate() {
        let tags = stamped_tags(&original.tags, index);
        let _ = state
            .repos
            .archives
            .update_then(
                new_id,
                |a| a.tags = tags.clone(),
                |old, new| crate::archives::reindex_title_and_tags(state, old, new),
            )
            .await;
    }

    // Categories
    if let Ok(categories) = state.repos.categories.list_all().await {
        for category in categories {
            if !category.archives.contains(&original.id) {
                continue;
            }
            let _ = state
                .repos
                .categories
                .update(&category.catid, |c| {
                    for new_id in new_ids {
                        if !c.archives.contains(new_id) {
                            c.archives.push(new_id.clone());
                        }
                    }
                })
                .await;
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

    // Table of contents: a chapter's page belongs to exactly one volume, so it is mapped the same
    // way as a bookmark. Without this the source's chapters vanished with the source — the volumes
    // only ever carried their own page ranges, never the names the user gave them.
    for (new_id, page_map) in page_maps {
        let mut original_to_local = HashMap::new();
        for (local, original_page) in page_map {
            original_to_local.insert(*original_page, *local);
        }
        let entries = localise_toc(&original.toc, &original_to_local);
        if entries.is_empty() {
            continue;
        }
        let _ = state
            .repos
            .archives
            .update(new_id, |archive| archive.toc = entries.clone())
            .await;
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
    create_tankoubon: bool,
) -> Result<serde_json::Value, String> {
    let archive = state
        .repos
        .archives
        .get(archive_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "Archive not found.".to_string())?;
    let mut progress = SplitProgress::new(state, archive_id);
    progress.emit(0.02, "started", None, None, None).await;
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
    let mut max_memory_kb = current_memory_kb().unwrap_or(0);
    progress.emit(0.05, "reading", None, None, None).await;

    let temp_root = state.library.temp_dir.join(format!(
        "archive-split-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp_root).map_err(|e| e.to_string())?;

    // Every ZIP is written under `temp_root` and only moves into the library once its content id
    // is known (below), so a half-written output is never visible to the watcher or the pipeline.
    let output_dir = temp_root.clone();

    // Stream entries into each output ZIP one file at a time. We must NOT load every page's
    // bytes into a HashMap at once: a 500MB+ RAR would OOM the browser/dev machine. `read_entry`
    // reopens the archive per file, which is slower but keeps memory bounded.
    let mut created_files = Vec::new();
    // The readable name each staged ZIP carries into the library: it becomes the new archive's
    // title and its sidecar's `original_filename`, which is the only place a split's pretty name
    // survives once the bytes are hash-named.
    let mut staged_names: HashMap<PathBuf, String> = HashMap::new();
    let mut source_by_output: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    // Two groups can be named identically — a "vol 01-05" summary folder sitting beside the volume
    // folders it repeats produces exactly that, and the analyser names a group after its own
    // directory. Staging both into one path truncates the first group's ZIP with the second's
    // bytes and leaves the first group's placement with nothing to move: reported live
    // 2026-11-26 on a 5-volume RAR (第02巻 and 第05巻 vanished, and the source had already been
    // deleted because nothing counted them as lost).
    // Resolve every group's name and file set up front. Two things need the complete plan: the
    // duplicate-name check (a name has to be reserved before the first ZIP is written) and the
    // progress denominator — the bar counts pages written, so `total_pages` must be known before
    // the first page, not discovered group by group.
    let mut used_names: HashSet<String> = HashSet::new();
    let plan: Vec<(String, Vec<String>)> = suggestion
        .split_groups
        .iter()
        .map(|group| {
            let zip_name = unique_zip_name(&group.zip_name, &mut used_names);
            let files: Vec<String> = all_files
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
            (zip_name, files)
        })
        .collect();
    let total_pages: usize = plan.iter().map(|(_, files)| files.len()).sum();
    let mut written_pages = 0usize;
    // One event per percent of the work, not one per page: 922 events over a local SSE stream is
    // noise the UI would coalesce anyway, and each one costs a JSON frame.
    let page_step = (total_pages / 100).max(1);

    for (zip_name, group_files) in &plan {
        let zip_name = zip_name.clone();
        let target = output_dir.join(&zip_name);
        staged_names.insert(target.clone(), zip_name.clone());
        let mut target_sources: HashMap<String, String> = HashMap::new();
        let file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        // ZipWriter emits a header per member; against a bare `File` each one is its own syscall.
        // The bundle writer (`download_manager/bundle.rs`) already buffers this way.
        let mut writer = zip::ZipWriter::new(std::io::BufWriter::with_capacity(1 << 20, file));
        let mut seen = HashSet::new();

        if group_files.is_empty() {
            writer.finish().map_err(|e| e.to_string())?;
            created_files.push(target.clone());
            continue;
        }

        // Read one output group's files in a single libarchive pass (5 opens for a 5-volume
        // archive instead of 922), but keep only that group's bytes in memory at a time. That pass
        // is the slow part of a volume — report it as its own stage, with the page count unchanged
        // so the bar holds still instead of jumping ahead while nothing has been written yet.
        progress
            .emit(
                0.08 + 0.72 * written_pages as f32 / total_pages.max(1) as f32,
                "reading",
                Some(written_pages),
                Some(total_pages),
                Some(&zip_name),
            )
            .await;
        let group_bytes = read_entries(&original_path, group_files).map_err(|e| e.to_string())?;
        let mut bytes_by_path: HashMap<String, Vec<u8>> = HashMap::new();
        for (path, bytes) in group_files.iter().cloned().zip(group_bytes) {
            if let Some(bytes) = bytes {
                bytes_by_path.insert(path, bytes);
            }
        }

        for path in group_files {
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
            written_pages += 1;
            if written_pages.is_multiple_of(page_step) || written_pages == total_pages {
                progress
                    .emit(
                        0.08 + 0.72 * written_pages as f32 / total_pages.max(1) as f32,
                        "generating",
                        Some(written_pages),
                        Some(total_pages),
                        Some(&zip_name),
                    )
                    .await;
            }
        }
        // Drop the group's byte map before opening the next ZIP writer.
        // `group_bytes` was already consumed into `bytes_by_path`; dropping the map is enough.
        drop(bytes_by_path);
        writer.finish().map_err(|e| e.to_string())?;
        source_by_output.insert(target.clone(), target_sources);
        created_files.push(target.clone());
    }
    progress
        .emit(
            0.82,
            "generating",
            Some(total_pages),
            Some(total_pages),
            None,
        )
        .await;
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
    progress
        .emit(0.84, "ingesting", Some(0), Some(total_groups), None)
        .await;

    // Ingest every generated archive so they enter the library. Each ZIP was staged under
    // `temp_root`; here its own bytes decide the id, the bytes move to that id's hash path — the
    // layout every other ingest path uses since 020 — and only then is the record written, with
    // the readable split name kept as the title and in the sidecar (`original_filename`).
    let mut new_ids = Vec::new();
    let mut new_id_by_path: HashMap<PathBuf, ArchiveId> = HashMap::new();
    // What the caller is told was created is the library-side path, not the staging path: the
    // staging copy is gone (moved or dropped as a duplicate) by the time this function returns.
    let mut placed_files: Vec<PathBuf> = Vec::new();
    // Every generated volume that never reached the library, with why. The source archive is only
    // deleted when this stays empty: it is the only copy of those pages left otherwise.
    let mut failed_groups: Vec<serde_json::Value> = Vec::new();
    for (index, path) in created_files.iter().enumerate() {
        let zip_name = staged_names.get(path).cloned().unwrap_or_default();
        // One event per volume: each is a real, discrete step (hash, move, catalogue), and the
        // previous volume's outcome wording is what makes the wait readable.
        progress
            .emit(
                0.84 + 0.15 * index as f32 / total_groups as f32,
                "ingesting",
                Some(index),
                Some(total_groups),
                Some(&zip_name),
            )
            .await;
        if !path.exists() {
            tracing::warn!(
                ?path,
                "archive_split: a staged archive disappeared before it could be placed"
            );
            failed_groups.push(json!({
                "name": zip_name,
                "reason": "the staged archive was gone before it could be moved into the library",
            }));
            continue;
        }
        let id = match lanrurugi_storage::id::size_aware_id(path) {
            Ok(id) => ArchiveId(id),
            Err(e) => {
                tracing::warn!(?path, error = %e, "archive_split: could not hash a generated archive");
                failed_groups.push(json!({ "name": zip_name, "reason": e.to_string() }));
                continue;
            }
        };
        let dest = lanrurugi_storage::paths::hash_layout_path(
            &state.library.archive_dir,
            id.as_str(),
            "zip",
        );
        // One ingest per content id at a time; `locked: true` below tells the pipeline that this
        // task already holds it (taking it again would deadlock).
        let _ingest_guard = lanrurugi_scanner::pipeline::content_lock(id.as_str()).await;
        if dest.exists() {
            // These exact bytes are already on disk (a record may or may not exist): the staged
            // copy is redundant, and the pipeline below resolves the existing archive.
            let _ = tokio::fs::remove_file(path).await;
        } else {
            let copy_options = lanrurugi_core::io::CopyOptions {
                strict_verify: crate::settings::read_ingest_strict_verify(state).await,
            };
            // `content/<bucket>/` exists only once something has landed in it, so a library that
            // never used part of the id space leaves most of the 256 buckets missing. A move into
            // a missing directory fails with `NotFound`, which used to look exactly like the
            // staged ZIP having vanished: that quietly lost 2–3 of every 5 split volumes
            // (2026-11-26). `in_dir` creates the bucket first and survives a prune racing it.
            let placed = lanrurugi_storage::fsutil::in_dir(&dest, || {
                let (from, to) = (path.clone(), dest.clone());
                let opts = copy_options;
                async move {
                    crate::download_manager::place_into_library(&from, &to, false, &opts)
                        .await
                        .map_err(|e| match e {
                            lanrurugi_core::io::CopyError::Io(inner) => inner,
                            other => std::io::Error::other(other.to_string()),
                        })
                }
            })
            .await;
            if let Err(e) = placed {
                tracing::warn!(?path, error = ?e, "archive_split: could not place a generated archive into the library");
                failed_groups.push(json!({ "name": zip_name, "reason": e.to_string() }));
                continue;
            }
        }
        let options = lanrurugi_scanner::pipeline::IngestOptions {
            // Identical bytes elsewhere in the library mean these pages are already catalogued;
            // the split reuses that archive rather than overwriting it.
            duplicate_policy: lanrurugi_scanner::pipeline::DuplicatePolicy::Reject,
            // The file name on disk is the id now, so the filename-collision rule has nothing to
            // compare; the split's own name reaches the record and the sidecar through `title_filename`.
            intended_filename: None,
            title_filename: (!zip_name.is_empty()).then_some(zip_name.as_str()),
            defer_file_path: false,
            locked: true,
            blake3: None,
        };
        match lanrurugi_scanner::pipeline::ingest_file_with_policy(
            state.repos.archives.as_ref(),
            &state.redis.config,
            &state.redis.search,
            &state.equivalence,
            &state.library.thumb_dir,
            &dest,
            options,
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
                    &state.equivalence,
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
                    &state.equivalence,
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
                    &state.equivalence,
                    new_id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Ok(lanrurugi_scanner::pipeline::IngestOutcome::Rejected { existing_id, .. }) => {
                // A record that lives by name (under `_byname/`) keeps its own path; the copy this
                // loop just moved into the hash layout is then a redundant duplicate and would
                // linger forever (nothing sweeps bucket files without a record).
                if let Ok(Some(existing)) = state.repos.archives.get(&existing_id).await {
                    if Path::new(&existing.file) != dest.as_path() {
                        let _ = tokio::fs::remove_file(&dest).await;
                    }
                }
                if !new_ids.contains(&existing_id) {
                    new_ids.push(existing_id.clone());
                }
                new_id_by_path.insert(path.clone(), existing_id.clone());
                let _ = lanrurugi_search::indexer::index_new_archive(
                    &state.redis.search,
                    &state.equivalence,
                    existing_id.as_str(),
                    &archive.title,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(?path, error = %e, "archive_split: failed to ingest generated archive");
                failed_groups.push(json!({ "name": zip_name, "reason": e.to_string() }));
            }
        }
        if dest.exists() && !placed_files.contains(&dest) {
            placed_files.push(dest.clone());
        }
        // A concurrent library scan can catalogue the file the moment it lands in the bucket, and
        // the pipeline then answers `Unchanged` — the record exists, so the split's own title never
        // reaches it and the reader sees a hash as the volume's name. Only the fallback title (the
        // record's own id) is replaced, so a real name is never trodden on.
        if let Some(id) = new_id_by_path.get(path) {
            if !zip_name.is_empty() {
                ensure_readable_title(state, id, &zip_name).await;
            }
        }
    }

    // Every staged ZIP has either moved into the library or been dropped as a duplicate; the
    // staging directory itself is now empty. The `finished` frame is sent at the very end (after
    // the source is gone and every record is written), because the reader navigates away on it.
    let _ = std::fs::remove_dir_all(&temp_root);

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
        let mut old_first = None;
        let updated = state
            .repos
            .groupings
            .update(&tank.tankid, |g| {
                old_first = g.archives.first().cloned();
                g.archives.retain(|a| a != archive_id);
                for n in &new_ids {
                    if !g.archives.contains(n) {
                        g.archives.push(n.clone());
                    }
                }
            })
            .await;
        let Ok(Some(updated)) = updated else {
            continue;
        };
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

    let wants_delete_original = match delete_original_override {
        Some(v) => v,
        None => crate::settings::read_archive_split_delete_original_enabled(state).await,
    };
    // A partial split must keep the source: the volumes that failed exist nowhere else.
    let delete_original = wants_delete_original && failed_groups.is_empty();
    if wants_delete_original && !delete_original {
        tracing::warn!(
            failed = failed_groups.len(),
            "archive_split: keeping the source archive because not every volume was created"
        );
    }

    // Optionally delete the original.
    if delete_original {
        // `purge_one_archive` is the one function every other delete goes through: the deletion
        // journal (028), the record, Tankoubon membership, patch provenance, activity, the search
        // index, categories, bookmarks, stamps, the download queue, and then the bytes, the
        // sidecar and the thumbnails. This branch used to duplicate a hand-picked subset of that
        // and silently skipped the rest — the source archive's sidecar in particular outlived it.
        let outcome = crate::archives::purge_one_archive(
            state,
            archive_id,
            auth.as_ref(),
            crate::archives::PurgeOptions {
                journal_source: "split",
                ..crate::archives::PurgeOptions::SINGLE
            },
        )
        .await;
        match outcome {
            crate::archives::DeleteOneOutcome::Error(e) => {
                tracing::warn!(%archive_id, error = %e, "archive_split: could not delete the source archive");
            }
            _ => {
                // A multivolume source's record points at one volume; the rest are this job's
                // own leftovers, and nothing else knows they belong together.
                if original_path != original_file_path {
                    let _ = std::fs::remove_file(&original_path);
                }
            }
        }
        let _ = split_repo(state).delete(&archive_id.0).await;
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
                "created_files": placed_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                "new_archive_ids": new_ids.iter().map(|n| n.0.clone()).collect::<Vec<_>>(),
                "failed_groups": failed_groups.clone(),
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

    // One tankoubon holding every volume, in split order — the common follow-up to splitting a
    // multi-volume archive, and the reason the panel offers the checkbox at all.
    let tankoubon_id = if create_tankoubon && !new_ids.is_empty() {
        let mut seen_ids: HashSet<&str> = HashSet::new();
        let ordered: Vec<ArchiveId> = new_ids
            .iter()
            .filter(|id| seen_ids.insert(id.as_str()))
            .cloned()
            .collect();
        match crate::tankoubons::create_tankoubon_with(
            state,
            &archive.title,
            "",
            &archive.tags,
            ordered,
        )
        .await
        {
            Ok(tankid) => Some(tankid.0),
            Err(e) => {
                tracing::warn!(%archive_id, error = %e, "archive_split: could not create the tankoubon");
                failed_groups.push(json!({
                    "name": "tankoubon",
                    "reason": format!("the volumes were created but the tankoubon failed: {e}"),
                }));
                None
            }
        }
    } else {
        None
    };

    // Last frame: everything the reader needs to finish up. `source_deleted` is the authoritative
    // answer to "is this reader page still valid" — the client must not guess it from the counts,
    // because two groups can dedup into one record while the source is still deleted.
    emit_finished(
        state,
        archive_id,
        if total_groups == 0 {
            1.0
        } else {
            (new_ids.len() as f32 / total_groups as f32).min(1.0)
        },
        new_ids.len(),
        total_groups,
        delete_original,
        current_memory_kb(),
    )
    .await;

    Ok(json!({
        "success": true,
        "memory_kb": {
            "peak_kb": max_memory_kb,
            "hwm_kb": peak_memory_kb().unwrap_or(0),
            "final_kb": current_memory_kb().unwrap_or(0),
        },
        "created_files": placed_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "new_archive_ids": new_ids.iter().map(|n| n.0.clone()).collect::<Vec<_>>(),
        "failed_groups": failed_groups,
        "deleted_original": delete_original,
        "original_kept_for": new_ids.iter().map(|n| n.0.clone()).collect::<Vec<_>>(),
        "tankoubon_id": tankoubon_id,
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
    if lanrurugi_scanner::archive_format::is_book(path) {
        return;
    }
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
    /// The executor writes each output under `temp_root` and then places it with
    /// `hash_layout_path(..., "zip")`. The pipeline only writes a sidecar when
    /// `is_hash_layout_path` accepts the final path — so the two must agree on the exact shape,
    /// or a split's readable name would survive nowhere but the Redis record.
    #[test]
    fn a_split_output_path_is_where_the_pipeline_writes_a_sidecar() {
        let library = std::path::Path::new("/srv/library");
        let id = "0123456789abcdef0123456789abcdef01234567";
        let dest = lanrurugi_storage::paths::hash_layout_path(library, id, "zip");
        assert!(lanrurugi_storage::paths::is_hash_layout_path(
            library, &dest, id
        ));
        assert_eq!(
            lanrurugi_storage::paths::sidecar_path(library, id),
            dest.with_extension("json")
        );
    }

    /// Every volume used to inherit the source's own `date_added` second, which left their order
    /// to Redis's set iteration — the same five volumes came back in a different order on every
    /// refresh. Each one now sits one second after the previous.
    #[test]
    fn split_outputs_are_one_second_apart_so_a_date_sort_is_stable() {
        let tags = "date_added:1700000000,artist:X,timestamp:1699999999";
        assert_eq!(stamped_tags(tags, 0), tags);
        assert_eq!(
            stamped_tags(tags, 3),
            "date_added:1700000003,artist:X,timestamp:1700000002"
        );
        // No date namespace at all: leave it alone rather than inventing one.
        assert_eq!(stamped_tags("artist:X", 2), "artist:X");
    }

    /// Two split groups can ask for the same output name (a "vol 01-05" summary folder beside the
    /// volume folders it repeats). Staging both into one path truncated the first group's ZIP with
    /// the second's bytes, and the first group's placement then found nothing to move — 2 of 5
    /// volumes vanished that way on a real archive.
    #[test]
    fn a_chapter_lands_only_in_the_volume_that_holds_its_page() {
        let toc = vec![
            TocEntry {
                page: 1,
                name: "c1".to_string(),
            },
            TocEntry {
                page: 400,
                name: "c2".to_string(),
            },
            TocEntry {
                page: 513,
                name: "c19".to_string(),
            },
        ];
        // Volume two starts at original page 368; page 513 is its 146th.
        let mut second_volume = HashMap::new();
        for page in 368..548 {
            second_volume.insert(page, page - 368 + 1);
        }
        let entries = localise_toc(&toc, &second_volume);
        // Page 400 is the volume's 33rd page; page 513 its 146th; page 1 is not in this volume.
        assert_eq!(
            entries,
            vec![
                TocEntry {
                    page: 33,
                    name: "c2".to_string()
                },
                TocEntry {
                    page: 146,
                    name: "c19".to_string()
                },
            ]
        );
        // The first volume keeps page 1 and nothing else.
        let mut first_volume = HashMap::new();
        for page in 1..368 {
            first_volume.insert(page, page);
        }
        assert_eq!(localise_toc(&toc, &first_volume), vec![toc[0].clone()]);
    }

    #[test]
    fn a_repeated_output_name_is_renamed_instead_of_overwritten() {
        let mut used: HashSet<String> = HashSet::new();
        assert_eq!(unique_zip_name("第02巻.zip", &mut used), "第02巻.zip");
        assert_eq!(unique_zip_name("第02巻.zip", &mut used), "第02巻 (2).zip");
        assert_eq!(unique_zip_name("第02巻.zip", &mut used), "第02巻 (3).zip");
        assert_eq!(unique_zip_name("第05巻", &mut used), "第05巻");
        assert_eq!(unique_zip_name("第05巻", &mut used), "第05巻 (2)");
        assert_eq!(used.len(), 5);
    }

    #[test]
    fn portable_memory_readings_agree_with_proc() {
        let rss = super::sysinfo_rss_kb().expect("rss via sysinfo");
        let peak = super::rusage_peak_kb().expect("peak via getrusage");
        assert!(rss > 0 && peak >= rss / 2, "rss {rss} KiB, peak {peak} KiB");
        if let Some(proc_rss) = super::read_proc_status_kb("VmRSS:") {
            // Same process, read moments apart: within a factor of two.
            assert!(
                rss <= proc_rss * 2 && proc_rss <= rss * 2,
                "{rss} vs {proc_rss}"
            );
        }
    }

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
