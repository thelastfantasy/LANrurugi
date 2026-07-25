//! One-time migration: rewrites every archive's `rating:` tag from legacy's whole-star-repeat
//! encoding (`rating:⭐⭐⭐`, `.length`-counted) to this app's own decimal encoding (`rating:3`) —
//! see `apps/frontend/src/lib/rating.ts`'s own docs for why the switch happened (half-star support
//! needs a format that can represent a fraction at all, which repeated-emoji-counting can't).
//!
//! Not a permanent code path: the frontend's own `parseRating` already reads the old star-repeat
//! format correctly (falls back to counting `⭐` characters when the value isn't a plain decimal),
//! so nothing breaks without running this — it's purely a "make existing stored tags consistent
//! with what the UI now always writes" cleanup, run once against a real library.
//!
//! Run with: `cargo run -p lanrurugi-storage --example migrate_ratings -- <redis-base-url>`
//! (defaults to `redis://127.0.0.1:6379` if the URL argument is omitted).

use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::ArchiveRepository;

const STAR: char = '⭐';

/// Mirrors `apps/frontend/src/lib/rating.ts::parseRating`'s star-repeat fallback branch exactly —
/// this migration only ever needs to *read* the old format (to convert it), never the new one.
fn parse_legacy_star_rating(value: &str) -> Option<u32> {
    let count = value.chars().filter(|&c| c == STAR).count() as u32;
    (count > 0).then_some(count)
}

#[tokio::main]
async fn main() {
    let redis_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "redis://127.0.0.1:6379".to_string());

    let dbs = RedisDbs::connect(&redis_url).expect("failed to build Redis connection pool");
    let repo = ArchiveRepository::new(dbs.archive);

    let archives = repo.list_all().await.expect("failed to list archives");
    println!(
        "Scanning {} archives for legacy-format ratings...",
        archives.len()
    );

    let mut migrated = 0usize;
    for mut archive in archives {
        let Some((tag_index, new_value)) =
            archive.tags.split(',').enumerate().find_map(|(i, raw)| {
                let trimmed = raw.trim();
                let rest = trimmed.strip_prefix("rating:")?;
                let stars = parse_legacy_star_rating(rest)?;
                Some((i, stars.to_string()))
            })
        else {
            continue;
        };

        let mut parts: Vec<String> = archive
            .tags
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        parts[tag_index] = format!("rating:{new_value}");
        archive.tags = parts.join(", ");

        println!(
            "  {} ({}): rating tag -> rating:{new_value}",
            archive.id, archive.title
        );
        repo.save(&archive)
            .await
            .expect("failed to save migrated archive");
        migrated += 1;
    }

    println!("Done. Migrated {migrated} archive(s).");
}
