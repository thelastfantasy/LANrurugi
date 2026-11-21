//! Moves what the reader attached to a superseded revision onto the revision that replaced it.
//!
//! `always_overwrite` (issue #107) deletes the older revision once the newer one is catalogued, and
//! that delete takes the archive's bookmarks, stamps, reading position and category membership with
//! it. The newer revision is the same work, usually with a few pages added, removed or fixed, so
//! those are carried across instead, each page followed in this order:
//!
//! 1. the same entry name in the new archive;
//! 2. the same bytes (SHA-1) under another name;
//! 3. the same page number, if the new archive has that many pages.
//!
//! A page none of these find is dropped, along with whatever was on it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lanrurugi_core::entities::Stamp;
use lanrurugi_core::ids::{ArchiveId, StampId};
use lanrurugi_scanner::patch::EffectivePage;
use lanrurugi_storage::bookmarks::Bookmark;

use crate::AppState;

type PageHash = [u8; 20];

/// How to find one old page again: its entry name and, for a page stored in the archive itself, its
/// content hash.
#[derive(Debug, Clone, PartialEq)]
struct PageKey {
    name: String,
    hash: Option<PageHash>,
}

/// Everything to carry, read from the old revision before it is deleted.
pub(crate) struct Carryover {
    old_id: ArchiveId,
    progress: u32,
    bookmarks: Vec<Bookmark>,
    stamps: Vec<Stamp>,
    categories: Vec<String>,
    /// Old 1-based page number → how to find it again, for every page something above sits on.
    pages: HashMap<u32, PageKey>,
}

/// Reads what `old_id` carries. `None` when there is nothing to carry, or the record is gone.
pub(crate) async fn capture(state: &AppState, old_id: &ArchiveId) -> Option<Carryover> {
    let archive = state.repos.archives.get(old_id).await.ok().flatten()?;
    let bookmarks = state
        .bookmarks
        .list_for_archive(old_id.as_str())
        .await
        .unwrap_or_default();
    let mut stamps = Vec::new();
    for stamp_id in &archive.stamp_ids {
        if let Ok(Some(stamp)) = state.repos.stamps.get(stamp_id).await {
            stamps.push(stamp);
        }
    }
    let categories: Vec<String> = state
        .repos
        .categories
        .for_archive(old_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !c.is_dynamic())
        .map(|c| c.catid.to_string())
        .collect();
    if bookmarks.is_empty()
        && stamps.is_empty()
        && categories.is_empty()
        && archive.lastreadpage == 0
    {
        return None;
    }

    let wanted: HashSet<u32> = bookmarks
        .iter()
        .map(|b| b.page)
        .chain(stamps.iter().filter_map(Stamp::page))
        .chain((archive.lastreadpage > 0).then_some(archive.lastreadpage))
        .collect();
    let file = archive.file.clone();
    let pages = tokio::task::spawn_blocking(move || keys_for(Path::new(&file), &wanted))
        .await
        .unwrap_or_default();

    Some(Carryover {
        old_id: old_id.clone(),
        progress: archive.lastreadpage,
        bookmarks,
        stamps,
        categories,
        pages,
    })
}

/// The effective page list (what the reader numbers), with patch-sourced pages marked.
/// `blake3` is the archive's record's, when the caller has it.
fn effective_pages(path: &Path, blake3: Option<&str>) -> Vec<EffectivePage> {
    match lanrurugi_scanner::archive_format::list_pages(path) {
        Ok(names) => lanrurugi_scanner::patch::effective_pages(path, &names, blake3),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not list pages to carry across a revision");
            Vec::new()
        }
    }
}

/// SHA-1 of each page stored in the archive itself; `None` for a patch-sourced page or a read failure.
fn hashes_of(path: &Path, pages: &[EffectivePage]) -> Vec<Option<PageHash>> {
    let original: Vec<String> = pages
        .iter()
        .filter_map(|p| match p {
            EffectivePage::Original(name) => Some(name.clone()),
            EffectivePage::Patched(_) => None,
        })
        .collect();
    let by_name: HashMap<String, Option<PageHash>> =
        match lanrurugi_scanner::archive_format::hash_entries(path, &original) {
            Ok(hashes) => original.into_iter().zip(hashes).collect(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not hash pages to carry across a revision");
                HashMap::new()
            }
        };
    pages
        .iter()
        .map(|p| match p {
            EffectivePage::Original(name) => by_name.get(name).copied().flatten(),
            EffectivePage::Patched(_) => None,
        })
        .collect()
}

fn keys_for(path: &Path, wanted: &HashSet<u32>) -> HashMap<u32, PageKey> {
    let pages = effective_pages(path, None);
    let chosen: Vec<(u32, EffectivePage)> = pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| (index as u32 + 1, page))
        .filter(|(number, _)| wanted.contains(number))
        .collect();
    let only: Vec<EffectivePage> = chosen.iter().map(|(_, page)| page.clone()).collect();
    let hashes = hashes_of(path, &only);
    chosen
        .into_iter()
        .zip(hashes)
        .map(|((number, page), hash)| {
            (
                number,
                PageKey {
                    name: page.entry_name().to_string(),
                    hash,
                },
            )
        })
        .collect()
}

/// Where an old page lands in the new revision — see this module's docs for the order. Pure.
fn map_page(
    old_page: u32,
    key: Option<&PageKey>,
    new_names: &[String],
    new_hashes: Option<&[Option<PageHash>]>,
) -> Option<u32> {
    let position = |index: usize| Some(index as u32 + 1);
    if let Some(key) = key {
        if let Some(index) = new_names.iter().position(|n| *n == key.name) {
            return position(index);
        }
        if let (Some(hash), Some(new_hashes)) = (key.hash, new_hashes) {
            if let Some(index) = new_hashes.iter().position(|h| *h == Some(hash)) {
                return position(index);
            }
        }
    }
    (old_page >= 1 && old_page as usize <= new_names.len()).then_some(old_page)
}

/// Puts what `plan` carried onto `new_id`. Call after the old revision is deleted: a stamp keeps
/// its own key's timestamp, which could otherwise collide with the old one's key.
pub(crate) async fn apply(state: &AppState, plan: Carryover, new_id: &ArchiveId) {
    let Ok(Some(new_archive)) = state.repos.archives.get(new_id).await else {
        return;
    };
    let file = new_archive.file.clone();
    let blake3 = new_archive.blake3.clone();
    let new_pages =
        tokio::task::spawn_blocking(move || effective_pages(Path::new(&file), blake3.as_deref()))
            .await
            .unwrap_or_default();
    let new_names: Vec<String> = new_pages
        .iter()
        .map(|p| p.entry_name().to_string())
        .collect();
    // The whole new archive is hashed only when some old page has a hash to match and its name is
    // gone: most revisions keep their page names, and hashing reads every page.
    let needs_hashes = plan
        .pages
        .values()
        .any(|k| k.hash.is_some() && !new_names.contains(&k.name));
    let new_hashes = if needs_hashes {
        let file = new_archive.file.clone();
        tokio::task::spawn_blocking(move || hashes_of(Path::new(&file), &new_pages))
            .await
            .ok()
    } else {
        None
    };
    let hashes = new_hashes.as_deref();
    let target = |old_page: u32| map_page(old_page, plan.pages.get(&old_page), &new_names, hashes);

    let mut moved = 0usize;
    let mut dropped = 0usize;

    let mut placed: HashSet<u32> = HashSet::new();
    for bookmark in &plan.bookmarks {
        let Some(page) = target(bookmark.page) else {
            dropped += 1;
            continue;
        };
        // Two bookmarks that land on one page become one; the first name found is kept.
        if !placed.insert(page) {
            continue;
        }
        if state
            .bookmarks
            .add(new_id.as_str(), page, bookmark.bookmarked_at)
            .await
            .is_ok()
        {
            moved += 1;
            if let Some(name) = bookmark.name.as_deref() {
                let _ = state
                    .bookmarks
                    .set_name(new_id.as_str(), page, Some(name))
                    .await;
            }
        }
    }

    for stamp in &plan.stamps {
        let Some(page) = stamp.page().and_then(target) else {
            dropped += 1;
            continue;
        };
        let mut millis = stamp_millis(&stamp.stamp_id).unwrap_or(0);
        // Keeps the original time in the key, nudged forward only if another stamp already holds it.
        while let Ok(Some(_)) = state
            .repos
            .stamps
            .get(&StampId(format!("STAMPS_{page}_{millis}")))
            .await
        {
            millis += 1;
        }
        match state
            .repos
            .stamps
            .create(
                new_id,
                page,
                &stamp.content,
                &stamp.position,
                &stamp.icon,
                &stamp.rect,
                millis,
            )
            .await
        {
            Ok(_) => moved += 1,
            Err(e) => {
                tracing::warn!(%new_id, error = %e, "could not carry a stamp across a revision")
            }
        }
    }

    if plan.progress > 0 && new_archive.lastreadpage == 0 {
        if let Some(page) = target(plan.progress) {
            if let Err(e) = state
                .repos
                .archives
                .set_progress(new_id, page, None, now_secs())
                .await
            {
                tracing::warn!(%new_id, error = %e, "could not carry the reading position across a revision");
            }
        }
    }

    for catid in &plan.categories {
        if let Err(e) =
            crate::categories::add_archive_to_category(state, catid, new_id.as_str()).await
        {
            tracing::warn!(%new_id, catid, error = ?e, "could not carry a category across a revision");
        }
    }

    tracing::info!(
        old = %plan.old_id,
        new = %new_id,
        moved,
        dropped,
        categories = plan.categories.len(),
        "carried bookmarks and stamps across a revision"
    );
}

/// The millisecond timestamp in a `STAMPS_<page>_<ms>` key.
fn stamp_millis(stamp_id: &StampId) -> Option<u64> {
    stamp_id.as_str().rsplit('_').next()?.parse().ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn key(name: &str, hash: Option<u8>) -> PageKey {
        PageKey {
            name: name.to_string(),
            hash: hash.map(|b| [b; 20]),
        }
    }

    #[test]
    fn a_page_follows_its_name_first() {
        // A page inserted before it: same name, new position.
        let new = names(&["001.jpg", "001a.jpg", "002.jpg"]);
        assert_eq!(
            map_page(2, Some(&key("002.jpg", Some(1))), &new, None),
            Some(3)
        );
    }

    #[test]
    fn a_renamed_page_follows_its_bytes() {
        let new = names(&["p1.jpg", "p2.jpg", "p3.jpg"]);
        let hashes = vec![Some([9; 20]), Some([9; 20]), Some([1; 20])];
        assert_eq!(
            map_page(2, Some(&key("002.jpg", Some(1))), &new, Some(&hashes)),
            Some(3)
        );
    }

    #[test]
    fn otherwise_it_keeps_its_number_while_the_new_archive_has_one() {
        let new = names(&["x1.jpg", "x2.jpg"]);
        let hashes = vec![Some([7; 20]), Some([8; 20])];
        assert_eq!(
            map_page(2, Some(&key("002.jpg", Some(1))), &new, Some(&hashes)),
            Some(2)
        );
        // Also when nothing was known about the old page.
        assert_eq!(map_page(1, None, &new, None), Some(1));
    }

    #[test]
    fn a_page_past_the_end_with_no_match_is_dropped() {
        let new = names(&["x1.jpg", "x2.jpg"]);
        assert_eq!(map_page(5, Some(&key("005.jpg", None)), &new, None), None);
        assert_eq!(map_page(0, None, &new, None), None);
    }

    #[test]
    fn stamp_keys_keep_their_time() {
        assert_eq!(
            stamp_millis(&StampId("STAMPS_12_1700000000123".into())),
            Some(1700000000123)
        );
        assert_eq!(stamp_millis(&StampId("garbage".into())), None);
    }

    fn zip_with(dir: &Path, name: &str, pages: &[(&str, &str)]) -> String {
        use std::io::Write as _;
        let path = dir.join(name);
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (entry, content) in pages {
            writer.start_file(*entry, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        path.to_string_lossy().into_owned()
    }

    fn archive(id: &str, file: String, pagecount: u32) -> lanrurugi_core::entities::Archive {
        lanrurugi_core::entities::Archive {
            id: ArchiveId(id.to_string()),
            name: id.to_string(),
            title: id.to_string(),
            file,
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
            blake3: None,
            primary_source: None,
            progress_pages: None,
        }
    }

    /// The whole path against Redis: a page found by name, one by bytes after a rename, one by
    /// number, and one dropped past the end of the shorter new revision.
    #[tokio::test]
    async fn bookmarks_and_stamps_follow_their_pages_to_the_new_revision() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let old_file = zip_with(
            dir.path(),
            "old.zip",
            &[
                ("01.jpg", "a"),
                ("02.jpg", "b"),
                ("03.jpg", "c"),
                ("04.jpg", "d"),
                ("05.jpg", "e"),
            ],
        );
        // 02 gains a page before it; 03's bytes are renamed to x.jpg; 04 is gone with no match
        // but page 4 still exists; 05 is gone and the archive now ends at page 4.
        let new_file = zip_with(
            dir.path(),
            "new.zip",
            &[
                ("01.jpg", "a"),
                ("01b.jpg", "new"),
                ("02.jpg", "b"),
                ("x.jpg", "c"),
            ],
        );
        let old_id = ArchiveId(format!("carry-old-{}", uuid::Uuid::new_v4()));
        let new_id = ArchiveId(format!("carry-new-{}", uuid::Uuid::new_v4()));
        let mut old = archive(old_id.as_str(), old_file, 5);
        old.lastreadpage = 3;
        state.repos.archives.create_or_replace(&old).await.unwrap();
        state
            .repos
            .archives
            .create_or_replace(&archive(new_id.as_str(), new_file, 4))
            .await
            .unwrap();
        for page in [2, 3, 4, 5] {
            state
                .bookmarks
                .add(old_id.as_str(), page, 100)
                .await
                .unwrap();
        }
        state
            .bookmarks
            .set_name(old_id.as_str(), 2, Some("named"))
            .await
            .unwrap();
        let stamp = state
            .repos
            .stamps
            .create(&old_id, 3, "note", "10,10", "", "", 1_700_000_000_000)
            .await
            .unwrap();

        let plan = capture(&state, &old_id).await.expect("something to carry");
        // What `purge_one_archive` does to these before `apply` runs.
        state
            .bookmarks
            .remove_all_for_archive(old_id.as_str())
            .await
            .unwrap();
        state.repos.stamps.delete_many_raw(&[stamp]).await.unwrap();
        state.repos.archives.delete(&old_id).await.unwrap();
        apply(&state, plan, &new_id).await;

        let mut pages: Vec<(u32, Option<String>)> = state
            .bookmarks
            .list_for_archive(new_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.page, b.name))
            .collect();
        pages.sort();
        assert_eq!(
            pages,
            vec![(3, Some("named".to_string())), (4, None)],
            "02.jpg by name -> 3; 03 by bytes -> 4 (x.jpg); 04 by number -> 4, merged; 05 dropped"
        );
        let new = state.repos.archives.get(&new_id).await.unwrap().unwrap();
        assert_eq!(
            new.lastreadpage, 4,
            "the reading position follows page 3's bytes"
        );
        assert_eq!(new.stamp_ids.len(), 1);
        let carried = state
            .repos
            .stamps
            .get(&new.stamp_ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(carried.page(), Some(4));
        assert_eq!(carried.content, "note");

        state
            .bookmarks
            .remove_all_for_archive(new_id.as_str())
            .await
            .unwrap();
        state
            .repos
            .stamps
            .delete_many_raw(&new.stamp_ids)
            .await
            .unwrap();
        state.repos.archives.delete(&new_id).await.unwrap();
    }
}
