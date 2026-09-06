//! One-time migration: rewrites every Tankoubon's `chapter_names_` zset member from the
//! old HashMap format `{"id":"name",...}` to the new ordered-array format
//! `[{"id":"...","name":"..."}]`.
//!
//! The old format was introduced in commit `de32b85`; the new format replaced it in the
//! same session. Any data written between those two points still has the old shape.
//!
//! Run with: `cargo run -p lanrurugi-storage --example migrate_chapter_names -- <redis-url>`
//! (defaults to `redis://127.0.0.1:16379` if the URL argument is omitted).

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::GroupingRepository;

#[tokio::main]
async fn main() {
    let redis_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "redis://127.0.0.1:16379".to_string());

    let dbs = RedisDbs::connect(&redis_url).expect("failed to build Redis connection pool");
    let repo = GroupingRepository::new(dbs.archive.clone());

    let groupings = repo.list_all().await.expect("failed to list tankoubons");
    println!("Scanning {} tankoubons...", groupings.len());

    let mut conn = dbs
        .archive
        .get()
        .await
        .expect("failed to get Redis connection");
    let mut migrated = 0usize;

    for grouping in &groupings {
        let members: Vec<String> = conn
            .zrangebyscore(grouping.tankid.as_str(), -7isize, -7isize)
            .await
            .unwrap_or_default();

        let raw = match members
            .first()
            .and_then(|m| m.strip_prefix("chapter_names_"))
        {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        // Already in the new array format?
        if raw.trim_start().starts_with('[') {
            continue;
        }

        let old: std::collections::HashMap<String, String> = match serde_json::from_str(raw) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("  {} — parse error, skipping: {e}", grouping.tankid);
                continue;
            }
        };
        let old_len = old.len();

        let new_val = serde_json::to_string(
            &old.into_iter()
                .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let member = format!("chapter_names_{new_val}");
        let _: () = conn
            .zrembyscore(grouping.tankid.as_str(), -7isize, -7isize)
            .await
            .unwrap();
        let _: () = conn
            .zadd(grouping.tankid.as_str(), member, -7isize)
            .await
            .unwrap();

        println!(
            "  {} ({}): migrated {} chapter name(s)",
            grouping.tankid, grouping.name, old_len
        );
        migrated += 1;
    }

    println!("Done. Migrated {migrated} tankoubon(s).");
}
