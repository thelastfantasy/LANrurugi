//! Cover-image perceptual hashing for duplicate detection.
//!
//! Legacy compared a SHA-1 of the cover's *bytes* (the `thumbhash` field), which is why a
//! re-encoded or resized scan of the same cover never matched. A 64-bit average hash survives
//! re-encoding, resizing and mild cropping, and the archive id is itself a content fingerprint, so
//! a cached value can never be stale for the archive it is keyed by (a changed file gets a new id).

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use image::DynamicImage;

use crate::AppState;

/// Redis hash (config DB): archive id → `v1:<16 hex chars>`.
pub const COVER_HASH_KEY: &str = "LANRURUGI_DUP_COVER_HASH";
const PREFIX: &str = "v1:";

/// The cached hashes of those `ids` that have one. Every archive's cover is hashed once; a scan
/// reads these first so a rescan has nearly everything before it hashes a single cover.
pub async fn cached(state: &AppState, ids: &[String]) -> HashMap<String, [u8; 8]> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return HashMap::new();
    };
    let all: HashMap<String, String> = conn.hgetall(COVER_HASH_KEY).await.unwrap_or_default();
    ids.iter()
        .filter_map(|id| {
            let raw = all.get(id)?.strip_prefix(PREFIX)?;
            Some((id.clone(), parse_hex8(raw)?))
        })
        .collect()
}

/// Hashes one archive's stored cover thumbnail and caches the result. `None` when it has none.
pub async fn hash_and_store(state: &AppState, id: &str) -> Option<[u8; 8]> {
    let bytes = hash_one(state, id).await?;
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn
            .hset(COVER_HASH_KEY, id, format!("{PREFIX}{}", hex8(&bytes)))
            .await;
    }
    Some(bytes)
}

/// Hashes the cover thumbnail already on disk. An archive without one is skipped rather than given
/// a thumbnail here: generating covers for a whole library decodes a page per archive, which is
/// what made a scan slow and memory-hungry. Such archives are counted and reported instead.
async fn hash_one(state: &AppState, id: &str) -> Option<[u8; 8]> {
    let bytes = crate::archives::stored_cover_thumbnail(state, id).await?;
    if crate::archives::is_placeholder_thumbnail(&bytes) {
        return None;
    }
    let image = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::image_decode::load_from_memory(&bytes)
    })
    .await
    .ok()?
    .ok()?;
    Some(hash_image(&image))
}

fn hash_image(image: &DynamicImage) -> [u8; 8] {
    let hash = lanrurugi_imgcompare::alignment::hash_page(image);
    let mut out = [0u8; 8];
    for (slot, byte) in out.iter_mut().zip(hash.as_bytes()) {
        *slot = *byte;
    }
    out
}

/// Hamming distance over two 64-bit perceptual hashes.
pub fn distance(a: &[u8; 8], b: &[u8; 8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

fn hex8(bytes: &[u8; 8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_hex8(raw: &str) -> Option<[u8; 8]> {
    if raw.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(raw.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_differing_bits() {
        let a = [0u8; 8];
        let mut b = [0u8; 8];
        b[0] = 0b0000_0111;
        assert_eq!(distance(&a, &b), 3);
        assert_eq!(distance(&a, &a), 0);
    }

    #[test]
    fn hex_round_trip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04];
        assert_eq!(parse_hex8(&hex8(&bytes)), Some(bytes));
        assert_eq!(parse_hex8("not-hex"), None);
    }
}
