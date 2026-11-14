//! `AppState` as the sharing engine sees it (023).

use futures_util::future::BoxFuture;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_share::host::{
    CatalogueRefusal, Host, ImportHost, ShareArchive, ShareHost, SharedMetadata,
};
use lanrurugi_share::settings::ShareSettings;
use lanrurugi_storage::redis::RedisDbs;

use crate::AppState;

impl AppState {
    /// This state as the sharing engine holds it.
    pub fn share_host(&self) -> Host {
        std::sync::Arc::new(self.clone())
    }
}

impl ShareHost for AppState {
    fn redis(&self) -> RedisDbs {
        self.redis.clone()
    }

    fn jobs(&self) -> JobRegistry {
        self.jobs.clone()
    }

    fn temp_dir(&self) -> std::path::PathBuf {
        self.library.temp_dir.clone()
    }

    fn archive(&self, id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>> {
        let repos = self.repos.clone();
        let id = id.clone();
        Box::pin(async move {
            let archive = repos.archives.get(&id).await.ok().flatten()?;
            Some(ShareArchive {
                id: archive.id.clone(),
                title: archive.title.clone(),
                name: archive.name.clone(),
                file: std::path::PathBuf::from(archive.file),
                tags: archive.tags.clone(),
                summary: archive.summary.clone(),
                pagecount: archive.pagecount,
                arcsize: archive.arcsize,
                toc: archive
                    .toc
                    .iter()
                    .map(|c| lanrurugi_share_proto::messages::Chapter {
                        page: c.page,
                        name: c.name.clone(),
                    })
                    .collect(),
            })
        })
    }

    fn thumbnail(&self, id: &ArchiveId, page: u32) -> BoxFuture<'_, Option<(String, Vec<u8>)>> {
        let state = self.clone();
        let id = id.clone();
        Box::pin(async move {
            crate::archives::share_thumbnail(&state, &id, page)
                .await
                .map(|(content_type, bytes)| (content_type.to_string(), bytes))
        })
    }

    fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz> {
        let state = self.clone();
        Box::pin(async move { crate::share_net::limits::timezone(&state).await })
    }

    fn categories(&self, id: &ArchiveId) -> BoxFuture<'_, Vec<String>> {
        let repos = self.repos.clone();
        let id = id.clone();
        Box::pin(async move {
            repos
                .categories
                .for_archive(&id)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|c| !c.is_dynamic())
                .map(|c| c.name)
                .collect()
        })
    }

    fn tankoubon(
        &self,
        id: &lanrurugi_core::ids::TankId,
    ) -> BoxFuture<'_, Option<lanrurugi_share::host::ShareTankoubon>> {
        let repos = self.repos.clone();
        let id = id.clone();
        Box::pin(async move {
            let tank = repos.groupings.get(&id).await.ok().flatten()?;
            Some(lanrurugi_share::host::ShareTankoubon {
                name: tank.name.clone(),
                summary: tank.summary.clone(),
                tags: tank.tags.clone(),
                archives: tank.archives.clone(),
            })
        })
    }

    fn settings(&self) -> BoxFuture<'_, ShareSettings> {
        Box::pin(async move { crate::share_settings::load(self).await })
    }

    fn node_code(&self) -> BoxFuture<'_, Option<lanrurugi_share_proto::node_code::NodeCode>> {
        Box::pin(async move {
            crate::shares::node_code(self)
                .await
                .map_err(|e| tracing::warn!(error = %e, "this instance has no node code right now"))
                .ok()
        })
    }

    fn add_import_bytes(
        &self,
        share: &lanrurugi_core::ids::ShareId,
        bytes: u64,
    ) -> BoxFuture<'_, ()> {
        let repo = crate::shares::repo(self);
        let share = share.clone();
        Box::pin(async move {
            if let Err(error) = repo.add_import_bytes(&share, bytes).await {
                tracing::warn!(share = %share, error = %error, "could not flush served import bytes");
            }
        })
    }

    fn start_host_services(&self) {
        crate::share_net::blobs::spawn_prepare_worker(self.clone());
    }

    fn record_activity(
        &self,
        action: &'static str,
        target: lanrurugi_storage::activity::ActivityTarget,
        after: serde_json::Value,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            crate::activity::record_automatic(
                self,
                "share",
                action,
                target,
                lanrurugi_storage::activity::Outcome::Success,
                Some(after),
                None,
            )
            .await;
        })
    }

    fn imports(&self) -> &dyn ImportHost {
        self
    }
}

impl ImportHost for AppState {
    fn catalogue_import(
        &self,
        staging: std::path::PathBuf,
        name: String,
        blake3_hex: String,
        overwrite: bool,
    ) -> BoxFuture<'_, Result<ArchiveId, CatalogueRefusal>> {
        use crate::download_manager::ingest::IngestDownloadError;
        use lanrurugi_scanner::pipeline::{DuplicatePolicy, DuplicateReason};
        Box::pin(async move {
            let policy = if overwrite {
                DuplicatePolicy::Overwrite
            } else {
                DuplicatePolicy::Reject
            };
            match crate::download_manager::ingest::catalogue_staged_file(
                self,
                &staging,
                &name,
                // The bao root the transfer verified is the file's own BLAKE3 (FR-022).
                Some(blake3_hex.as_str()),
                None,
                policy,
                false,
                false,
                false,
            )
            .await
            {
                Ok(done) => Ok(ArchiveId(done.archive_id)),
                Err(IngestDownloadError::Duplicate {
                    existing_id,
                    reason: DuplicateReason::ContentHash,
                }) => Err(CatalogueRefusal::SameContent(ArchiveId(existing_id))),
                Err(IngestDownloadError::Duplicate {
                    reason: DuplicateReason::Filename,
                    ..
                }) => Err(CatalogueRefusal::NameTaken),
                Err(e) => Err(CatalogueRefusal::Failed(e.to_string())),
            }
        })
    }

    /// Title and summary replace the scanner's guesses; shared tags merge into the archive's own;
    /// the sharer's chapters fill an empty table of contents (one the book itself gave is kept),
    /// as a book's own outline does. Bookmarks and stamps are the sharer's own and are not carried.
    fn apply_shared_metadata(&self, id: ArchiveId, shared: SharedMetadata) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Ok(Some(mut archive)) = self.repos.archives.get(&id).await {
                let old_title = archive.title.clone();
                if !shared.title.is_empty() {
                    archive.title = shared.title.clone();
                }
                if !shared.summary.is_empty() {
                    archive.summary = shared.summary.clone();
                }
                if archive.toc.is_empty() {
                    archive.toc = shared
                        .toc
                        .iter()
                        .filter(|c| c.page >= 1 && c.page <= archive.pagecount.max(1))
                        .map(|c| lanrurugi_core::entities::TocEntry {
                            page: c.page,
                            name: c.name.clone(),
                        })
                        .collect();
                }
                if self.repos.archives.save(&archive).await.is_ok() && old_title != archive.title {
                    let _ = lanrurugi_search::indexer::update_title_index(
                        &self.redis.search,
                        &self.equivalence,
                        id.as_str(),
                        &old_title,
                        &archive.title,
                    )
                    .await;
                }
            }
            crate::plugins::apply_metadata_tags(self, id.as_str(), &shared.tags).await;
        })
    }

    fn set_archive_tags(&self, id: ArchiveId, tags: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Ok(Some(mut archive)) = self.repos.archives.get(&id).await else {
                return;
            };
            let old_tags = std::mem::replace(&mut archive.tags, tags);
            if self.repos.archives.save(&archive).await.is_ok() {
                let _ = lanrurugi_search::indexer::update_tag_indexes(
                    &self.redis.search,
                    &self.equivalence,
                    id.as_str(),
                    &old_tags,
                    &archive.tags,
                )
                .await;
            }
        })
    }

    fn add_to_category(&self, id: ArchiveId, category_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ =
                crate::categories::add_archive_to_category(self, &category_id, id.as_str()).await;
        })
    }

    fn static_category_named(&self, name: String) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            crate::categories::static_category_named(self, &name)
                .await
                .map(|id| id.0)
        })
    }

    fn tankoubon_exists(&self, name: String, members: Vec<ArchiveId>) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            self.repos
                .groupings
                .list_all()
                .await
                .unwrap_or_default()
                .iter()
                .any(|g| g.name == name && g.archives == members)
        })
    }

    fn create_tankoubon(
        &self,
        name: String,
        summary: String,
        tags: String,
        members: Vec<ArchiveId>,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            crate::tankoubons::create_tankoubon_with(self, &name, &summary, &tags, members)
                .await
                .map(|_| ())
        })
    }

    fn set_queue_state(
        &self,
        queue_item_id: String,
        to: lanrurugi_storage::download_queue::DownloadQueueState,
        error: Option<String>,
    ) -> BoxFuture<'_, ()> {
        use lanrurugi_storage::download_queue::DownloadQueueState;
        Box::pin(async move {
            if let Ok(Some(mut item)) = self.download_queue.get(&queue_item_id).await {
                item.state = to;
                if matches!(
                    to,
                    DownloadQueueState::Done
                        | DownloadQueueState::Error
                        | DownloadQueueState::Cancelled
                ) {
                    item.finished_at = Some(lanrurugi_share::util::now_secs() * 1000);
                }
                item.error = error
                    .map(|reason| lanrurugi_core::queue_error::QueueError::ShareImport { reason });
                if self.download_queue.update(&item).await.is_ok() {
                    crate::download_queue::broadcast_item_update(self, &item);
                }
            }
        })
    }

    fn set_queue_job(&self, queue_item_id: String, job_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Ok(Some(mut item)) = self.download_queue.get(&queue_item_id).await {
                item.job_id = Some(job_id);
                if self.download_queue.update(&item).await.is_ok() {
                    crate::download_queue::broadcast_item_update(self, &item);
                }
            }
        })
    }

    fn queue_paused(&self, queue_item_id: String) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            self.download_queue
                .get(&queue_item_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|item| {
                    item.state == lanrurugi_storage::download_queue::DownloadQueueState::Paused
                })
        })
    }

    fn set_import_cancellation(
        &self,
        queue_item_id: String,
        token: Option<tokio_util::sync::CancellationToken>,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut map = self.download_cancellations.lock().await;
            match token {
                Some(token) => {
                    map.insert(queue_item_id, token);
                }
                None => {
                    map.remove(&queue_item_id);
                }
            }
        })
    }

    fn import_progress(
        &self,
        queue_item_id: &str,
        job_id: &str,
        record: &lanrurugi_storage::shares::ShareImport,
        done: u64,
        total: u64,
    ) {
        let Some(tx) = &self.download_queue_tx else {
            return;
        };
        let _ = tx.send(serde_json::json!({
            "kind": "progress",
            "id": queue_item_id,
            "job_id": job_id,
            "downloaded_bytes": done,
            "total_bytes": total,
        }));
        let _ = tx.send(serde_json::json!({
            "kind": "share_import",
            "id": queue_item_id,
            "import": crate::share_imports::detail(record, false),
        }));
    }

    /// `tempmaxsize` in bytes (the setting is in MB).
    fn temp_quota_bytes(&self) -> BoxFuture<'_, Option<u64>> {
        Box::pin(async move {
            let mut conn = self.redis.config.get().await.ok()?;
            let megabytes: Option<String> = deadpool_redis::redis::AsyncCommands::hget(
                &mut conn,
                lanrurugi_storage::keys::CONFIG_KEY,
                "tempmaxsize",
            )
            .await
            .ok()?;
            let megabytes = megabytes
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(crate::download_manager::ingest::DEFAULT_TEMP_MAX_SIZE_MB)
                .max(0) as u64;
            Some(megabytes * 1024 * 1024)
        })
    }
}
