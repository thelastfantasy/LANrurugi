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
}
