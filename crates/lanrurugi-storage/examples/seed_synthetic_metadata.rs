//! Seeds Redis with N synthetic archive metadata records (no real files on disk) — purpose-built
//! for issue #70's recommendation-engine performance analysis at 100k-archive scale, with a focus
//! on the *local ONNX embedding* cost specifically (title/tag embedding + cosine-similarity
//! ranking never touch the filesystem or the LLM rerank step, so a real archive body is pure
//! overhead for this one measurement).
//!
//! Real-data seeded: this script first reads whatever archives already exist in the target Redis
//! (a real, if small, library) and reuses their actual titles/tags as a template pool — synthetic
//! records are built by recombining real title fragments (series name + a real artist tag +
//! swapped volume number) rather than fully-invented strings, so the embedding model sees the
//! same kind of near-duplicate-series semantic clustering a real 100k library would have, not
//! random noise. Falls back to a small built-in pool only if the target Redis is empty.
//!
//! Run with:
//! `cargo run -p lanrurugi-storage --example seed_synthetic_metadata -- <target-redis-url> [count] [--templates-from <source-redis-url>]`
//! (`target-redis-url` defaults to `redis://127.0.0.1:16379`, `count` to 100000). Without
//! `--templates-from`, real templates are read from `target-redis-url` itself before seeding
//! starts.
//!
//! This is throwaway benchmark data — point `target-redis-url` at a scratch Redis instance, not
//! a real library, and use `--templates-from <dev-redis-url>` to borrow real titles/tags without
//! writing anything back to that dev instance.

use lanrurugi_core::entities::{Archive, Grouping};
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::{ArchiveRepository, GroupingRepository};

/// Placeholder series names — synthetic, not real work titles (this fallback pool only kicks in
/// when the target Redis has no real archives to borrow real title/tag templates from; see the
/// module docs above). Mixed CJK/Latin on purpose, to still exercise the same multilingual
/// tokenization path a real library would.
const FALLBACK_SERIES_NAMES: &[&str] = &[
    "架空の異世界転生シリーズ",
    "架空の冒険者ギルド物語",
    "架空の魔法少女育成記",
    "架空の学園剣術譚",
    "fictional vampire chronicles",
    "fictional last summoner saga",
    "fictional cosplay photo collection",
    "fictional anthology volume",
];

/// Placeholder artist handles — synthetic, not real circle/artist names (see
/// `FALLBACK_SERIES_NAMES`'s own doc for why this pool exists at all).
const FALLBACK_ARTISTS: &[&str] = &[
    "artist-alpha",
    "artist-beta",
    "artist-gamma",
    "artist-delta",
    "artist-epsilon",
];

/// A real archive's title, stripped of any trailing volume/chapter marker, plus its own
/// `artist:`-namespaced tags — the two fields the embedder and LLM rerank actually look at.
struct SeriesTemplate {
    base_title: String,
    artist_tags: Vec<String>,
}

/// Strips a trailing volume/chapter number so recombination doesn't produce titles like
/// "Foo 第3巻 第7巻" — handles the three numbering styles seen in this codebase's own test
/// fixtures and real plugin output: kanji-counter volumes (「巻の壱」style), CJK "第N卷/巻/話",
/// and a bare "Vol.N"/"#N" suffix.
fn strip_volume_marker(title: &str) -> String {
    let markers = ["巻の", "第", "Vol.", "Vol ", "#"];
    for m in markers {
        if let Some(pos) = title.rfind(m) {
            // Only strip if the marker is reasonably near the end (avoids chopping a title whose
            // *subject matter* happens to contain one of these substrings earlier on).
            if title.len() - pos < 20 {
                return title[..pos].trim_end().to_string();
            }
        }
    }
    title.to_string()
}

async fn load_real_templates(archives: &ArchiveRepository) -> Vec<SeriesTemplate> {
    let real = archives.list_all().await.unwrap_or_default();
    real.iter()
        .map(|a| {
            let artist_tags: Vec<String> = a
                .tags
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| t.starts_with("artist:"))
                .collect();
            SeriesTemplate {
                base_title: strip_volume_marker(&a.title),
                artist_tags,
            }
        })
        .collect()
}

fn fallback_templates() -> Vec<SeriesTemplate> {
    FALLBACK_SERIES_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| SeriesTemplate {
            base_title: name.to_string(),
            artist_tags: vec![format!(
                "artist:{}",
                FALLBACK_ARTISTS[i % FALLBACK_ARTISTS.len()]
            )],
        })
        .collect()
}

/// 64-bit finalizer (splitmix64-style) — matches `lanrurugi-bench`'s own `mix64`, reused here so
/// this generator's output is exactly as deterministic/reproducible given the same index.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

fn synthetic_archive_id(index: usize) -> ArchiveId {
    // Real archive IDs are exactly 40 lowercase-hex chars (SHA-1 digest length,
    // `lanrurugi_storage::id::ARCHIVE_ID_LEN`) — anything else fails `ArchiveId`'s own format
    // validation and every lookup 404s. 16+16+8 hex digits = 40 chars exactly (`{:08x}` on a u32,
    // not a u64 — the earlier version of this formatted `h3` as a u64 with `{:08x}`, which prints
    // its *full* 16 hex digits since `08` is a minimum width, not a truncation, silently producing
    // 48-char ids that were never valid `ArchiveId`s at all). Salted so re-running this script
    // doesn't collide with a previous run over the same index range.
    let h1 = mix64((index as u64) ^ 0xA5A5_A5A5_A5A5_A5A5);
    let h2 = mix64(h1 ^ 0x9E3779B97F4A7C15);
    let h3 = mix64(h2 ^ 0xBF58476D1CE4E5B9) as u32;
    ArchiveId(format!("{h1:016x}{h2:016x}{h3:08x}"))
}

fn synthetic_archive(index: usize, templates: &[SeriesTemplate]) -> Archive {
    let h = mix64(index as u64);
    let template = &templates[index % templates.len()];
    let volume = (index % 30) + 1;
    let title = format!("{} 第{volume}巻", template.base_title);
    let pagecount = 60 + (h % 180) as u32;
    let mut tag_parts: Vec<String> = template.artist_tags.clone();
    tag_parts.push("category:manga".to_string());
    tag_parts.push(format!("date_added:{}", 1_700_000_000 + index as u64));
    tag_parts.push("source:synthetic-bench".to_string());
    let tags = tag_parts.join(",");

    Archive {
        id: synthetic_archive_id(index),
        name: title.clone(),
        title,
        file: format!("/synthetic/archive_{index:06}.zip"),
        tags,
        summary: String::new(),
        arcsize: 50_000_000 + (h % 100_000_000),
        pagecount,
        isnew: index.is_multiple_of(10),
        lastreadpage: 0,
        lastreadtime: 0,
        thumbhash: None,
        toc: Vec::new(),
        stamp_ids: Vec::new(),
        heal_failed_at: None,
        corrupted_pages: Vec::new(),
        has_patch: false,
    }
}

#[tokio::main]
async fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let mut templates_from: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw_args.len() {
        if raw_args[i] == "--templates-from" {
            templates_from = raw_args.get(i + 1).cloned();
            i += 2;
        } else {
            positional.push(raw_args[i].clone());
            i += 1;
        }
    }
    let redis_url = positional
        .first()
        .cloned()
        .unwrap_or_else(|| "redis://127.0.0.1:16379".to_string());
    let count: usize = positional
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    let dbs = RedisDbs::connect(&redis_url).expect("failed to build target Redis connection pool");
    let archives = ArchiveRepository::new(dbs.archive.clone());
    let groupings = GroupingRepository::new(dbs.archive.clone());

    let template_source_url = templates_from.clone().unwrap_or_else(|| redis_url.clone());
    let template_archives = if templates_from.is_some() {
        let template_dbs = RedisDbs::connect(&template_source_url)
            .expect("failed to build template-source Redis connection pool");
        ArchiveRepository::new(template_dbs.archive)
    } else {
        ArchiveRepository::new(dbs.archive.clone())
    };

    let mut templates = load_real_templates(&template_archives).await;
    if templates.is_empty() {
        println!(
            "No real archives found in {template_source_url} — using built-in fallback title pool."
        );
        templates = fallback_templates();
    } else {
        println!(
            "Loaded {} real title/tag template(s) from {template_source_url}.",
            templates.len()
        );
    }

    println!(
        "Seeding {count} synthetic archives into {redis_url} (recombined from real templates) ..."
    );
    let start = std::time::Instant::now();

    let mut tank_members: Vec<ArchiveId> = Vec::new();
    for i in 0..count {
        let archive = synthetic_archive(i, &templates);
        // Every 20th archive joins one running synthetic Tankoubon (rotated every 6 members) —
        // enough real Tankoubon membership data to exercise the sibling-exclusion filter without
        // making every recommendation candidate a tank member (which would defeat the point of
        // testing "excluded vs eligible" at realistic ratios).
        if i.is_multiple_of(20) {
            tank_members.push(archive.id.clone());
        }
        archives
            .save(&archive)
            .await
            .unwrap_or_else(|e| panic!("failed to save archive {i}: {e}"));

        if tank_members.len() == 6 {
            let tank_index = i / 20 / 6;
            let tank = Grouping {
                tankid: TankId(format!("TANK_{:010}", 1_700_000_000 + tank_index)),
                name: format!("Synthetic Tankoubon {tank_index:04}"),
                summary: String::new(),
                tags: String::new(),
                progress: 0,
                archives: std::mem::take(&mut tank_members),
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Vec::new(),
                created_at: None,
                updated_at: None,
            };
            groupings
                .save(&tank)
                .await
                .unwrap_or_else(|e| panic!("failed to save tankoubon {tank_index}: {e}"));
        }

        if i > 0 && i % 10_000 == 0 {
            println!(
                "  {i}/{count} ({:.1}s elapsed)",
                start.elapsed().as_secs_f64()
            );
        }
    }

    println!(
        "Done. Seeded {count} archives in {:.1}s.",
        start.elapsed().as_secs_f64()
    );
}
