//! Library-open/bootstrap routine (User Story 1): verifies Redis is reachable and the archive
//! folder exists, with no destructive step — pointing LANrurugi at an existing legacy library
//! must never itself alter that library (constitution Principle I).

use std::path::Path;

use thiserror::Error;

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
}
