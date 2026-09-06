//! `database/scripts` endpoint group — additive, no legacy REST contract equivalent. Only
//! `Plugin/Scripts/FolderToCat.pm` ("Subfolders to Categories") stays a native endpoint here: it
//! walks the entire archive directory tree (`std::fs::read_dir` recursion), which is I/O-heavy
//! enough to be worth keeping in Rust rather than round-tripping every path through a Deno
//! subprocess. `SourceFinder.pm`/`nHentaiSourceConverter.pm` are real `script`-type plugins now
//! (`plugins/script/{sourcefinder,nhentaisourceconverter}.ts`, run through the same `/plugins/use`
//! machinery every other plugin uses) — see `lanrurugi-api::plugins`'s own `existing_archive_id`/
//! `archives` host-side injection for why those two didn't need to stay native despite also
//! touching every archive's tags.
//!
//! This module also hosts the new native **Subfolders to Tankoubons** maintenance endpoint, the
//! direct “make a single-volume/Tankoubon from each directory” counterpart to FolderToCat.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use lanrurugi_core::entities::{Category, Grouping};
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_llm::LlmClient;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/database/scripts/subfolders-to-categories",
            post(subfolders_to_categories),
        )
        .route(
            "/database/scripts/subfolders-to-tankoubons",
            post(subfolders_to_tankoubons),
        )
}

#[derive(Debug, Deserialize, Default)]
pub struct SubfoldersToCategoriesParams {
    #[serde(default)]
    delete_old_categories: bool,
    #[serde(default)]
    by_top_folder: bool,
}

fn walk_subfolders(
    root: &Path,
    current: &Path,
    by_top_folder: bool,
    out: &mut HashMap<String, Vec<PathBuf>>,
) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_subfolders(root, &path, by_top_folder, out);
            continue;
        }
        if current == root {
            continue; // direct children of the library root are excluded, matching legacy
        }
        if !lanrurugi_scanner::watcher::is_watched_archive_path(&path) {
            continue;
        }
        let folder_name = if by_top_folder {
            current
                .strip_prefix(root)
                .ok()
                .and_then(|rel| rel.components().next())
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
        } else {
            current
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        };
        let Some(folder_name) = folder_name else {
            continue;
        };
        out.entry(folder_name).or_default().push(path);
    }
}

/// Recursively collects every archive file under `dir`, including files inside nested
/// subdirectories. Used only after a first-level subdirectory has already been selected as the
/// grouping unit.
fn collect_archives_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_archives_recursive(&path, out);
        } else if lanrurugi_scanner::watcher::is_watched_archive_path(&path) {
            out.push(path);
        }
    }
}

/// Tankoubon-specific directory walk: only the **first-level subdirectories** of the library root
/// are grouping units. For every such subdirectory, all archives under it — including those in
/// deeper nested subdirectories — are collected into that one Tankoubon. Nested subdirectories do
/// not become separate Tankoubons.
fn walk_first_level_subfolders(root: &Path, out: &mut HashMap<String, Vec<PathBuf>>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(folder_name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let mut files = Vec::new();
        collect_archives_recursive(&dir, &mut files);
        if !files.is_empty() {
            out.entry(folder_name).or_default().extend(files);
        }
    }
}

/// `POST /database/scripts/subfolders-to-categories` — scans the archive folder and creates one
/// static Category per subfolder that directly contains archives (`FolderToCat.pm::run_script`).
///
/// Reports its own wall-clock `elapsed_ms` in the response (directory walk through category
/// creation, inclusive) so it can be directly compared against `plugins/script/foldertocat.ts` —
/// a real `.ts` script plugin doing the identical directory walk + grouping logic itself (via
/// `Deno.readDir`, not host-injected paths — the user wanted a genuine head-to-head, not a
/// contrived one where the plugin is handed pre-walked data), used to gauge the real overhead of
/// running this kind of I/O-heavy whole-library scan through the Deno-subprocess plugin sandbox
/// versus native Rust.
async fn subfolders_to_categories(
    State(state): State<AppState>,
    Query(params): Query<SubfoldersToCategoriesParams>,
) -> Response {
    let start = Instant::now();
    if params.delete_old_categories {
        let categories = match state.repos.categories.list_all().await {
            Ok(c) => c,
            Err(e) => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "subfolders_to_categories",
                    e.to_string(),
                )
            }
        };
        for category in categories.iter().filter(|c| c.search.is_none()) {
            let _ = state.repos.categories.delete(&category.catid).await;
        }
    }

    let mut subfolders = HashMap::new();
    walk_subfolders(
        &state.library.archive_dir,
        &state.library.archive_dir,
        params.by_top_folder,
        &mut subfolders,
    );

    let id_by_path: HashMap<PathBuf, ArchiveId> = match state.repos.archives.list_all().await {
        Ok(all) => all
            .into_iter()
            .map(|a| (PathBuf::from(a.file), a.id))
            .collect(),
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "subfolders_to_categories",
                e.to_string(),
            )
        }
    };

    // `CategoryRepository::list_all` discovers categories via a `SET_??????????` key glob
    // (verified: `Model/Category.pm`) — exactly a 10-digit timestamp, no extra suffix — so every
    // catid generated here must match that shape exactly or the category becomes invisible to
    // every other endpoint that lists categories.
    let mut next_candidate = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut created_categories = Vec::new();
    for (folder_name, paths) in subfolders {
        let mut catid = lanrurugi_core::ids::CategoryId(format!("SET_{next_candidate}"));
        while state
            .repos
            .categories
            .get(&catid)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            next_candidate += 1;
            catid = lanrurugi_core::ids::CategoryId(format!("SET_{next_candidate}"));
        }
        next_candidate += 1;

        let archive_ids: Vec<ArchiveId> = paths
            .into_iter()
            .filter_map(|path| id_by_path.get(&path).cloned())
            .collect();
        let category = Category {
            catid: catid.clone(),
            name: folder_name,
            search: None,
            archives: archive_ids,
            pinned: false,
            visible_to_guest: false,
        };
        if state.repos.categories.save(&category).await.is_ok() {
            created_categories.push(catid.into_string());
        }
    }

    axum::Json(json!({
        "operation": "subfolders_to_categories",
        "success": 1,
        "created_categories": created_categories,
        "elapsed_ms": start.elapsed().as_millis(),
    }))
    .into_response()
}

/// One directory group discovered by [`walk_first_level_subfolders`], with only the info the
/// Tankoubon-creation path needs: the display folder name, all member archives, and their
/// relative paths (handy for LLM naming and for building a human-readable directory tree).
struct FolderTankGroup {
    folder: String,
    files: Vec<String>,
    titles: Vec<String>,
    archive_ids: Vec<ArchiveId>,
}

/// The subset of [`FolderTankGroup`] sent to the LLM. Serde shape matches the JSON prompt.
#[derive(Serialize)]
struct LlmGroupInput {
    folder: String,
    files: Vec<String>,
    titles: Vec<String>,
}

#[derive(Deserialize)]
struct LlmTankSuggestion {
    folder: String,
    #[serde(default)]
    tank_name: Option<String>,
    #[serde(default)]
    artists: Vec<String>,
    #[serde(default)]
    circles: Vec<String>,
}

#[derive(Deserialize)]
struct LlmTankBatch {
    #[serde(default)]
    suggestions: Vec<LlmTankSuggestion>,
}

/// Best-effort LLM enrichment: batch-names every discovered folder group and, where the model is
/// confident, returns an `artist:`/`circle:` author tag. Never fatal — `Ok(None)` is used for
/// “no key / call failed / unparseable”, and the caller falls back to folder names.
async fn infer_tank_suggestions(
    groups: &[FolderTankGroup],
    state: &AppState,
) -> Option<HashMap<String, LlmTankSuggestion>> {
    if groups.is_empty() {
        return None;
    }
    if state.redis.config.ensure_available().await.is_err() {
        return None;
    }

    let inputs: Vec<LlmGroupInput> = groups
        .iter()
        .map(|g| LlmGroupInput {
            folder: g.folder.clone(),
            files: g.files.clone(),
            titles: g.titles.clone(),
        })
        .collect();

    let system = crate::llm_prompts::subfolders_to_tankoubons_system();

    let user = serde_json::to_string(&json!({ "groups": inputs })).unwrap_or_default();
    match state
        .redis
        .config
        .json_chat::<LlmTankBatch>(&system, &user, 0.3, 2000)
        .await
    {
        Ok(batch) => {
            let mut map = HashMap::new();
            for suggestion in batch.suggestions {
                map.entry(suggestion.folder.clone()).or_insert(suggestion);
            }
            Some(map)
        }
        Err(e) => {
            tracing::warn!(error = %e, "subfolders_to_tankoubons: LLM enrichment failed, falling back to folder names");
            None
        }
    }
}

/// Creates one Tankoubon from a folder group, keeping the search/title/tag indexes and the
/// first-archive thumbnails in the same state as the manual `PUT /tankoubons` flow would.
async fn create_tankoubon_from_folder(
    state: &AppState,
    archive_ids: Vec<ArchiveId>,
    name: String,
    tags: String,
) -> Result<TankId, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut candidate = TankId(format!("TANK_{now}"));
    let mut attempt = now;
    while state
        .repos
        .groupings
        .get(&candidate)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        attempt += 1;
        candidate = TankId(format!("TANK_{attempt}"));
    }

    let grouping = Grouping {
        tankid: candidate.clone(),
        name: name.clone(),
        summary: String::new(),
        tags: tags.clone(),
        progress: 0,
        archives: archive_ids.clone(),
        thumbnail_manual: false,
        thumbnail_source_archive: None,
        thumbnail_source_page: None,
        chapter_names: Default::default(),
        created_at: Some(now),
        updated_at: Some(now),
    };

    state
        .repos
        .groupings
        .save(&grouping)
        .await
        .map_err(|e| e.to_string())?;

    if let Err(e) =
        lanrurugi_search::indexer::update_title_index(&state.redis.search, &candidate, "", &name)
            .await
    {
        tracing::warn!(%candidate, error = %e, "failed to write new tank title index entry");
    }
    if let Err(e) =
        lanrurugi_search::indexer::add_tank_to_index(&state.redis.search, &candidate).await
    {
        tracing::warn!(%candidate, error = %e, "failed to add new tank to search index");
    }
    if !tags.is_empty() {
        if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
            &state.redis.search,
            &candidate,
            "",
            &tags,
        )
        .await
        {
            tracing::warn!(%candidate, error = %e, "failed to index new tank tags");
        }
    }
    let joined: Vec<String> = archive_ids.iter().map(|a| a.0.clone()).collect();
    if let Err(e) =
        lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &joined, &[]).await
    {
        tracing::warn!(%candidate, error = %e, "failed to sync tank membership in search index");
    }

    crate::tankoubons::sync_tankoubon_thumbnail_with_first_archive(state, &grouping, None).await;

    Ok(candidate)
}

/// `POST /database/scripts/subfolders-to-tankoubons` — scans only the **first-level subfolders**
/// of the archive folder and creates one Tankoubon per such subfolder. All archives under that
/// first-level subdirectory (including archives in deeper nested subdirectories) are added to the
/// same Tankoubon; nested subdirectories do not create their own Tankoubons. Duplicate names are
/// allowed; this is a non-destructive action and never deletes or deduplicates existing
/// Tankoubons.
///
/// When a DeepSeek API key is configured, the endpoint best-effort uses the LLM to infer a better
/// Tankoubon name and an `artist:`/`circle:` author tag. Without a key, or when the LLM call
/// fails, it falls back to the raw folder name and skips the author tag.
async fn subfolders_to_tankoubons(State(state): State<AppState>) -> Response {
    let start = Instant::now();
    if !crate::settings::read_subfolders_to_tankoubons_enabled(&state).await {
        return axum::Json(json!({
            "operation": "subfolders_to_tankoubons",
            "success": 1,
            "enabled": false,
            "created_tankoubons": [],
            "llm_used": false,
            "elapsed_ms": start.elapsed().as_millis(),
        }))
        .into_response();
    }

    let mut subfolders = HashMap::new();
    walk_first_level_subfolders(&state.library.archive_dir, &mut subfolders);

    let all_archives = match state.repos.archives.list_all().await {
        Ok(all) => all,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "subfolders_to_tankoubons",
                e.to_string(),
            )
        }
    };
    let id_by_path: HashMap<PathBuf, ArchiveId> = all_archives
        .iter()
        .map(|a| (PathBuf::from(a.file.clone()), a.id.clone()))
        .collect();
    let title_by_path: HashMap<PathBuf, String> = all_archives
        .into_iter()
        .map(|a| (PathBuf::from(a.file), a.title))
        .collect();

    let mut groups: Vec<FolderTankGroup> = Vec::new();
    for (folder_name, mut paths) in subfolders {
        paths.sort();
        let mut archive_ids = Vec::new();
        let mut files = Vec::new();
        let mut titles = Vec::new();
        for path in paths {
            if let Some(id) = id_by_path.get(&path) {
                archive_ids.push(id.clone());
                let rel = path
                    .strip_prefix(&state.library.archive_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                files.push(rel);
                titles.push(title_by_path.get(&path).cloned().unwrap_or_default());
            }
        }
        if archive_ids.is_empty() {
            continue;
        }
        groups.push(FolderTankGroup {
            folder: folder_name,
            files,
            titles,
            archive_ids,
        });
    }
    groups.sort_by(|a, b| a.folder.cmp(&b.folder));

    let llm_enrichments = infer_tank_suggestions(&groups, &state).await;
    let llm_used = llm_enrichments.is_some();

    let mut created_tankoubons = Vec::new();
    for group in groups {
        let suggestion = llm_enrichments.as_ref().and_then(|m| m.get(&group.folder));
        let name = suggestion
            .and_then(|s| s.tank_name.as_deref())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| group.folder.clone());
        let mut tag_list = Vec::new();
        if let Some(s) = suggestion {
            for artist in &s.artists {
                if !artist.trim().is_empty() {
                    tag_list.push(format!("artist:{}", artist.trim()));
                }
            }
            for circle in &s.circles {
                if !circle.trim().is_empty() {
                    tag_list.push(format!("circle:{}", circle.trim()));
                }
            }
        }
        let tags = tag_list.join(",");

        match create_tankoubon_from_folder(&state, group.archive_ids, name, tags).await {
            Ok(id) => created_tankoubons.push(id.into_string()),
            Err(e) => {
                tracing::warn!(folder = %group.folder, error = %e, "subfolders_to_tankoubons: failed to create tankoubon");
            }
        }
    }

    axum::Json(json!({
        "operation": "subfolders_to_tankoubons",
        "success": 1,
        "enabled": true,
        "created_tankoubons": created_tankoubons,
        "llm_used": llm_used,
        "elapsed_ms": start.elapsed().as_millis(),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_suggestion_parse_accepts_missing_optional_fields() {
        let raw = r#"{"suggestions":[{"folder":"My Series","tank_name":"My Series"}]}"#;
        let parsed: LlmTankBatch = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.suggestions.len(), 1);
        assert_eq!(parsed.suggestions[0].folder, "My Series");
        assert_eq!(
            parsed.suggestions[0].tank_name.as_deref(),
            Some("My Series")
        );
        assert!(parsed.suggestions[0].artists.is_empty());
        assert!(parsed.suggestions[0].circles.is_empty());
    }

    #[test]
    fn llm_suggestion_parse_accepts_multiple_artists_and_circle() {
        let raw = r#"{"suggestions":[{"folder":"A","artists":["Author 1","Author 2"],"circles":["Circle"]}]}"#;
        let parsed: LlmTankBatch = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.suggestions[0].artists.len(), 2);
        assert_eq!(parsed.suggestions[0].circles.len(), 1);
    }
}
