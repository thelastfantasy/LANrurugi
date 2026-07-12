//! `database/scripts` endpoint group — additive, no legacy REST contract equivalent (legacy
//! exposes these as "script"-type plugins run through the `/plugins/use` machinery, but they need
//! whole-library/whole-database access no Deno-sandboxed plugin process gets, so they're native
//! endpoints here instead of `.ts` plugins). Faithful ports of three purely-local legacy scripts
//! (no third-party site access — see project scope decision):
//! - `Plugin/Scripts/FolderToCat.pm` ("Subfolders to Categories")
//! - `Plugin/Scripts/SourceFinder.pm` ("Source Finder")
//! - `Plugin/Scripts/nHentaiSourceConverter.pm` ("nHentai Source Converter")

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    Router::new()
        .route(
            "/database/scripts/subfolders-to-categories",
            post(subfolders_to_categories),
        )
        .route("/database/scripts/source-finder", post(source_finder))
        .route(
            "/database/scripts/nhentai-source-converter",
            post(nhentai_source_converter),
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
async fn subfolders_to_categories(
    State(state): State<AppState>,
    Query(params): Query<SubfoldersToCategoriesParams>,
) -> Response {
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

    let id_by_path: HashMap<PathBuf, String> = match state.repos.archives.list_all().await {
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
        let mut catid = format!("SET_{next_candidate}");
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
            catid = format!("SET_{next_candidate}");
        }
        next_candidate += 1;

        let archive_ids: Vec<String> = paths
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
            created_categories.push(catid);
        }
    }

    axum::Json(json!({
        "operation": "subfolders_to_categories",
        "success": 1,
        "created_categories": created_categories,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SourceFinderParams {
    url: String,
}

fn trim_url(url: &str) -> String {
    let url = url.trim();
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let without_www = without_scheme
        .strip_prefix("www.")
        .unwrap_or(without_scheme);
    let without_query = without_www.split('?').next().unwrap_or(without_www);
    without_query.trim_end_matches('/').to_string()
}

/// `POST /database/scripts/source-finder` — searches every archive's `source:` tag for one
/// matching `url` (`SourceFinder.pm::run_script`), including the E-Hentai/ExHentai domain-alias
/// special case. Legacy maintains a dedicated `LRR_URLMAP` index for this; this scans all
/// archives' tags on demand instead (same simplification already used by `GET /database/stats` —
/// correct, just not index-accelerated, which is acceptable at personal-library scale).
async fn source_finder(
    State(state): State<AppState>,
    Query(params): Query<SourceFinderParams>,
) -> Response {
    let trimmed = trim_url(&params.url);
    if trimmed.is_empty() {
        return axum::Json(json!({ "operation": "source_finder", "success": 0, "error": "No URL specified!", "total": 0 })).into_response();
    }

    let mut candidates = vec![trimmed.clone()];
    if let Some(rest) = trimmed.strip_prefix("exhentai.org/") {
        candidates.push(format!("e-hentai.org/{rest}"));
    } else if let Some(rest) = trimmed.strip_prefix("e-hentai.org/") {
        candidates.push(format!("exhentai.org/{rest}"));
    }

    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "source_finder",
                e.to_string(),
            )
        }
    };

    for archive in &archives {
        for tag in archive.tags.split(',') {
            let Some(source) = tag.trim().strip_prefix("source:") else {
                continue;
            };
            let source_trimmed = trim_url(source);
            if candidates.iter().any(|c| c == &source_trimmed) {
                return axum::Json(json!({ "operation": "source_finder", "success": 1, "total": 1, "id": archive.id })).into_response();
            }
        }
    }

    axum::Json(json!({ "operation": "source_finder", "success": 0, "error": "URL not found in database.", "total": 0 })).into_response()
}

/// `POST /database/scripts/nhentai-source-converter` — rewrites `source:{id}` tags with 6 or
/// fewer digits into `source:nhentai.net/g/{id}` (`nHentaiSourceConverter.pm::run_script`).
async fn nhentai_source_converter(State(state): State<AppState>) -> Response {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "nhentai_source_converter",
                e.to_string(),
            )
        }
    };

    let mut modified = 0u32;
    for mut archive in archives {
        let mut changed = false;
        let new_tags: Vec<String> = archive
            .tags
            .split(',')
            .map(|raw| {
                let tag = raw.trim();
                if let Some(digits) = tag.strip_prefix("source:") {
                    if digits.len() <= 6
                        && !digits.is_empty()
                        && digits.bytes().all(|b| b.is_ascii_digit())
                    {
                        changed = true;
                        return format!("source:nhentai.net/g/{digits}");
                    }
                }
                tag.to_string()
            })
            .collect();

        if changed {
            archive.tags = new_tags.join(", ");
            if state.repos.archives.save(&archive).await.is_ok() {
                modified += 1;
            }
        }
    }

    axum::Json(
        json!({ "operation": "nhentai_source_converter", "success": 1, "modified": modified }),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_url_strips_scheme_www_query_and_trailing_slash() {
        assert_eq!(
            trim_url("https://www.e-hentai.org/g/123/abc/?p=1"),
            "e-hentai.org/g/123/abc"
        );
        assert_eq!(
            trim_url("http://e-hentai.org/g/123/abc/"),
            "e-hentai.org/g/123/abc"
        );
        assert_eq!(
            trim_url("  e-hentai.org/g/123/abc  "),
            "e-hentai.org/g/123/abc"
        );
    }

    #[test]
    fn trim_url_leaves_bare_paths_alone() {
        assert_eq!(trim_url("nhentai.net/g/999"), "nhentai.net/g/999");
    }
}
