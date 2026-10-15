//! `GET /api/version` — a Zipline-style version/update-check endpoint.
//!
//! Kept in the public `public_router()` group so the app's own `Footer`/`UpdateBanner` can call
//! it before a session exists, matching the existing public `/api/info` pattern
//! (`misc::public_router` / issue #92). The unified `/api` router still runs `require_api_key`,
//! but `route_policy.csv` explicitly allows `anonymous`/`guest_visitor` to this route. The
//! payload deliberately contains no secrets and exposes no more than the already-public
//! `/api/info` version fields plus GitHub release/commit metadata.
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

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
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

/// The changelog path's own slot, kept separate from [`CACHE`] because its key includes the
/// caller's `since` — sharing one slot would let a `?since=` request evict the plain version
/// check's cached answer and vice versa, on a route both `UpdateBanner` and the release-notes
/// overlay hit on the same page load.
static CHANGELOG_CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Most releases one changelog response may carry. A user returning after a long absence would
/// otherwise get every release ever published in a single payload; 30 is far more than any real
/// gap between two upgrades of a self-hosted instance.
const CHANGELOG_LIMIT: usize = 30;

/// `GET /version`'s query string.
#[derive(Deserialize)]
struct VersionQuery {
    /// The build the caller last saw, as a tag (`v0.2.0`) or a commit sha. Supplied only when a
    /// browser notices its stored build differs from the running one; when present the response
    /// additionally carries `changelog`, the releases published in between.
    since: Option<String>,
}

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

pub(crate) fn current_sha() -> Option<String> {
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

/// Epoch seconds of a GitHub `{"date": "..."}` object (commit author/committer, tag commit).
fn date_secs(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value["date"].as_str()?)
        .ok()
        .map(|at| at.timestamp())
}

/// Epoch seconds a release was published at. `created_at` is the fallback because `published_at`
/// is absent for a draft, and a draft slipping into the window is better than a section that
/// silently has no date.
fn release_secs(release: &Value) -> Option<i64> {
    let raw = release["published_at"]
        .as_str()
        .or(release["created_at"].as_str())?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.timestamp())
}

/// One section of the changelog, as the overlay renders it. The tag's commit sha comes from the
/// already-fetched tags list rather than a per-release `/commits/{tag}` call: the releases list
/// itself never carries the commit a tag points at, and one call per section would spend GitHub's
/// 60-requests-per-hour unauthenticated budget on a single modal.
fn release_entry(release: &Value, tags: &[Value]) -> Value {
    let tag = release["tag_name"].as_str().unwrap_or_default();
    json!({
        "tag": tag,
        "sha": tags
            .iter()
            .find(|t| t["name"].as_str() == Some(tag))
            .and_then(|t| t["commit"]["sha"].as_str()),
        "published_at": release["published_at"].as_str().or(release["created_at"].as_str()),
        "url": release["html_url"].as_str(),
        "body": release["body"].as_str().unwrap_or_default(),
    })
}

/// The instant a build identity belongs to, where the identity is a tag (`v0.2.0`), a bare version
/// (`0.2.0`), or a commit sha: the release published for it, else the commit's own date. `None`
/// when GitHub knows nothing about it.
///
/// Both sides of the upgrade window go through this, which is what makes a trunk build comparable
/// to a released one — neither necessarily has a release row, but every build has a commit date.
async fn identity_instant(identity: &str, releases: &[Value], tags: &[Value]) -> Option<i64> {
    // `details.version` is `CARGO_PKG_VERSION` ("0.2.0") while the tag is "v0.2.0" — the same
    // asymmetry `fetch_version_info`'s own `expected_tag` bridges.
    let alternate = identity
        .strip_prefix('v')
        .map(str::to_string)
        .unwrap_or_else(|| format!("v{identity}"));
    let tag = tags.iter().find_map(|t| {
        let name = t["name"].as_str()?;
        let sha = t["commit"]["sha"].as_str()?;
        (name == identity || name == alternate || sha_match(sha, identity)).then_some(name)
    });

    if let Some(published) = tag
        .and_then(|tag| {
            releases
                .iter()
                .find(|r| r["tag_name"].as_str() == Some(tag))
        })
        .and_then(release_secs)
    {
        return Some(published);
    }
    // Either a tagged build whose release fell outside `CHANGELOG_LIMIT`, or a plain trunk commit.
    github_get(&format!("/commits/{}", tag.unwrap_or(identity)))
        .await
        .ok()
        .and_then(|commit| date_secs(&commit["commit"]["committer"]))
}

/// Releases published between the build the caller last saw (`since`) and the one it is running
/// now, newest first — "everything that changed while I wasn't looking".
///
/// An unrecognized `since` (a hash GitHub has forgotten, a hand-edited `localStorage` value)
/// degrades to legacy's own behaviour — the newest release alone — rather than to nothing at all.
async fn changelog_since(
    since: &str,
    version: &str,
    sha: Option<&str>,
) -> Result<Vec<Value>, String> {
    let releases = github_get(&format!("/releases?per_page={CHANGELOG_LIMIT}")).await?;
    let releases = releases
        .as_array()
        .ok_or("GitHub releases response is not an array")?
        .clone();
    let tags = github_get("/tags?per_page=100")
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let Some(lower) = identity_instant(since, &releases, &tags).await else {
        return Ok(releases
            .iter()
            .take(1)
            .map(|release| release_entry(release, &tags))
            .collect());
    };
    // Inclusive upper bound: the running build's own release belongs in its own upgrade window. A
    // trunk build has no release row, so its commit date is the bound, and if even that is
    // unreachable, "now".
    let upper = match sha {
        Some(sha) => identity_instant(sha, &releases, &tags).await,
        None => None,
    }
    .or(identity_instant(version, &releases, &tags).await)
    .unwrap_or_else(|| chrono::Utc::now().timestamp());

    Ok(releases
        .iter()
        .filter(|release| release_secs(release).is_some_and(|at| at > lower && at <= upper))
        .map(|release| release_entry(release, &tags))
        .collect())
}

/// The `changelog` array for this request, or `None` to omit the key entirely — either because the
/// caller supplied no `since`, or because GitHub could not be asked. Omitted rather than sent
/// empty on purpose: "couldn't ask" and "nothing was published in between" are different answers,
/// and only the second is worth telling the user.
async fn changelog_for(since: Option<&str>, version: &str, sha: Option<&str>) -> Option<Value> {
    let since = since?;
    let key = format!("{version},{}|{since}", sha.unwrap_or("unknown"));
    if let Some(cached) = CHANGELOG_CACHE.lock().unwrap().as_ref() {
        if cached.key == key && cached.fetched_at.elapsed() < CACHE_TTL {
            return Some(cached.data.clone());
        }
    }
    match changelog_since(since, version, sha).await {
        Ok(entries) => {
            let value = Value::Array(entries);
            *CHANGELOG_CACHE.lock().unwrap() = Some(Cached {
                key,
                fetched_at: Instant::now(),
                data: value.clone(),
            });
            Some(value)
        }
        Err(e) => {
            tracing::warn!(error = %e, "changelog lookup failed");
            None
        }
    }
}

async fn get_version(State(state): State<AppState>, Query(query): Query<VersionQuery>) -> Response {
    let since = query
        .since
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
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

    // Cloned out of the guard rather than read inside the `if let`: a `std::sync::MutexGuard` held
    // across the `changelog_for(..).await` below makes this handler's future `!Send`, which axum
    // reports only as the far less obvious "`Handler` is not satisfied".
    let cached_data = {
        let guard = CACHE.lock().unwrap();
        match guard.as_ref() {
            Some(cached) if cached.key == cache_key && cached.fetched_at.elapsed() < CACHE_TTL => {
                Some(cached.data.clone())
            }
            _ => None,
        }
    };
    if let Some(cached_data) = cached_data {
        let mut body = json!({
            "enabled": true,
            "details": details_json(version, sha.as_deref()),
            "data": cached_data,
            "cached": true,
        });
        if let Some(changelog) = changelog_for(since, version, sha.as_deref()).await {
            body["changelog"] = changelog;
        }
        return axum::Json(body).into_response();
    }

    match fetch_version_info(version, sha.as_deref()).await {
        Ok(data) => {
            *CACHE.lock().unwrap() = Some(Cached {
                key: cache_key,
                fetched_at: Instant::now(),
                data: data.clone(),
            });
            let mut body = json!({
                "enabled": true,
                "details": details_json(version, sha.as_deref()),
                "data": data,
                "cached": false,
            });
            if let Some(changelog) = changelog_for(since, version, sha.as_deref()).await {
                body["changelog"] = changelog;
            }
            axum::Json(body).into_response()
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

    fn tags_fixture() -> Vec<Value> {
        vec![
            json!({"name": "v0.2.0", "commit": {"sha": "b0000000000000000000000000000000000000b"}}),
            json!({"name": "v0.1.0", "commit": {"sha": "a0000000000000000000000000000000000000a"}}),
        ]
    }

    #[test]
    fn release_entry_takes_the_sha_from_the_tags_list() {
        let release = json!({
            "tag_name": "v0.2.0",
            "published_at": "2026-10-14T00:00:00Z",
            "html_url": "https://example.test/releases/v0.2.0",
            "body": "notes",
        });
        let entry = release_entry(&release, &tags_fixture());
        assert_eq!(entry["tag"], "v0.2.0");
        assert_eq!(entry["sha"], "b0000000000000000000000000000000000000b");
        assert_eq!(entry["published_at"], "2026-10-14T00:00:00Z");
        assert_eq!(entry["body"], "notes");
    }

    #[test]
    fn release_entry_survives_a_tag_the_tags_list_does_not_know() {
        // A release row can outlive its tag (or arrive before `/tags` catches up). The section must
        // still render, just without a hash rather than not at all.
        let release = json!({"tag_name": "v9.9.9", "published_at": "2026-10-14T00:00:00Z"});
        let entry = release_entry(&release, &tags_fixture());
        assert_eq!(entry["sha"], Value::Null);
        assert_eq!(entry["body"], "");
    }

    #[test]
    fn release_secs_falls_back_to_created_at_for_a_draft() {
        // `published_at` is null on a draft; the window filter must still be able to place it.
        let draft = json!({"created_at": "2026-10-14T00:00:00Z", "published_at": Value::Null});
        assert_eq!(
            release_secs(&draft),
            date_secs(&json!({"date": "2026-10-14T00:00:00Z"}))
        );
        assert_eq!(release_secs(&json!({"body": "no dates at all"})), None);
    }

    #[test]
    fn date_secs_orders_two_releases() {
        let older = date_secs(&json!({"date": "2026-07-03T00:15:53Z"})).unwrap();
        let newer = date_secs(&json!({"date": "2026-07-06T22:08:30Z"})).unwrap();
        assert!(newer > older);
        assert_eq!(date_secs(&json!({"date": "not a date"})), None);
    }
}
