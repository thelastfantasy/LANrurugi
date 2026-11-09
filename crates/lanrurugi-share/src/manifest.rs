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
