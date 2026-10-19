//! Library-open/bootstrap routine (User Story 1): verifies Redis is reachable and the archive
//! folder exists, with no destructive step — pointing LANrurugi at an existing legacy library
//! must never itself alter that library (constitution Principle I). The one write: a brand-new
//! instance with no `LRR_CONFIG` at all gets the settings every reader requires.

use std::path::Path;

use thiserror::Error;

use crate::keys::CONFIG_KEY;
use crate::redis::RedisDbs;

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("archive folder {0:?} does not exist or is not a directory")]
    ArchiveDirUnreachable(std::path::PathBuf),
    #[error("could not reach Redis (archive DB): {0}")]
    RedisUnreachable(#[from] deadpool_redis::PoolError),
    #[error("Redis PING did not return PONG")]
    RedisNotResponding,
}

pub async fn bootstrap(redis: &RedisDbs, archive_dir: &Path) -> Result<(), BootstrapError> {
    if !archive_dir.is_dir() {
        return Err(BootstrapError::ArchiveDirUnreachable(
            archive_dir.to_path_buf(),
        ));
    }

    let mut conn = redis.archive.get().await?;
    let pong: String = deadpool_redis::redis::cmd("PING")
        .query_async(&mut conn)
        .await
        .map_err(|_| BootstrapError::RedisNotResponding)?;
    if pong != "PONG" {
        return Err(BootstrapError::RedisNotResponding);
    }

    initialize_fresh_config(redis).await
}

/// Settings a fresh instance starts with that readers treat as required, not defaulted.
const FRESH_CONFIG: &[(&str, &str)] = &[("guestmode", "0")];

/// Writes [`FRESH_CONFIG`] only when `LRR_CONFIG` does not exist yet. An existing config missing a
/// field is deliberately left alone: such an instance predates the field and is migrated once,
/// explicitly (specs/007-guest-restricted-access research.md), not silently defaulted here.
async fn initialize_fresh_config(redis: &RedisDbs) -> Result<(), BootstrapError> {
    let mut conn = redis.config.get().await?;
    let exists: bool = deadpool_redis::redis::cmd("EXISTS")
        .arg(CONFIG_KEY)
        .query_async(&mut conn)
        .await
        .map_err(|_| BootstrapError::RedisNotResponding)?;
    if !exists {
        let mut hset = deadpool_redis::redis::cmd("HSET");
        hset.arg(CONFIG_KEY);
        for (field, value) in FRESH_CONFIG {
            hset.arg(*field).arg(*value);
        }
        hset.query_async::<()>(&mut conn)
            .await
            .map_err(|_| BootstrapError::RedisNotResponding)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_missing_archive_dir() {
        let Ok(base_url) = std::env::var("LANRURUGI_TEST_REDIS_URL") else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let redis = RedisDbs::connect(&base_url).unwrap();
        let err = bootstrap(&redis, Path::new("/nonexistent/path/xyz"))
            .await
            .unwrap_err();
        assert!(matches!(err, BootstrapError::ArchiveDirUnreachable(_)));
    }

    #[tokio::test]
    async fn succeeds_against_a_real_dir_and_reachable_redis() {
        let Ok(base_url) = std::env::var("LANRURUGI_TEST_REDIS_URL") else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let redis = RedisDbs::connect(&base_url).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        bootstrap(&redis, tmp.path()).await.unwrap();
    }

    #[tokio::test]
    async fn seeds_a_fresh_config_but_never_fills_an_existing_one() {
        let Ok(base_url) = std::env::var("LANRURUGI_TEST_REDIS_URL") else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        use deadpool_redis::redis::AsyncCommands;
        let redis = RedisDbs::connect(&base_url).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = redis.config.get().await.unwrap();

        let _: () = conn.del(CONFIG_KEY).await.unwrap();
        bootstrap(&redis, tmp.path()).await.unwrap();
        let fresh: Option<String> = conn.hget(CONFIG_KEY, "guestmode").await.unwrap();
        assert_eq!(fresh.as_deref(), Some("0"));

        // A predating instance: config present, field absent — left for the migration.
        let _: () = conn.del(CONFIG_KEY).await.unwrap();
        let _: () = conn.hset(CONFIG_KEY, "theme", "modern").await.unwrap();
        bootstrap(&redis, tmp.path()).await.unwrap();
        let untouched: Option<String> = conn.hget(CONFIG_KEY, "guestmode").await.unwrap();
        assert_eq!(untouched, None);
        let _: () = conn.del(CONFIG_KEY).await.unwrap();
    }
}
