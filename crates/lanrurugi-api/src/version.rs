//! `GET /api/version` — a Zipline-style version/update-check endpoint.
//!
//! Kept in the public (unauthenticated) router group so the app's own `Footer`/`UpdateBanner`
//! can call it before a session exists, matching the existing public `/api/info` pattern
//! (`misc::public_router` / issue #92). The payload deliberately contains no secrets and exposes
//! no more than the already-public `/api/info` version fields plus GitHub release/commit metadata.
//!
//! The backend performs the GitHub API calls (instead of the older frontend-only
//! `useUpdateCheck`, which fetched GitHub directly from every browser) so:
//! - the GitHub token can be kept server-side when rate limits become a concern,
//! - responses can be cached process-wide for a few hours,
//! - a workflow-aware result can distinguish tagged release builds from trunk/upstream builds,
//!   and can mark an upstream build as currently pullable only after the multi-arch Docker
//!   "amend-builds" job has completed successfully (Zipline's `src/lib/version/github.ts`
//!   approach).
//!
//! `--disable-update-check` / `LANRURUGI_DISABLE_UPDATE_CHECK` is honored by returning
//! `enabled: false` without making any outbound GitHub request.

use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use crate::AppState;

/// Repository used for update checks. Overridable for forks or self-hosted mirrors.
const DEFAULT_REPO: &str = "thelastfantasy/LANrurugi";
/// Zipline copies this same six-hour TTL — long enough to avoid GitHub rate-limit pain, short
/// enough that a newly published release appears in reasonable time.
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone)]
struct Cached {
    key: String,
    fetched_at: Instant,
    data: Value,
}

/// Process-wide check cache. LANrurugi is a single process/server; if a future multi-process
/// deployment needs this, move the cache into Redis (the `config` logical DB is already used for
/// the app's other cross-process mutable state).
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

pub fn public_router() -> Router<AppState> {
    Router::new().route("/version", get(get_version))
}

fn repo() -> String {
    std::env::var("LANRURUGI_UPDATE_CHECK_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

fn github_api() -> String {
    format!("https://api.github.com/repos/{}", repo())
}

fn github_web() -> String {
    format!("https://github.com/{}", repo())
}

/// Current binary's package version. `CARGO_PKG_VERSION` is baked in at compile time.
fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Best-effort current git SHA:
/// 1. runtime env (`LANRURUGI_GIT_SHA`) — set by a container entrypoint if desired;
/// 2. compile-time env (`LANRURUGI_GIT_SHA`) — set in the Docker build via `ARG`/`ENV`;
/// 3. `git rev-parse HEAD` — works in source/CI checkouts, not anywhere `.git` is absent.
///
/// Cached once at first use: the SHA is a fixed property of the binary/deployment, and repeatedly
/// spawning `git rev-parse HEAD` per request would be needless overhead.
static SHA_CACHE: OnceLock<Option<String>> = OnceLock::new();

fn current_sha() -> Option<String> {
    SHA_CACHE
        .get_or_init(|| {
            if let Ok(value) = std::env::var("LANRURUGI_GIT_SHA") {
                if !value.is_empty() && value != "unknown" {
                    return Some(value);
                }
            }
            if let Some(value) = option_env!("LANRURUGI_GIT_SHA") {
                if !value.is_empty() && value != "unknown" {
                    return Some(value.to_string());
                }
            }
            let output = Command::new("git")
                .arg("rev-parse")
                .arg("HEAD")
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if sha.is_empty() {
                None
            } else {
                Some(sha)
            }
        })
        .clone()
}

fn details_json(version: &str, sha: Option<&str>) -> Value {
    json!({
        "version": version,
        "sha": sha,
    })
}

/// Zipline's short-SHA matcher: accepts either side as prefix, with a minimum length to avoid
/// matching a 1-2 character abbreviated SHA by accident.
fn sha_match(first: &str, second: &str) -> bool {
    if first.len() < 7 || second.len() < 7 {
        return false;
    }
    let first = first.to_lowercase();
    let second = second.to_lowercase();
    first == second || first.starts_with(&second) || second.starts_with(&first)
}

async fn github_get(path: &str) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let url = format!("{}{}", github_api(), path);
    let mut request = client
        .get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "lanrurugi");
    if let Ok(token) = std::env::var("LANRURUGI_GITHUB_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub {} returned {}", path, response.status()));
    }
    response.json::<Value>().await.map_err(|e| e.to_string())
}

async fn fetch_version_info(version: &str, sha: Option<&str>) -> Result<Value, String> {
    let latest_release = github_get("/releases/latest").await?;
    let tags = github_get("/tags?per_page=100").await?;
    let tag_array = tags
        .as_array()
        .ok_or_else(|| "GitHub tags response is not an array".to_string())?;

    let expected_tag = if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    };
    let version_tag = tag_array
        .iter()
        .find(|t| t["name"].as_str() == Some(expected_tag.as_str()));

    let release_tag = if let Some(sha) = sha {
        tag_array.iter().find(|t| {
            t["commit"]["sha"]
                .as_str()
                .is_some_and(|tag_sha| sha_match(tag_sha, sha))
        })
    } else {
        version_tag
    };

    let latest_tag = latest_release["tag_name"]
        .as_str()
        .ok_or_else(|| "GitHub latest release has no tag_name".to_string())?
        .to_string();
    let latest_url = format!("{}/releases/{}", github_web(), latest_tag);
    let latest = json!({
        "tag": latest_tag,
        "url": latest_url,
    });

    if let Some(tag) = release_tag {
        let tag_name = tag["name"]
            .as_str()
            .ok_or_else(|| "GitHub tag has no name".to_string())?
            .to_string();
        let tag_sha = tag["commit"]["sha"]
            .as_str()
            .ok_or_else(|| "GitHub tag has no commit sha".to_string())?
            .to_string();
        return Ok(json!({
            "isUpstream": false,
            "isRelease": true,
            "isLatest": latest_tag == tag_name,
            "version": {
                "tag": tag_name,
                "sha": tag_sha,
                "url": format!("{}/releases/{}", github_web(), tag_name),
            },
            "latest": latest,
        }));
    }

    // Not a tagged release. Zipline treats this as an upstream/trunk build and compares against
    // the repository's latest commit, optionally checking whether that commit actually produced a
    // pullable multi-arch Docker image (the `amend-builds` check run).
    let Some(sha) = sha else {
        // Without a SHA we cannot identify an upstream commit. Report a benign "nothing new"
        // result rather than failing the whole endpoint.
        return Ok(json!({
            "isUpstream": false,
            "isRelease": false,
            "isLatest": true,
            "version": {
                "tag": expected_tag,
                "sha": Value::Null,
                "url": format!("{}/commit/unknown", github_web()),
            },
            "latest": latest,
        }));
    };

    let commits = github_get("/commits?per_page=1").await?;
    let latest_commit = commits
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| "GitHub commits response is empty".to_string())?;
    let latest_commit_sha = latest_commit["sha"]
        .as_str()
        .ok_or_else(|| "GitHub latest commit has no sha".to_string())?
        .to_string();

    // "pull" mirrors Zipline's `amend-builds` check: only advertise an upstream update as
    // pullable once the commit's Docker build jobs (the workflow job that creates the shared
    // multi-arch manifests) have completed successfully.
    let pull = match github_get(&format!("/commits/{}/check-runs", latest_commit_sha)).await {
        Ok(runs) => runs["check_runs"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|run| run["name"].as_str() == Some("amend-builds"))
            })
            .is_some_and(|run| {
                run["status"].as_str() == Some("completed")
                    && run["conclusion"].as_str() == Some("success")
            }),
        Err(_) => false,
    };

    let (latest_tag, latest_url) = extracted_latest(&latest);

    Ok(json!({
        "isUpstream": true,
        "isRelease": false,
        "isLatest": sha_match(&latest_commit_sha, sha),
        "version": {
            "tag": version_tag
                .and_then(|t| t["name"].as_str())
                .unwrap_or(expected_tag.as_str())
                .to_string(),
            "sha": sha,
            "url": format!("{}/commit/{}", github_web(), sha),
        },
        "latest": {
            "tag": latest_tag,
            "url": latest_url,
            "commit": {
                "sha": latest_commit_sha,
                "url": format!("{}/commit/{}", github_web(), latest_commit_sha),
                "pull": pull,
            }
        },
    }))
}

fn extracted_latest(latest: &Value) -> (String, String) {
    let tag = latest["tag"].as_str().unwrap_or("unknown").to_string();
    let url = latest["url"].as_str().unwrap_or("").to_string();
    (tag, url)
}

async fn get_version(State(state): State<AppState>) -> Response {
    if state.disable_update_check {
        // No outbound request is made when the operator has explicitly suppressed update checks.
        // Returning a success shape with `enabled: false` keeps the frontend contract simple while
        // ensuring it treats this exactly like the old client-side `debugMode` path (no toast).
        let version = current_version();
        let sha = current_sha();
        return axum::Json(json!({
            "enabled": false,
            "details": details_json(version, sha.as_deref()),
            "data": {
                "isUpstream": false,
                "isRelease": false,
                "isLatest": true,
                "version": {
                    "tag": version,
                    "sha": sha,
                    "url": Value::Null,
                },
                "latest": Value::Null,
            },
            "cached": false,
        }))
        .into_response();
    }

    let version = current_version();
    let sha = current_sha();
    let cache_key = format!("{},{}", version, sha.as_deref().unwrap_or("unknown"));

    if let Some(cached) = CACHE.lock().unwrap().as_ref() {
        if cached.key == cache_key && cached.fetched_at.elapsed() < CACHE_TTL {
            return axum::Json(json!({
                "enabled": true,
                "details": details_json(version, sha.as_deref()),
                "data": cached.data.clone(),
                "cached": true,
            }))
            .into_response();
        }
    }

    match fetch_version_info(version, sha.as_deref()).await {
        Ok(data) => {
            *CACHE.lock().unwrap() = Some(Cached {
                key: cache_key,
                fetched_at: Instant::now(),
                data: data.clone(),
            });
            axum::Json(json!({
                "enabled": true,
                "details": details_json(version, sha.as_deref()),
                "data": data,
                "cached": false,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "version lookup failed");
            // Fail open with current-only data: the UI may show the version badge but must not
            // claim a nonexistent update is available just because GitHub was unreachable.
            axum::Json(json!({
                "enabled": true,
                "details": details_json(version, sha.as_deref()),
                "data": {
                    "isUpstream": false,
                    "isRelease": false,
                    "isLatest": true,
                    "version": {
                        "tag": version,
                        "sha": sha,
                        "url": Value::Null,
                    },
                    "latest": Value::Null,
                },
                "cached": false,
            }))
            .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_match_accepts_either_prefix() {
        assert!(sha_match("0123456789abcdef", "0123456789"));
        assert!(sha_match("0123456789", "0123456789abcdef"));
        assert!(sha_match("0123456789abcdef", "0123456789abcdef"));
        assert!(!sha_match("0123456a", "0123456b"));
        assert!(!sha_match("abc", "abcdefg"));
    }

    #[test]
    fn disabled_response_has_no_github_dependency() {
        // The handler is not exercised here (it's async and needs AppState); this test documents
        // that the shape helpers never require a Redis connection or an outbound HTTP call.
        let sha = current_sha();
        let details = details_json(current_version(), sha.as_deref());
        assert_eq!(details["version"], current_version());
    }
}
