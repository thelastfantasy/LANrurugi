//! One-time migration: repairs two data bugs that affected archives ingested through the Upload
//! page's local-file-upload feature before their root causes were fixed in
//! `crates/lanrurugi-api/src/upload.rs` and `crates/lanrurugi-api/src/download_manager/ingest.rs`:
//!
//! 1. `pagecount` stuck at 0 — the staging file written during upload had no extension, so
//!    `archive_format::list_pages`'s pure extension-based format check always failed and silently
//!    fell back to 0 (mirrors a bug already fixed on the download side in `stream.rs`). The
//!    already-catalogued archive's real on-disk file (in `archive_dir`) is unaffected — only the
//!    transient staging file lacked an extension — so this is a pure re-read-and-persist fix, not
//!    a re-ingest. Scanned for across *every* archive with `pagecount: 0` (not just ones
//!    reachable via a still-live download-queue item — a dev/test Redis with no persistence
//!    enabled loses that queue history on restart, and in any case an archive with real,
//!    readable pages should never legitimately have `pagecount: 0` regardless of how it got that
//!    way), not just ones a queue lookup can confirm as local uploads.
//! 2. A bogus `source:<filename>` tag — `catalogue_staged_file` unconditionally stamped
//!    `source:<source_url>` using whatever was passed in, and the upload path passed the uploaded
//!    file's own name (there being no real external source for a local upload) rather than
//!    omitting the tag entirely. Detected by comparing the tag's value against the archive's own
//!    filename (trimmed the same way `trim_url` would) — a real download's `source:` is some
//!    other site's URL and will never match the archive's own on-disk filename, so this can't
//!    misfire on a genuine `source:e-hentai.org/...`-style tag.
//!
//! Run with: `cargo run -p lanrurugi-scanner --example fix_local_upload_metadata -- <redis-base-url>`
//! (defaults to `redis://127.0.0.1:6379` if the URL argument is omitted).

use std::path::Path;

use lanrurugi_scanner::archive_format;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::ArchiveRepository;

/// Same trimming `crate::plugins::trim_url` (lanrurugi-api) applies before stamping a `source:`
/// tag — reproduced here (that function is crate-private and this is a different crate) purely so
/// a bogus `source:<filename>` tag written under the old buggy behavior is recognized regardless
/// of which of these it happened to strip.
fn trim_like_source_tag(value: &str) -> String {
    let mut v = value.trim();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = v.strip_prefix(prefix) {
            v = rest;
            break;
        }
    }
    let v = v.strip_prefix("www.").unwrap_or(v);
    let v = v.split('?').next().unwrap_or(v);
    v.strip_suffix('/').unwrap_or(v).to_string()
}

#[tokio::main]
async fn main() {
    let redis_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "redis://127.0.0.1:6379".to_string());

    let dbs = RedisDbs::connect(&redis_url).expect("failed to build Redis connection pool");
    let archives = ArchiveRepository::new(dbs.archive);

    let all = archives.list_all().await.expect("failed to list archives");
    println!("Scanning {} archive(s).", all.len());

    let mut fixed_pagecount = 0usize;
    let mut fixed_source_tag = 0usize;

    for mut archive in all {
        let mut changed = false;

        if archive.pagecount == 0 {
            match archive_format::list_pages(Path::new(&archive.file)) {
                Ok(pages) if !pages.is_empty() => {
                    println!(
                        "  {} ({}): pagecount 0 -> {}",
                        archive.id,
                        archive.title,
                        pages.len()
                    );
                    archive.pagecount = pages.len() as u32;
                    changed = true;
                    fixed_pagecount += 1;
                }
                Ok(_) => println!(
                    "  {} ({}): re-read gave 0 pages, leaving as-is",
                    archive.id, archive.title
                ),
                Err(e) => println!(
                    "  {} ({}): failed to re-read pages ({e}), leaving as-is",
                    archive.id, archive.title
                ),
            }
        }

        let own_filename = Path::new(&archive.file)
            .file_name()
            .and_then(|n| n.to_str())
            .map(trim_like_source_tag);

        let without_bogus_source: Vec<&str> = archive
            .tags
            .split(',')
            .map(str::trim)
            .filter(|t| {
                let Some(value) = t.to_lowercase().strip_prefix("source:").map(str::to_string)
                else {
                    return true;
                };
                let Some(own) = &own_filename else {
                    return true;
                };
                trim_like_source_tag(&value).to_lowercase() != own.to_lowercase()
            })
            .collect();
        if without_bogus_source.len() != archive.tags.split(',').count() {
            println!(
                "  {} ({}): removing bogus source: tag (matched own filename)",
                archive.id, archive.title
            );
            archive.tags = without_bogus_source.join(", ");
            changed = true;
            fixed_source_tag += 1;
        }

        if changed {
            archives
                .save(&archive)
                .await
                .expect("failed to save archive");
        }
    }

    println!(
        "Done. Fixed pagecount on {fixed_pagecount} archive(s), removed bogus source: tag on {fixed_source_tag} archive(s)."
    );
}
