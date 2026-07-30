//! Optional background full-verification job (FR-007's deferred-verification clause): re-checks
//! archives that share a **legacy-style** sample hash (first 512000 bytes, ignoring size — i.e.
//! files that would have collided under the pre-fix algorithm) against a stronger, full-byte
//! comparison, so any lingering ambiguity from a legacy-migrated library is surfaced rather than
//! silently assumed. Reports progress via the `lanrurugi-core::jobs` abstraction (T013), reused
//! here instead of a bespoke tracker.

use std::collections::HashMap;
use std::path::PathBuf;

use lanrurugi_core::concurrency::run_blocking;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_storage::id::legacy_id;
use lanrurugi_storage::repository::ArchiveRepository;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct SuspectGroup {
    pub legacy_sample_hash: String,
    pub archive_ids: Vec<String>,
    /// `true` if every file in the group is fully byte-identical (a genuine duplicate, not a
    /// false-merge candidate); `false` means they only share a leading-512000-byte prefix and are
    /// correctly tracked as distinct archives.
    pub fully_identical: bool,
}

/// Runs the verification pass, reporting progress on `job_id` as it goes. Returns the completed
/// report (also stored as the job's `result` via `JobRegistry::finish`).
pub async fn run(jobs: JobRegistry, job_id: String, archives: ArchiveRepository) {
    jobs.mark_active(&job_id).await;

    let all = match archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            jobs.fail(&job_id, e.to_string()).await;
            return;
        }
    };
    let total = all.len().max(1);

    // Bucket by legacy sample hash (cheap fingerprint) — anything with >1 member is a candidate
    // for the false-merge defect and needs the stronger check.
    let mut buckets: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
    for (i, archive) in all.iter().enumerate() {
        if let Ok(hash) = legacy_id(std::path::Path::new(&archive.file)) {
            buckets
                .entry(hash)
                .or_default()
                .push((archive.id.clone(), PathBuf::from(&archive.file)));
        }
        jobs.set_progress(&job_id, (i + 1) as f32 / total as f32)
            .await;
    }

    let mut groups = Vec::new();
    for (hash, members) in buckets {
        if members.len() < 2 {
            continue;
        }
        let archive_ids: Vec<String> = members.iter().map(|(id, _)| id.clone()).collect();
        let fully_identical = run_blocking(move || files_all_identical(&members))
            .await
            .unwrap_or(false);
        groups.push(SuspectGroup {
            legacy_sample_hash: hash,
            archive_ids,
            fully_identical,
        });
    }

    jobs.finish(&job_id, json!({ "suspect_groups": groups }))
        .await;
}

fn files_all_identical(members: &[(String, PathBuf)]) -> bool {
    let Some((_, first_path)) = members.first() else {
        return true;
    };
    let Ok(first_bytes) = std::fs::read(first_path) else {
        return false;
    };
    members[1..].iter().all(|(_, path)| {
        std::fs::read(path)
            .map(|bytes| bytes == first_bytes)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        deadpool_redis::Config::from_url(format!("{}/0", base.trim_end_matches('/')))
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()
    }

    #[tokio::test]
    async fn flags_shared_prefix_files_as_distinct_not_identical() {
        let Some(pool) = test_pool() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool);
        let dir = tempfile::tempdir().unwrap();

        let mut shared_prefix = vec![b'x'; 512_000];
        let path_a = dir.path().join("a.zip");
        std::fs::write(&path_a, &shared_prefix).unwrap();
        shared_prefix.extend_from_slice(b"different tail content");
        let path_b = dir.path().join("b.zip");
        std::fs::write(&path_b, &shared_prefix).unwrap();

        use lanrurugi_core::entities::Archive;
        let mk = |id: &str, file: &std::path::Path| Archive {
            id: id.to_string(),
            name: "n".into(),
            title: "t".into(),
            file: file.to_string_lossy().to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 1,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
        };
        let id_a = "a".repeat(40);
        let id_b = "b".repeat(40);
        archives.save(&mk(&id_a, &path_a)).await.unwrap();
        archives.save(&mk(&id_b, &path_b)).await.unwrap();

        let jobs = JobRegistry::new();
        let job_id = jobs.create("verify").await;
        run(jobs.clone(), job_id.clone(), archives.clone()).await;

        let status = jobs.get(&job_id).await.unwrap();
        let result = status.result.unwrap();
        let groups = result["suspect_groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["fully_identical"], false);
        assert_eq!(groups[0]["archive_ids"].as_array().unwrap().len(), 2);

        archives.delete(&id_a).await.unwrap();
        archives.delete(&id_b).await.unwrap();
    }
}
