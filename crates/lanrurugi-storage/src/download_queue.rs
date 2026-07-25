//! Redis persistence for the Upload page's download queue
//! (`specs/001-lanrurugi-full-rewrite`'s Upload-page-redesign addendum) — a purely additive
//! Redis namespace, no legacy key shape touched, so a queued/in-progress download stays visible
//! across a page refresh or a second browser tab (both poll `GET /download_queue`) even though
//! the actual download itself already runs as shared server-side state
//! (`lanrurugi_core::jobs::JobRegistry`) independent of any one client's connection.
//!
//! Structural template: [`crate::plugin_options`] (same `thiserror` error enum shape, same `Pool`
//! field, same test-gated-on-`LANRURUGI_TEST_REDIS_URL` convention) — the one addition is
//! `LANRURUGI_DOWNLOAD_QUEUE_IDS`, a Redis Set of live item IDs, needed since (unlike
//! `plugin_options`, which enumerates via the plugin directory itself) nothing else provides an
//! external list of queue-item IDs.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DownloadQueueStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, DownloadQueueStorageError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadQueueState {
    Queued,
    Starting,
    Downloading,
    Done,
    Error,
    /// A user-requested Stop interrupted this item's in-flight download (`download_queue::stop_one`).
    /// Deliberately its own state rather than reverting straight to `Queued` — the same reasoning
    /// that motivated showing a "已取消"/"Cancelled" label on the row at all also means that signal
    /// needs to survive a page refresh, not just live in transient frontend mutation state. Treated
    /// as a startable state everywhere `Queued`/`Error` already are (see `start_one`'s guard).
    Cancelled,
}

/// What produced this queue item — a download (the original, only kind this queue ever held) or
/// a local file upload (`PUT /archives/upload`), which now goes through this same persisted
/// state machine so an uploaded file's outcome (and any filename conflict it hits) survives a
/// page refresh exactly like a download's does, instead of living only in frontend memory.
/// `#[serde(default)]` makes every record written before this field existed deserialize as
/// `Download` — the only kind that could have produced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueItemOrigin {
    #[default]
    Download,
    LocalUpload,
}

/// Set on a `DownloadQueueItem` when its most recent ingest attempt was blocked by a `Filename`
/// collision — a different, already-cataloged archive owns the resolved filename, but the
/// downloaded *content* is genuinely new (not the same as `QueueError::DuplicateArchive`'s
/// `ContentHash` case, which is unconditionally rejected and never reaches this state at all: the
/// two `DuplicateReasonKind`s are not symmetric in what's actually safe to allow — see that type's
/// own docs). The downloaded bytes are staged at `temp_path` (not catalogued, not deleted)
/// awaiting the user's explicit choice — `POST /download_queue/{id}/overwrite` or
/// `.../rename` — rather than being silently auto-renamed or immediately discarded. Cleared (and
/// `temp_path` deleted) either when the user resolves it, or by the periodic stale-temp-file sweep
/// in `main.rs` after 24 hours if left unresolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFilenameConflict {
    /// Absolute path to the staged file inside `temp_dir`, named `temp_{crc32}_{filename}` — the
    /// content-derived crc32 prefix (rather than e.g. a numeric `(1)`/`(2)` suffix) makes a second
    /// collision against a stale unresolved temp file's own name astronomically unlikely, and
    /// lets the periodic sweep recognize its own files by a stable `temp_*` glob regardless of
    /// what the original filename was.
    pub temp_path: String,
    /// The filename that was originally intended and that collided — the basename portion of
    /// `temp_path`'s own name, without the `temp_{crc32}_` prefix. Surfaced to the frontend as the
    /// `{filename}` template variable in the rename UI (without its extension — see `{ext}`).
    pub original_filename: String,
    /// The archive ID that already owns `original_filename` — what `.../overwrite` replaces, and
    /// what the frontend links to so the user can inspect it before deciding.
    pub existing_id: String,
    /// Lowercase hex CRC32 of the staged content — the same value embedded in `temp_path`'s own
    /// name, surfaced separately as the `{crc}` template variable so the frontend doesn't need to
    /// parse it back out of the path.
    pub crc32: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadQueueItem {
    pub id: String,
    /// `#[serde(default)]` for the same backward-compatibility reason as `origin` itself — a
    /// record written before this field existed has no `origin` key in its stored JSON at all.
    #[serde(default)]
    pub origin: QueueItemOrigin,
    /// For a download: the source URL. For a local upload: the uploaded file's own filename —
    /// there is no real URL, but every other piece of this state machine (the completed-item
    /// title fallback, the filename-conflict `source:` tag, `resolve_conflict`'s dedup) already
    /// keys off `url`, so a local upload reuses the same field rather than growing a parallel
    /// "url or filename" special case through every one of those call sites.
    pub url: String,
    /// Resolved once, client-side, at add-to-queue time (which download plugin's `url_pattern`
    /// matched this URL) and fixed from then on — not re-resolved at start time, so what the user
    /// saw when adding is what actually runs even if plugin configuration changes in between.
    /// For a local upload: the fixed placeholder `"local_upload"` (never a real plugin
    /// namespace) — used only as this item's grouping key on the Upload page, never looked up
    /// via `PluginPool::plugin_info`.
    pub plugin_namespace: String,
    /// The uploaded/downloaded file's size in bytes, once known — persisted here (not read back
    /// out of a `JobRegistry` job, which is in-process-only and lost on restart) so a completed
    /// item's size survives a page refresh/server restart. Set once, at creation, for a local
    /// upload (the size is already known from the multipart body before ingest even starts); a
    /// download instead reports its size live via `job_id`'s linked `JobStatus.total_bytes`
    /// (this field stays `None` for a download — see the frontend's own size-display fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    pub category: Option<String>,
    pub auto_fetch_metadata: bool,
    pub overwrite_on_duplicate: bool,
    pub state: DownloadQueueState,
    /// Set once `start` has launched the actual background download —
    /// `lanrurugi_core::jobs::JobRegistry` job ID, used by the frontend to look up live
    /// `downloaded_bytes`/`total_bytes` progress via the existing `GET /jobs` polling.
    pub job_id: Option<String>,
    /// Archive IDs a successful managed download produced (set alongside the transition to
    /// `Done`), persisted here — not just in the linked job's own result — because
    /// `JobRegistry` is purely in-process memory and is lost on every server restart, while this
    /// queue item is meant to survive one (it's stored in Redis). The frontend reads this field
    /// first for the completed-item "click title to open archive" link, falling back to the
    /// (possibly already-gone) job result only for an item finished earlier in the same process
    /// lifetime whose queue record predates this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_ids: Option<Vec<String>>,
    /// Set by a "Fetch metadata" preview action (no archive download) — shown in place of the raw
    /// URL once known.
    pub title: Option<String>,
    /// The metadata plugin's full `execMetadata` response (`{tags, title, summary}` — see
    /// `POST /plugins/use`'s `data` field), stashed verbatim alongside `title` by the same
    /// "Fetch metadata" preview action. Kept as
    /// untyped JSON rather than a fixed struct because every metadata plugin's `tags` string uses
    /// its own namespace vocabulary (E-Hentai's `artist:`/`uploader:`/`category:`/`timestamp:`
    /// don't necessarily mean anything to, say, a Pixiv-derived plugin) — the frontend renders
    /// whatever keys/tags actually came back rather than the host assuming a shared schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_preview: Option<serde_json::Value>,
    /// Structured, translatable failure detail (`lanrurugi_core::queue_error::QueueError`) — no
    /// free-text message; the frontend maps `.kind` to an i18n key and renders its own fields
    /// into it. See that type's own docs for why no raw Rust `Display` string is stored here.
    pub error: Option<lanrurugi_core::queue_error::QueueError>,
    /// See [`PendingFilenameConflict`]'s own docs. Independent of `error` (which is set alongside
    /// it, `QueueError::DuplicateFilename`, for the generic error-rendering path) so the frontend
    /// can specifically detect "this needs a resolve action" rather than a plain retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_filename_conflict: Option<PendingFilenameConflict>,
    pub created_at: i64,
}

/// What [`DownloadQueueRepository::add`] needs to construct a new item — every field a caller
/// (download's `add_to_queue`, local upload's `upload_archive`) actually chooses; every other
/// `DownloadQueueItem` field either has one universal starting value (`archive_ids`/`title`/
/// `metadata_preview`/`error`/`pending_filename_conflict` all start `None`) or is assigned by
/// `add` itself (`id`/`created_at`).
pub struct NewQueueItem {
    pub origin: QueueItemOrigin,
    /// See [`DownloadQueueItem::url`]'s own docs — a download's source URL, or a local upload's
    /// filename.
    pub url: String,
    /// See [`DownloadQueueItem::plugin_namespace`]'s own docs — a download's real plugin
    /// namespace, or the local-upload placeholder `"local_upload"`.
    pub plugin_namespace: String,
    /// Already known at creation time for a local upload (the multipart body's byte length);
    /// `None` for a download, which instead reports size live through its linked job.
    pub file_size: Option<u64>,
    pub category: Option<String>,
    pub auto_fetch_metadata: bool,
    pub overwrite_on_duplicate: bool,
    /// A download starts `Queued` (waiting for the user to press Start/for `start_all`). A local
    /// upload's bytes are already fully in hand by the time this is called — its own `add` call
    /// site sets this straight to whatever the synchronous ingest attempt that follows actually
    /// produces (`Done`/`Error`), so it never passes through `Queued` at all.
    pub state: DownloadQueueState,
}

fn item_key(id: &str) -> String {
    format!("LANRURUGI_DOWNLOAD_QUEUE_ITEM_{id}")
}

const IDS_KEY: &str = "LANRURUGI_DOWNLOAD_QUEUE_IDS";

#[derive(Clone)]
pub struct DownloadQueueRepository {
    pool: Pool,
}

impl DownloadQueueRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Assigns a fresh ID and persists the new item, returning it. A struct parameter (not a
    /// growing list of positional `bool`/`Option<String>` args a caller could pass in the wrong
    /// order) since a download's add and a local upload's add now both need to fill in different
    /// subsets of fields (a download's `Queued` start; a local upload's already-known
    /// `file_size` and `LocalUpload` origin) — see [`NewQueueItem`]'s own field docs for what
    /// each caller is expected to supply.
    pub async fn add(&self, new_item: NewQueueItem) -> Result<DownloadQueueItem> {
        let item = DownloadQueueItem {
            id: uuid::Uuid::new_v4().to_string(),
            origin: new_item.origin,
            url: new_item.url,
            plugin_namespace: new_item.plugin_namespace,
            file_size: new_item.file_size,
            category: new_item.category,
            auto_fetch_metadata: new_item.auto_fetch_metadata,
            overwrite_on_duplicate: new_item.overwrite_on_duplicate,
            state: new_item.state,
            job_id: None,
            archive_ids: None,
            title: None,
            metadata_preview: None,
            error: None,
            pending_filename_conflict: None,
            created_at: now_unix(),
        };
        self.save(&item).await?;
        Ok(item)
    }

    /// Creates or fully overwrites one item's stored record and registers its ID in the live-IDs
    /// set (a no-op if already present).
    async fn save(&self, item: &DownloadQueueItem) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = item_key(&item.id);
        let raw = serde_json::to_string(item)
            .map_err(|e| DownloadQueueStorageError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.sadd(IDS_KEY, &item.id).await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<DownloadQueueItem>> {
        let mut conn = self.pool.get().await?;
        let key = item_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| DownloadQueueStorageError::Json(key, e)),
        }
    }

    pub async fn list_all(&self) -> Result<Vec<DownloadQueueItem>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(IDS_KEY).await?;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(item) = self.get(&id).await? {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Full-replace update — the caller is expected to have already merged any partial change
    /// into a freshly-`get`'d item (mirrors `PluginOptionsRepository::save`'s own contract; the
    /// partial-update ergonomics live at the HTTP-handler layer, same as `PUT /plugins/options`).
    pub async fn update(&self, item: &DownloadQueueItem) -> Result<()> {
        self.save(item).await
    }

    /// Idempotent — deleting an item that doesn't exist is a no-op, not an error.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(item_key(id)).await?;
        let _: () = conn.srem(IDS_KEY, id).await?;
        Ok(())
    }

    pub async fn delete_many(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.delete(id).await?;
        }
        Ok(())
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    /// A `NewQueueItem` for a plain download, with only `url`/`plugin_namespace` varying by
    /// caller — every other field left at these tests' shared defaults, so a test only spells out
    /// the one or two fields it actually cares about.
    fn new_download(url: &str, plugin_namespace: &str) -> NewQueueItem {
        NewQueueItem {
            origin: QueueItemOrigin::Download,
            url: url.to_string(),
            plugin_namespace: plugin_namespace.to_string(),
            file_size: None,
            category: None,
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            state: DownloadQueueState::Queued,
        }
    }

    #[tokio::test]
    async fn round_trips_an_item_and_lists_deletes_it() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = DownloadQueueRepository::new(pool);

        let item = repo
            .add(new_download(
                "https://example.com/art/1",
                "download/ehentai",
            ))
            .await
            .unwrap();
        assert_eq!(item.state, DownloadQueueState::Queued);

        let fetched = repo.get(&item.id).await.unwrap().unwrap();
        assert_eq!(fetched, item);

        let all = repo.list_all().await.unwrap();
        assert!(all.iter().any(|i| i.id == item.id));

        let mut updated = fetched;
        updated.state = DownloadQueueState::Downloading;
        updated.job_id = Some("job-123".to_string());
        repo.update(&updated).await.unwrap();
        let refetched = repo.get(&item.id).await.unwrap().unwrap();
        assert_eq!(refetched.state, DownloadQueueState::Downloading);
        assert_eq!(refetched.job_id.as_deref(), Some("job-123"));

        // `archive_ids` must survive a round-trip through Redis — this is the field the
        // completed-item reader link now reads instead of the ephemeral `JobRegistry` result,
        // specifically so it still resolves after a server restart.
        let mut done = refetched;
        done.state = DownloadQueueState::Done;
        done.archive_ids = Some(vec!["abc123".to_string()]);
        repo.update(&done).await.unwrap();
        let refetched_done = repo.get(&item.id).await.unwrap().unwrap();
        assert_eq!(refetched_done.archive_ids, Some(vec!["abc123".to_string()]));

        repo.delete(&item.id).await.unwrap();
        assert_eq!(repo.get(&item.id).await.unwrap(), None);
        let all_after = repo.list_all().await.unwrap();
        assert!(!all_after.iter().any(|i| i.id == item.id));

        // Idempotent: deleting an already-absent item is a no-op.
        repo.delete(&item.id).await.unwrap();
    }

    #[tokio::test]
    async fn local_upload_origin_and_file_size_round_trip() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = DownloadQueueRepository::new(pool);

        // A local upload's `add` call, unlike a download's, already knows its outcome and size
        // up front — no intermediate `Queued`/`Downloading` stop, straight to `Done`.
        let item = repo
            .add(NewQueueItem {
                origin: QueueItemOrigin::LocalUpload,
                url: "manga volume 1.zip".to_string(),
                plugin_namespace: "local_upload".to_string(),
                file_size: Some(123_456),
                category: None,
                auto_fetch_metadata: false,
                overwrite_on_duplicate: false,
                state: DownloadQueueState::Done,
            })
            .await
            .unwrap();
        assert_eq!(item.origin, QueueItemOrigin::LocalUpload);
        assert_eq!(item.file_size, Some(123_456));
        assert_eq!(item.state, DownloadQueueState::Done);

        let fetched = repo.get(&item.id).await.unwrap().unwrap();
        assert_eq!(fetched.origin, QueueItemOrigin::LocalUpload);
        assert_eq!(fetched.file_size, Some(123_456));

        repo.delete(&item.id).await.unwrap();
    }

    /// A record serialized before `origin` existed has no `origin` key in its stored JSON at
    /// all — must deserialize as `Download` (`#[serde(default)]`), not fail or silently become
    /// `LocalUpload`, since every pre-existing queue item genuinely was a download.
    #[test]
    fn missing_origin_field_deserializes_as_download() {
        let json = r#"{
            "id": "abc",
            "url": "https://example.com",
            "plugin_namespace": "download/ehentai",
            "category": null,
            "auto_fetch_metadata": false,
            "overwrite_on_duplicate": false,
            "state": "queued",
            "job_id": null,
            "title": null,
            "error": null,
            "created_at": 0
        }"#;
        let item: DownloadQueueItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.origin, QueueItemOrigin::Download);
        assert_eq!(item.file_size, None);
    }

    #[tokio::test]
    async fn delete_many_removes_every_given_id() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = DownloadQueueRepository::new(pool);

        let a = repo
            .add(new_download("https://a", "download/chaika"))
            .await
            .unwrap();
        let b = repo
            .add(new_download("https://b", "download/chaika"))
            .await
            .unwrap();

        repo.delete_many(&[a.id.clone(), b.id.clone()])
            .await
            .unwrap();
        assert_eq!(repo.get(&a.id).await.unwrap(), None);
        assert_eq!(repo.get(&b.id).await.unwrap(), None);
    }
}
