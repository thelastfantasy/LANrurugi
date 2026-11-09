//! `AppState` as the sharing engine sees it (023).

use futures_util::future::BoxFuture;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_share::host::{ShareArchive, ShareHost};
use lanrurugi_share::settings::ShareSettings;
use lanrurugi_storage::redis::RedisDbs;

use crate::AppState;

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
                file: std::path::PathBuf::from(archive.file),
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
}
