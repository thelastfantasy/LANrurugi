//! `database/scripts` endpoint group — additive, no legacy REST contract equivalent. Only
//! `Plugin/Scripts/FolderToCat.pm` ("Subfolders to Categories") stays a native endpoint here: it
//! walks the entire archive directory tree (`std::fs::read_dir` recursion), which is I/O-heavy
//! enough to be worth keeping in Rust rather than round-tripping every path through a Deno
//! subprocess. `SourceFinder.pm`/`nHentaiSourceConverter.pm` are real `script`-type plugins now
//! (`plugins/script/{sourcefinder,nhentaisourceconverter}.ts`, run through the same `/plugins/use`
//! machinery every other plugin uses) — see `lanrurugi-api::plugins`'s own `existing_archive_id`/
//! `archives` host-side injection for why those two didn't need to stay native despite also
//! touching every archive's tags.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use lanrurugi_core::entities::Category;
use serde::Deserialize;
use serde_json::json;

use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/database/scripts/subfolders-to-categories",
        post(subfolders_to_categories),
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

    let id_by_path: HashMap<PathBuf, lanrurugi_core::ids::ArchiveId> =
        match state.repos.archives.list_all().await {
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

        let archive_ids: Vec<lanrurugi_core::ids::ArchiveId> = paths
            .into_iter()
            .filter_map(|path| id_by_path.get(&path).cloned())
            .collect();
        let category = Category {
            catid: catid.clone(),
            name: folder_name,
            search: None,
            archives: archive_ids,
            pinned: false,
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
