//! This instance's sharing identity and the share records it keeps: the node secret (also its
//! iroh key), the node code, and the code each share is opened with.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use hkdf::Hkdf;
use lanrurugi_share_proto::code::ShareCode;
use lanrurugi_share_proto::node_code::NodeCode;
use lanrurugi_share_proto::record::open_bytes;
use lanrurugi_storage::keys;
use lanrurugi_storage::shares::{Share, ShareRepository};
use sha2::Sha256;

use crate::host::ShareHost;
use crate::util::{hex, unhex};

/// Associated data sealed with each share's code, so a ciphertext cannot be moved to another field.
pub const CODE_ENC_AAD: &[u8] = b"lrrg-code-enc-v1";

pub fn repo(host: &dyn ShareHost) -> ShareRepository {
    ShareRepository::new(host.redis().config.clone())
}

/// This instance's long-term secret (also its iroh key), created on first use and included in
/// backups so the instance keeps its identity across restores.
pub async fn node_secret(config: &Pool) -> Result<[u8; 32], String> {
    let mut conn = config.get().await.map_err(|e| e.to_string())?;
    let existing: Option<String> = conn
        .get(keys::SHARE_NODE_SECRET_KEY)
        .await
        .map_err(|e| e.to_string())?;
    let stored = match existing {
        Some(s) => s,
        None => {
            let fresh: [u8; 32] = rand::random();
            let _: bool = conn
                .set_nx(keys::SHARE_NODE_SECRET_KEY, hex(&fresh))
                .await
                .map_err(|e| e.to_string())?;
            conn.get(keys::SHARE_NODE_SECRET_KEY)
                .await
                .map_err(|e| e.to_string())?
        }
    };
    unhex(&stored)
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| "the stored node secret is malformed".to_string())
}

/// This instance's node code, created once and kept forever (like a share code) until the user
/// rotates it.
pub async fn node_code(config: &Pool) -> Result<NodeCode, String> {
    let mut conn = config.get().await.map_err(|e| e.to_string())?;
    let existing: Option<String> = conn
        .get(keys::SHARE_NODE_CODE_KEY)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(stored) = existing {
        if let Ok(code) = NodeCode::parse(&stored) {
            return Ok(code);
        }
        tracing::warn!("the stored node code is malformed; minting a new one");
    }
    let fresh = NodeCode::generate();
    let _: bool = conn
        .set_nx(keys::SHARE_NODE_CODE_KEY, fresh.as_str())
        .await
        .map_err(|e| e.to_string())?;
    let stored: Option<String> = conn
        .get(keys::SHARE_NODE_CODE_KEY)
        .await
        .map_err(|e| e.to_string())?;
    let stored = stored.ok_or_else(|| "the node code could not be stored".to_string())?;
    NodeCode::parse(&stored).map_err(|e| e.to_string())
}

/// The key share codes are sealed with, derived from the node secret.
pub async fn code_key(config: &Pool) -> Result<[u8; 32], String> {
    let secret = node_secret(config).await?;
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, &secret)
        .expand(b"code-enc", &mut key)
        .expect("32 bytes is a valid HKDF output length");
    Ok(key)
}

/// The share's code, re-derived from `code_enc` (FR-006).
pub async fn share_code(config: &Pool, share: &Share) -> Option<ShareCode> {
    let key = code_key(config).await.ok()?;
    share_code_with(&key, share)
}

/// `share_code` for a caller that already fetched the key (a list decrypting several shares).
pub fn share_code_with(key: &[u8; 32], share: &Share) -> Option<ShareCode> {
    let plain = open_bytes(key, &unhex(&share.code_enc)?, CODE_ENC_AAD)?;
    ShareCode::parse(std::str::from_utf8(&plain).ok()?).ok()
}
