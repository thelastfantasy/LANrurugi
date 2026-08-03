//! Redis read/write mappers for Archive/Category/Grouping/Stamp, matching the legacy field names
//! and key shapes verified against `~/LANraragi` source (see module docs on `entities.rs` and
//! `redis.rs` for what was checked and why). All operations run against the **archive** logical DB
//! (`RedisDbs::archive`) — Category/Grouping/Stamp share that DB with Archive in the legacy layout,
//! they are not split across the other four logical DBs.
//!
//! Scope note: this module covers the entity CRUD itself. Secondary search-index side effects
//! (`LRR_TITLES`, `INDEX_*`, `LRR_UNTAGGED`, `LRR_TANKGROUPED`, ...) live in `lanrurugi-search` and
//! `lanrurugi-scanner`, which call into this module rather than duplicating its Redis access.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::entities::{Archive, Category, Grouping, Stamp, TocEntry};
use lanrurugi_core::ids::{ArchiveId, CategoryId, StampId, TankId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis field {field:?} for key {key:?}: {source}")]
    Json {
        key: String,
        field: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0} {1:?} not found")]
    NotFound(&'static str, String),
    #[error("parallel hashing task failed: {0}")]
    Concurrency(#[from] lanrurugi_core::concurrency::BlockingTaskError),
}

type Result<T> = std::result::Result<T, RepositoryError>;

/// Shared shape behind `ArchiveRepository`/`CategoryRepository`/`GroupingRepository`'s own
/// `list_all`: list every key matching `glob`, then fetch+collect each one that still exists by
/// the time its own `get` runs (a key can disappear between the `KEYS` scan and the per-id fetch
/// under concurrent writes — same race tolerated by each repository's own hand-written loop this
/// replaces, not a new behavior).
async fn list_all_by_glob<T, F, Fut>(pool: &Pool, glob: &str, get: F) -> Result<Vec<T>>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    let mut conn = pool.get().await?;
    let ids: Vec<String> = conn.keys(glob).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(item) = get(id).await? {
            out.push(item);
        }
    }
    Ok(out)
}

/// 40 lowercase-hex-char pattern, matching legacy `$redis->keys('????????????????????????????????????????')`.
const ARCHIVE_KEY_GLOB: &str = "????????????????????????????????????????";

#[derive(Clone)]
pub struct ArchiveRepository {
    pool: Pool,
}

impl ArchiveRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, id: &ArchiveId) -> Result<Option<Archive>> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.exists(id.as_str()).await?;
        if !exists {
            return Ok(None);
        }
        let fields: std::collections::HashMap<String, String> = conn.hgetall(id.as_str()).await?;
        Ok(Some(archive_from_fields(id, fields)?))
    }

    /// All archive IDs currently in the database (legacy 40-hex-char key glob).
    pub async fn list_ids(&self) -> Result<Vec<ArchiveId>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.keys(ARCHIVE_KEY_GLOB).await?;
        Ok(ids.into_iter().map(ArchiveId).collect())
    }

    pub async fn list_all(&self) -> Result<Vec<Archive>> {
        list_all_by_glob(&self.pool, ARCHIVE_KEY_GLOB, |id| async move {
            self.get(&ArchiveId(id)).await
        })
        .await
    }

    /// Finds the archive whose stored `file` path has `filename` as its basename, if any. No
    /// indexed lookup exists for this (legacy has none either — `file` is not a secondary Redis
    /// index anywhere) so this is a full `list_all` scan; acceptable given this project's existing
    /// library-size assumptions (`list_all` is already called unconditionally by, e.g., full-library
    /// search-index rebuilds).
    pub async fn find_by_filename(&self, filename: &str) -> Result<Option<Archive>> {
        let archives = self.list_all().await?;
        Ok(archives.into_iter().find(|a| {
            std::path::Path::new(&a.file)
                .file_name()
                .and_then(|n| n.to_str())
                == Some(filename)
        }))
    }

    /// Creates or fully overwrites an archive record's hash fields.
    pub async fn save(&self, archive: &Archive) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let toc_json = serde_json::to_string(&toc_to_legacy_map(&archive.toc)).map_err(|e| {
            RepositoryError::Json {
                key: archive.id.to_string(),
                field: "toc",
                source: e,
            }
        })?;
        let stamps_json =
            serde_json::to_string(&archive.stamp_ids).map_err(|e| RepositoryError::Json {
                key: archive.id.to_string(),
                field: "stamps",
                source: e,
            })?;
        let corrupted_pages_json =
            serde_json::to_string(&archive.corrupted_pages).map_err(|e| RepositoryError::Json {
                key: archive.id.to_string(),
                field: "corrupted_pages",
                source: e,
            })?;

        let fields: Vec<(&str, String)> = vec![
            ("name", archive.name.clone()),
            ("title", archive.title.clone()),
            ("file", archive.file.clone()),
            ("tags", archive.tags.clone()),
            ("summary", archive.summary.clone()),
            ("arcsize", archive.arcsize.to_string()),
            ("pagecount", archive.pagecount.to_string()),
            (
                "isnew",
                if archive.isnew { "true" } else { "false" }.to_string(),
            ),
            ("progress", archive.lastreadpage.to_string()),
            ("lastreadtime", archive.lastreadtime.to_string()),
            ("toc", toc_json),
            ("stamps", stamps_json),
            ("corrupted_pages", corrupted_pages_json),
        ];
        let _: () = conn.hset_multiple(archive.id.as_str(), &fields).await?;
        if let Some(thumbhash) = &archive.thumbhash {
            let _: () = conn
                .hset(archive.id.as_str(), "thumbhash", thumbhash)
                .await?;
        }
        // `HSET`, unlike `hset_multiple` above, never clears a field on its own — `heal_failed_at`
        // needs an explicit `HDEL` when `None` so a fresh catalogue of this archive ID (e.g.
        // re-downloading and overwriting a permanently-broken one) doesn't inherit a stale failure
        // marker left over from whatever record previously occupied this ID.
        match archive.heal_failed_at {
            Some(ts) => {
                let _: () = conn
                    .hset(archive.id.as_str(), "heal_failed_at", ts.to_string())
                    .await?;
            }
            None => {
                let _: () = conn.hdel(archive.id.as_str(), "heal_failed_at").await?;
            }
        }
        Ok(())
    }

    pub async fn delete(&self, id: &ArchiveId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(id.as_str()).await?;
        Ok(())
    }

    /// Reading-progress accessors: legacy stores this as plain fields on the Archive hash, not a
    /// separate entity (verified: `Utils::Database::build_json` reads `progress`/`lastreadtime`
    /// straight off the archive's own hash).
    pub async fn set_progress(&self, id: &ArchiveId, page: u32, read_at_unix: u64) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let fields: Vec<(&str, String)> = vec![
            ("progress", page.to_string()),
            ("lastreadtime", read_at_unix.to_string()),
        ];
        let _: () = conn.hset_multiple(id.as_str(), &fields).await?;
        Ok(())
    }

    pub async fn rename_id(&self, old_id: &ArchiveId, new_id: &ArchiveId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = deadpool_redis::redis::cmd("RENAME")
            .arg(old_id.as_str())
            .arg(new_id.as_str())
            .query_async(&mut conn)
            .await?;
        Ok(())
    }
}

fn toc_to_legacy_map(toc: &[TocEntry]) -> std::collections::BTreeMap<String, String> {
    toc.iter()
        .map(|e| (e.page.to_string(), e.name.clone()))
        .collect()
}

fn toc_from_legacy_json(raw: &str) -> Vec<TocEntry> {
    let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, String>>(raw) else {
        return Vec::new();
    };
    let mut entries: Vec<TocEntry> = map
        .into_iter()
        .filter_map(|(page, name)| page.parse().ok().map(|page| TocEntry { page, name }))
        .collect();
    entries.sort_by_key(|e| e.page);
    entries
}

fn archive_from_fields(
    id: &ArchiveId,
    mut fields: std::collections::HashMap<String, String>,
) -> Result<Archive> {
    let name = fields.remove("name").unwrap_or_default();
    let title = {
        let t = fields.remove("title").unwrap_or_default();
        if t.trim().is_empty() {
            name.clone()
        } else {
            t
        }
    };
    let stamp_ids: Vec<StampId> = fields
        .get("stamps")
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let corrupted_pages: Vec<String> = fields
        .remove("corrupted_pages")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let arcsize = fields.remove("arcsize").unwrap_or_default();
    let pagecount = fields.remove("pagecount").unwrap_or_default();
    let isnew = fields.remove("isnew").unwrap_or_default();
    let progress = fields.remove("progress").unwrap_or_default();
    let lastreadtime = fields.remove("lastreadtime").unwrap_or_default();
    let heal_failed_at = fields.remove("heal_failed_at").and_then(|s| s.parse().ok());

    Ok(Archive {
        id: id.clone(),
        name,
        title,
        file: fields.remove("file").unwrap_or_default(),
        tags: fields.remove("tags").unwrap_or_default(),
        summary: fields.remove("summary").unwrap_or_default(),
        arcsize: arcsize.parse().unwrap_or(0),
        pagecount: pagecount.parse().unwrap_or(0),
        isnew: isnew == "true",
        lastreadpage: progress.parse().unwrap_or(0),
        lastreadtime: lastreadtime.parse().unwrap_or(0),
        thumbhash: fields.remove("thumbhash"),
        toc: fields
            .get("toc")
            .map(|raw| toc_from_legacy_json(raw))
            .unwrap_or_default(),
        stamp_ids,
        heal_failed_at,
        corrupted_pages,
    })
}

/// Categories are `SET_<10-digit-unix-timestamp>` hashes (verified: `Model/Category.pm`).
#[derive(Clone)]
pub struct CategoryRepository {
    pool: Pool,
}

impl CategoryRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, catid: &CategoryId) -> Result<Option<Category>> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.exists(catid.as_str()).await?;
        if !exists {
            return Ok(None);
        }
        let fields: std::collections::HashMap<String, String> =
            conn.hgetall(catid.as_str()).await?;
        let search = fields.get("search").cloned().filter(|s| !s.is_empty());
        let archives: Vec<ArchiveId> = if search.is_none() {
            fields
                .get("archives")
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Some(Category {
            catid: catid.clone(),
            name: fields.get("name").cloned().unwrap_or_default(),
            search,
            archives,
            pinned: fields.get("pinned").map(|p| p == "1").unwrap_or(false),
        }))
    }

    pub async fn list_all(&self) -> Result<Vec<Category>> {
        list_all_by_glob(&self.pool, "SET_??????????", |id| async move {
            self.get(&CategoryId(id)).await
        })
        .await
    }

    /// Creates a new static (non-dynamic) category or overwrites metadata on an existing one.
    pub async fn save(&self, category: &Category) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let archives_json =
            serde_json::to_string(&category.archives).map_err(|e| RepositoryError::Json {
                key: category.catid.to_string(),
                field: "archives",
                source: e,
            })?;
        let fields: Vec<(&str, String)> = vec![
            ("name", category.name.clone()),
            ("search", category.search.clone().unwrap_or_default()),
            ("archives", archives_json),
            (
                "pinned",
                if category.pinned { "1" } else { "0" }.to_string(),
            ),
        ];
        let _: () = conn.hset_multiple(category.catid.as_str(), &fields).await?;
        Ok(())
    }

    pub async fn delete(&self, catid: &CategoryId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(catid.as_str()).await?;
        Ok(())
    }
}

/// Tankoubons are `TANK_<10-digit-timestamp>` **ZSETs** with packed metadata (verified:
/// `Model/Tankoubon.pm`). Metadata members sit at scores 0/-1/-2/-3; archive IDs occupy positive
/// scores 1..N in volume order.
#[derive(Clone)]
pub struct GroupingRepository {
    pool: Pool,
}

const SCORE_NAME: isize = 0;
const SCORE_SUMMARY: isize = -1;
const SCORE_TAGS: isize = -2;
const SCORE_PROGRESS: isize = -3;

impl GroupingRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, tankid: &TankId) -> Result<Option<Grouping>> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.exists(tankid.as_str()).await?;
        if !exists {
            return Ok(None);
        }

        let metadata_members: Vec<String> = conn
            .zrangebyscore(tankid.as_str(), SCORE_PROGRESS, SCORE_NAME)
            .await?;
        let mut name = String::new();
        let mut summary = String::new();
        let mut tags = String::new();
        let mut progress = 0u32;
        for member in metadata_members {
            if let Some(v) = member.strip_prefix("name_") {
                name = v.to_string();
            } else if let Some(v) = member.strip_prefix("summary_") {
                summary = v.to_string();
            } else if let Some(v) = member.strip_prefix("tags_") {
                tags = v.to_string();
            } else if let Some(v) = member.strip_prefix("progress_") {
                progress = v.parse().unwrap_or(0);
            }
        }

        let archive_strs: Vec<String> = conn.zrangebyscore(tankid.as_str(), 1, "+inf").await?;
        let archives: Vec<ArchiveId> = archive_strs.into_iter().map(ArchiveId).collect();

        Ok(Some(Grouping {
            tankid: tankid.clone(),
            name,
            summary,
            tags,
            progress,
            archives,
        }))
    }

    pub async fn list_all(&self) -> Result<Vec<Grouping>> {
        list_all_by_glob(&self.pool, "TANK_??????????", |id| async move {
            self.get(&TankId(id)).await
        })
        .await
    }

    /// Writes metadata members and the ordered archive-membership members in one call. Intended
    /// for create/full-replace; incremental add/remove-one-archive belongs in a higher-level
    /// service that also maintains `LRR_TANKGROUPED`/`LRR_TITLES` (lanrurugi-search).
    pub async fn save(&self, grouping: &Grouping) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(grouping.tankid.as_str()).await?;

        let mut members: Vec<(isize, String)> = vec![
            (SCORE_NAME, format!("name_{}", grouping.name)),
            (SCORE_SUMMARY, format!("summary_{}", grouping.summary)),
            (SCORE_TAGS, format!("tags_{}", grouping.tags)),
            (SCORE_PROGRESS, format!("progress_{}", grouping.progress)),
        ];
        for (i, archive_id) in grouping.archives.iter().enumerate() {
            members.push(((i + 1) as isize, archive_id.to_string()));
        }
        // deadpool-redis's ZADD takes (score, member) pairs.
        let zadd_args: Vec<(isize, String)> = members;
        let _: () = conn
            .zadd_multiple(grouping.tankid.as_str(), &zadd_args)
            .await?;
        Ok(())
    }

    pub async fn delete(&self, tankid: &TankId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(tankid.as_str()).await?;
        Ok(())
    }
}

/// Stamps are `STAMPS_<page>_<millisecond-timestamp>` hashes (verified: `Model/Stamp.pm`).
#[derive(Clone)]
pub struct StampRepository {
    pool: Pool,
}

impl StampRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, stamp_id: &StampId) -> Result<Option<Stamp>> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.exists(stamp_id.as_str()).await?;
        if !exists {
            return Ok(None);
        }
        let fields: std::collections::HashMap<String, String> =
            conn.hgetall(stamp_id.as_str()).await?;
        Ok(Some(Stamp {
            stamp_id: stamp_id.clone(),
            content: fields.get("content").cloned().unwrap_or_default(),
            position: fields.get("position").cloned().unwrap_or_default(),
            archive_id: ArchiveId(fields.get("archive_id").cloned().unwrap_or_default()),
            icon: fields.get("icon").cloned().unwrap_or_default(),
            rect: fields.get("rect").cloned().unwrap_or_default(),
        }))
    }

    /// Writes a stamp hash verbatim under its own (already-known) key, without touching the
    /// owning archive's `stamps` list — used by backup restore, which reconciles that list
    /// separately in one pass (`lanrurugi-backup::restore::relink_stamp_ids`) rather than via
    /// `create`'s incremental read-modify-write per stamp.
    pub async fn restore_raw(&self, stamp: &Stamp) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let fields: Vec<(&str, &str)> = vec![
            ("content", &stamp.content),
            ("position", &stamp.position),
            ("archive_id", stamp.archive_id.as_str()),
            ("icon", &stamp.icon),
            ("rect", &stamp.rect),
        ];
        let _: () = conn.hset_multiple(stamp.stamp_id.as_str(), &fields).await?;
        Ok(())
    }

    /// Creates a new stamp for `archive_id`'s page `page` and appends it to that archive's
    /// `stamps` JSON list (legacy `add_stamp`), returning the new stamp's key.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        archive_id: &ArchiveId,
        page: u32,
        content: &str,
        position: &str,
        icon: &str,
        rect: &str,
        now_millis: u64,
    ) -> Result<StampId> {
        let mut conn = self.pool.get().await?;
        let stamp_id = StampId(format!("STAMPS_{page}_{now_millis}"));

        let fields: Vec<(&str, &str)> = vec![
            ("content", content),
            ("position", position),
            ("archive_id", archive_id.as_str()),
            ("icon", icon),
            ("rect", rect),
        ];
        let _: () = conn.hset_multiple(stamp_id.as_str(), &fields).await?;

        let existing: Option<String> = conn.hget(archive_id.as_str(), "stamps").await?;
        let mut stamps: Vec<StampId> = existing
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        stamps.push(stamp_id.clone());
        let stamps_json = serde_json::to_string(&stamps).map_err(|e| RepositoryError::Json {
            key: archive_id.to_string(),
            field: "stamps",
            source: e,
        })?;
        let _: () = conn
            .hset(archive_id.as_str(), "stamps", stamps_json)
            .await?;

        Ok(stamp_id)
    }

    pub async fn update(
        &self,
        stamp_id: &StampId,
        content: Option<&str>,
        position: Option<&str>,
        icon: Option<&str>,
        rect: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        if let Some(content) = content {
            let _: () = conn.hset(stamp_id.as_str(), "content", content).await?;
        }
        if let Some(position) = position {
            let _: () = conn.hset(stamp_id.as_str(), "position", position).await?;
        }
        if let Some(icon) = icon {
            let _: () = conn.hset(stamp_id.as_str(), "icon", icon).await?;
        }
        if let Some(rect) = rect {
            let _: () = conn.hset(stamp_id.as_str(), "rect", rect).await?;
        }
        Ok(())
    }

    pub async fn delete(&self, stamp_id: &StampId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let archive_id: Option<String> = conn.hget(stamp_id.as_str(), "archive_id").await?;
        if let Some(archive_id) = archive_id {
            let existing: Option<String> = conn.hget(&archive_id, "stamps").await?;
            let mut stamps: Vec<StampId> = existing
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            stamps.retain(|s| s != stamp_id);
            if let Ok(stamps_json) = serde_json::to_string(&stamps) {
                let _: () = conn.hset(&archive_id, "stamps", stamps_json).await?;
            }
        }
        let _: () = conn.del(stamp_id.as_str()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::TocEntry;

    /// These tests exercise real Redis I/O and are skipped (with a message, not a failure) unless
    /// `LANRURUGI_TEST_REDIS_URL` is set — CI wires that up via a Redis service container (T007);
    /// locally, point it at a throwaway container.
    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    #[tokio::test]
    async fn archive_roundtrip_preserves_all_fields() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ArchiveRepository::new(pool);
        let id = ArchiveId("a".repeat(40));
        let archive = Archive {
            id: id.clone(),
            name: "some manga v1".to_string(),
            title: "Some Manga Vol. 1".to_string(),
            file: "/library/some manga v1.zip".to_string(),
            tags: "artist:jane,language:english".to_string(),
            summary: "a summary".to_string(),
            arcsize: 123456,
            pagecount: 20,
            isnew: true,
            lastreadpage: 5,
            lastreadtime: 1_700_000_000,
            thumbhash: Some("deadbeef".to_string()),
            toc: vec![TocEntry {
                page: 1,
                name: "Chapter 1".to_string(),
            }],
            stamp_ids: vec![],
            heal_failed_at: Some(1_700_000_500),
            corrupted_pages: vec!["page03.jpg".to_string(), "page07.jpg".to_string()],
        };

        repo.save(&archive).await.unwrap();
        let fetched = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched, archive);

        repo.delete(&id).await.unwrap();
        assert!(repo.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn grouping_roundtrip_preserves_order_and_metadata() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = GroupingRepository::new(pool);
        let tankid = TankId("TANK_1700000000".to_string());
        let grouping = Grouping {
            tankid: tankid.clone(),
            name: "My Series".to_string(),
            summary: "series summary".to_string(),
            tags: "series:my series".to_string(),
            progress: 42,
            archives: vec![
                ArchiveId("b".repeat(40)),
                ArchiveId("c".repeat(40)),
                ArchiveId("d".repeat(40)),
            ],
        };

        repo.save(&grouping).await.unwrap();
        let fetched = repo.get(&tankid).await.unwrap().unwrap();
        assert_eq!(fetched, grouping);

        repo.delete(&tankid).await.unwrap();
        assert!(repo.get(&tankid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn category_roundtrip_static_and_dynamic() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = CategoryRepository::new(pool);
        let catid = CategoryId("SET_1700000001".to_string());
        let category = Category {
            catid: catid.clone(),
            name: "Favorites".to_string(),
            search: None,
            archives: vec![ArchiveId("e".repeat(40))],
            pinned: true,
        };
        repo.save(&category).await.unwrap();
        assert_eq!(repo.get(&catid).await.unwrap().unwrap(), category);

        let dyn_catid = CategoryId("SET_1700000002".to_string());
        let dynamic = Category {
            catid: dyn_catid.clone(),
            name: "Recently added".to_string(),
            search: Some("date_added:*".to_string()),
            archives: vec![],
            pinned: false,
        };
        repo.save(&dynamic).await.unwrap();
        let fetched = repo.get(&dyn_catid).await.unwrap().unwrap();
        assert!(fetched.is_dynamic());
        assert!(fetched.archives.is_empty());

        repo.delete(&catid).await.unwrap();
        repo.delete(&dyn_catid).await.unwrap();
    }

    #[tokio::test]
    async fn stamp_create_links_to_archive_and_delete_unlinks() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_repo = ArchiveRepository::new(pool.clone());
        let stamp_repo = StampRepository::new(pool);

        let archive_id = ArchiveId("f".repeat(40));
        let archive = Archive {
            id: archive_id.clone(),
            name: "n".to_string(),
            title: "t".to_string(),
            file: "/x.zip".to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 10,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
        };
        archive_repo.save(&archive).await.unwrap();

        let stamp_id = stamp_repo
            .create(
                &archive_id,
                3,
                "hello",
                "10,20",
                "🎯",
                "10,20,30,40,tl,#ff0000",
                1_700_000_000_000,
            )
            .await
            .unwrap();
        assert_eq!(stamp_id.as_str(), "STAMPS_3_1700000000000");

        let fetched = stamp_repo.get(&stamp_id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "hello");
        assert_eq!(fetched.archive_id, archive_id);
        assert_eq!(fetched.page(), Some(3));
        assert_eq!(fetched.icon, "🎯");
        assert_eq!(fetched.rect, "10,20,30,40,tl,#ff0000");

        let updated_archive = archive_repo.get(&archive_id).await.unwrap().unwrap();
        assert_eq!(updated_archive.stamp_ids, vec![stamp_id.clone()]);

        stamp_repo.delete(&stamp_id).await.unwrap();
        assert!(stamp_repo.get(&stamp_id).await.unwrap().is_none());
        let final_archive = archive_repo.get(&archive_id).await.unwrap().unwrap();
        assert!(final_archive.stamp_ids.is_empty());

        archive_repo.delete(&archive_id).await.unwrap();
    }
}
