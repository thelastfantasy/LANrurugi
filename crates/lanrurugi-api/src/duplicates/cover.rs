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

/// What one hashing pass produced.
pub struct CoverHashes {
    /// id → 64-bit perceptual hash.
    pub hashes: HashMap<String, [u8; 8]>,
    /// Archives whose cover could not be read (deleted thumbnail and no regeneration possible).
    pub missing: usize,
}

/// Hashes every archive's cover, reusing anything already in the cache.
pub async fn collect(state: &AppState, ids: &[String]) -> CoverHashes {
    let mut cached: HashMap<String, [u8; 8]> = HashMap::new();
    let mut conn = state.redis.config.get().await.ok();
    if let Some(conn) = conn.as_mut() {
        let all: HashMap<String, String> = conn.hgetall(COVER_HASH_KEY).await.unwrap_or_default();
        for id in ids {
            if let Some(raw) = all.get(id).and_then(|v| v.strip_prefix(PREFIX)) {
                if let Some(bytes) = parse_hex8(raw) {
                    cached.insert(id.clone(), bytes);
                }
            }
        }
    }

    let mut hashes = HashMap::with_capacity(ids.len());
    let mut missing = 0usize;
    let mut fresh: Vec<(String, String)> = Vec::new();

    for id in ids {
        if let Some(bytes) = cached.get(id) {
            hashes.insert(id.clone(), *bytes);
            continue;
        }
        match hash_one(state, id).await {
            Some(bytes) => {
                hashes.insert(id.clone(), bytes);
                fresh.push((id.clone(), hex8(&bytes)));
            }
            None => missing += 1,
        }
    }

    if let Some(conn) = conn.as_mut() {
        for (id, value) in fresh {
            let _: Result<(), _> = conn
                .hset(COVER_HASH_KEY, &id, format!("{PREFIX}{value}"))
                .await;
        }
    }

    CoverHashes { hashes, missing }
}

async fn hash_one(state: &AppState, id: &str) -> Option<[u8; 8]> {
    let archive_id = lanrurugi_core::ids::ArchiveId(id.to_string());
    let (_, bytes) = crate::archives::share_thumbnail(state, &archive_id, 0).await?;
    // `share_thumbnail` never returns `None` for a valid id — it falls back to a shared placeholder
    // PNG. Hashing that would make every cover-less archive look identical, so treat it as missing
    // (and don't cache it: a real thumbnail may appear later without the id changing).
    if crate::archives::is_placeholder_thumbnail(&bytes) {
        return None;
    }
    let image = lanrurugi_core::concurrency::run_blocking(move || image::load_from_memory(&bytes))
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
