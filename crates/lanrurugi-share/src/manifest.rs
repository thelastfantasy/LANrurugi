//! The share's items as they are right now in the library.

use lanrurugi_share_proto::messages::{ArchiveEntry, ManifestItem, ShareManifest};
use lanrurugi_storage::shares::{Share, ShareItem};

use crate::host::{ShareArchive, ShareHost};

/// The wire form of one archive, without fingerprints (the browse view never carries them).
pub fn archive_entry(a: &ShareArchive) -> ArchiveEntry {
    ArchiveEntry {
        archive_id: a.id.to_string(),
        title: if a.title.trim().is_empty() {
            a.name.clone()
        } else {
            a.title.clone()
        },
        tags: a.tags.clone(),
        summary: a.summary.clone(),
        pagecount: a.pagecount,
        filename: a
            .file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        size: a.arcsize,
        blake3: None,
        categories: Vec::new(),
        toc: a.toc.clone(),
    }
}

/// The share's items as they are now in the library (browse form: no fingerprints). Tankoubons
/// list their current members in order; deleted items become `Unavailable` (US2-4).
pub async fn browse_manifest(host: &dyn ShareHost, share: &Share) -> ShareManifest {
    let mut items = Vec::with_capacity(share.items.len());
    for item in &share.items {
        match item {
            ShareItem::Archive { id } => match host.archive(id).await {
                Some(a) => items.push(ManifestItem::Archive(archive_entry(&a))),
                None => items.push(ManifestItem::Unavailable { title: None }),
            },
            ShareItem::Tankoubon { id } => match host.tankoubon(id).await {
                Some(tank) => {
                    let mut members = Vec::with_capacity(tank.archives.len());
                    for archive_id in &tank.archives {
                        if let Some(a) = host.archive(archive_id).await {
                            members.push(archive_entry(&a));
                        }
                    }
                    items.push(ManifestItem::Tankoubon {
                        id: id.to_string(),
                        name: tank.name,
                        tags: tank.tags,
                        summary: tank.summary,
                        members,
                    });
                }
                None => items.push(ManifestItem::Unavailable { title: None }),
            },
        }
    }
    ShareManifest {
        share_id: share.id.to_string(),
        title: share.title.clone().unwrap_or_default(),
        permissions: share.permissions,
        items,
    }
}

/// The claim form of the manifest: fingerprints where they are ready, and each archive's category
/// names for "keep the sharer's categories". Unready archives are returned as pending so the
/// caller can queue them for fingerprinting without this function spawning work.
pub async fn claim_manifest(
    host: &dyn ShareHost,
    shared: &crate::net::Shared,
    share: &Share,
) -> (
    ShareManifest,
    Vec<[u8; 32]>,
    Vec<lanrurugi_core::ids::ArchiveId>,
) {
    let mut manifest = browse_manifest(host, share).await;
    let mut roots = Vec::new();
    let mut pending = Vec::new();
    for item in &mut manifest.items {
        let entries: Vec<&mut lanrurugi_share_proto::messages::ArchiveEntry> = match item {
            ManifestItem::Archive(a) => vec![a],
            ManifestItem::Tankoubon { members, .. } => members.iter_mut().collect(),
            ManifestItem::Unavailable { .. } => vec![],
        };
        for entry in entries {
            let id = lanrurugi_core::ids::ArchiveId(entry.archive_id.clone());
            entry.categories = host.categories(&id).await;
            match crate::blobs::ready(host, &id).await {
                Some(info) => {
                    entry.blake3 = Some(info.blake3);
                    entry.size = info.size;
                    roots.push(info.blake3);
                    shared
                        .blobs
                        .lock()
                        .expect("blobs lock")
                        .insert(info.blake3, id);
                }
                None => pending.push(id),
            }
        }
    }
    roots.sort();
    roots.dedup();
    (manifest, roots, pending)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures_util::future::BoxFuture;
    use lanrurugi_core::ids::{ArchiveId, ShareId, TankId};
    use lanrurugi_core::jobs::JobRegistry;
    use lanrurugi_share_proto::messages::Permissions;
    use lanrurugi_share_proto::node_code::NodeCode;
    use lanrurugi_storage::redis::RedisDbs;
    use lanrurugi_storage::shares::{RatioAction, Share};
    use serde_json::json;

    use super::*;
    use crate::host::{EmptyHost, ShareTankoubon};

    /// A host that only knows the archives and tankoubon a test hands it. Everything else comes
    /// from `EmptyHost`, so this stays a handful of lines.
    struct StubHost {
        archives: Vec<ShareArchive>,
        tankoubon: Option<ShareTankoubon>,
        inner: EmptyHost,
    }

    impl ShareHost for StubHost {
        fn redis(&self) -> RedisDbs {
            self.inner.redis()
        }
        fn jobs(&self) -> JobRegistry {
            self.inner.jobs()
        }
        fn temp_dir(&self) -> PathBuf {
            self.inner.temp_dir()
        }
        fn archive(&self, id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>> {
            let found = self.archives.iter().find(|a| &a.id == id).cloned();
            Box::pin(async move { found })
        }
        fn settings(&self) -> BoxFuture<'_, crate::settings::ShareSettings> {
            self.inner.settings()
        }
        fn thumbnail(
            &self,
            _id: &ArchiveId,
            _page: u32,
        ) -> BoxFuture<'_, Option<(String, Vec<u8>)>> {
            Box::pin(async { None })
        }
        fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz> {
            Box::pin(async { chrono_tz::UTC })
        }
        fn categories(&self, _id: &ArchiveId) -> BoxFuture<'_, Vec<String>> {
            Box::pin(async { Vec::new() })
        }
        fn tankoubon(&self, _id: &TankId) -> BoxFuture<'_, Option<ShareTankoubon>> {
            let found = self.tankoubon.clone();
            Box::pin(async move { found })
        }
        fn node_code(&self) -> BoxFuture<'_, Option<NodeCode>> {
            Box::pin(async { None })
        }
        fn add_import_bytes(&self, _share: &ShareId, _bytes: u64) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    fn archive(id: &str, title: &str, name: &str, file: &str) -> ShareArchive {
        ShareArchive {
            id: ArchiveId(id.to_string()),
            title: title.to_string(),
            name: name.to_string(),
            file: PathBuf::from(file),
            tags: "artist:someone".to_string(),
            summary: format!("summary of {id}"),
            pagecount: 24,
            arcsize: 1234,
            toc: vec![],
        }
    }

    fn share() -> Share {
        Share {
            id: ShareId("SHARE_ONE".to_string()),
            title: Some("My Share".to_string()),
            code_hash: String::new(),
            short_code: false,
            code_enc: String::new(),
            lookup_pubkey: String::new(),
            record_key: String::new(),
            items: vec![
                ShareItem::Archive {
                    id: ArchiveId("A".to_string()),
                },
                ShareItem::Tankoubon {
                    id: TankId("TANK".to_string()),
                },
                ShareItem::Archive {
                    id: ArchiveId("GONE".to_string()),
                },
            ],
            permissions: Permissions::ALL,
            download_limit: None,
            downloads_used: 0,
            import_bytes: 0,
            total_bytes: 0,
            target_ratio: None,
            ratio_action: RatioAction::Stop,
            ratio_rate: None,
            ip_allow: Vec::new(),
            ip_first_n: None,
            expires_at: None,
            embed: false,
            allow_helpers: true,
            helpers_disabled_at: None,
            revoked_at: None,
            created_at: 1,
            created_by: "tester".to_string(),
        }
    }

    /// Snapshot of the browse manifest: titles, tags, sizes, the filename, and how a tankoubon's
    /// members and a deleted item are represented. 023 moved this code between crates; a change in
    /// any of these values would be invisible to the import tests until a recipient noticed it.
    #[tokio::test]
    async fn the_browse_manifest_is_stable() {
        let host = StubHost {
            archives: vec![
                archive("A", "Alpha", "Alpha", "/library/5f/A.cbz"),
                archive("B", "  ", "Beta", "/library/9c/B.zip"),
            ],
            tankoubon: Some(ShareTankoubon {
                name: "Volume One".to_string(),
                summary: "the first volume".to_string(),
                tags: "artist:someone".to_string(),
                archives: vec![ArchiveId("B".to_string()), ArchiveId("A".to_string())],
            }),
            inner: EmptyHost,
        };

        let manifest = browse_manifest(&host, &share()).await;
        let value = serde_json::to_value(&manifest).unwrap();

        assert_eq!(
            value,
            json!({
                "share_id": "SHARE_ONE",
                "title": "My Share",
                "permissions": {"import": true, "read": true, "download": true},
                "items": [
                    {"Archive": {
                        "archive_id": "A",
                        "title": "Alpha",
                        "tags": "artist:someone",
                        "summary": "summary of A",
                        "pagecount": 24,
                        "filename": "A.cbz",
                        "size": 1234,
                        "blake3": null,
                        "categories": [],
                        "toc": []
                    }},
                    {"Tankoubon": {
                        "id": "TANK",
                        "name": "Volume One",
                        "tags": "artist:someone",
                        "summary": "the first volume",
                        "members": [
                            {
                                "archive_id": "B",
                                "title": "Beta",
                                "tags": "artist:someone",
                                "summary": "summary of B",
                                "pagecount": 24,
                                "filename": "B.zip",
                                "size": 1234,
                                "blake3": null,
                                "categories": [],
                                "toc": []
                            },
                            {
                                "archive_id": "A",
                                "title": "Alpha",
                                "tags": "artist:someone",
                                "summary": "summary of A",
                                "pagecount": 24,
                                "filename": "A.cbz",
                                "size": 1234,
                                "blake3": null,
                                "categories": [],
                                "toc": []
                            }
                        ]
                    }},
                    {"Unavailable": {"title": null}}
                ]
            })
        );
    }
}
