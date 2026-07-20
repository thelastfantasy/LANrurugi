//! Bundles multiple already-downloaded, already-staged files into a single zip archive (spec
//! FR-018 / a plugin's `bundle_as_archive: true`, e.g. Pixiv's per-page images shipping as one
//! manga archive instead of N separate one-page archives).

use std::path::Path;

use thiserror::Error;

use super::stream::DownloadedFile;

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("failed to build the bundled archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("I/O error while bundling: {0}")]
    Io(#[from] std::io::Error),
    #[error("bundling task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Zips every file in `resources` (in order) into one new archive under `staging_dir`, each
/// entry named after that resource's own resolved filename (deduplicated with a numeric suffix
/// on collision, same convention as `ingest::unique_dest_path`), then deletes the original
/// per-resource staged files — only the one bundled archive remains on disk afterward. Runs the
/// actual zip I/O on a blocking thread (`tokio::task::spawn_blocking`, research.md §1) since
/// `zip::ZipWriter` is a synchronous API.
pub async fn bundle_into_one_archive(
    staging_dir: &Path,
    resources: Vec<DownloadedFile>,
    bundle_filename: &str,
) -> Result<DownloadedFile, BundleError> {
    let bundle_path = staging_dir.join(format!("download-{}.zip", uuid::Uuid::new_v4().simple()));
    let bundle_filename = bundle_filename.to_string();
    let bundle_path_for_blocking = bundle_path.clone();

    let bytes_downloaded: u64 = resources.iter().map(|r| r.bytes_downloaded).sum();

    tokio::task::spawn_blocking(move || -> Result<(), BundleError> {
        let file = std::fs::File::create(&bundle_path_for_blocking)?;
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();

        let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for resource in &resources {
            let entry_name = unique_entry_name(&resource.filename, &mut used_names);
            writer.start_file(entry_name, options)?;
            let mut source = std::fs::File::open(&resource.path)?;
            std::io::copy(&mut source, &mut writer)?;
        }
        writer.finish()?;

        for resource in &resources {
            let _ = std::fs::remove_file(&resource.path);
        }
        Ok(())
    })
    .await??;

    Ok(DownloadedFile {
        path: bundle_path,
        filename: bundle_filename,
        bytes_downloaded,
    })
}

fn unique_entry_name(filename: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(filename.to_string()) {
        return filename.to_string();
    }
    let path = Path::new(filename);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let candidate = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    format!("{stem}-{}", uuid::Uuid::new_v4().simple())
}

/// Suggested filename for the bundled archive itself — derived from the plugin's own namespace
/// since no single resource's own filename is representative of the whole bundle.
pub fn bundle_archive_filename(plugin_namespace: &str) -> String {
    let leaf = plugin_namespace
        .rsplit('/')
        .next()
        .unwrap_or(plugin_namespace);
    format!("{leaf}-{}.zip", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundles_multiple_files_into_one_zip_and_removes_the_originals() {
        use std::path::PathBuf;
        let dir = std::env::temp_dir().join(format!("lrr-bundle-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let mut resources = Vec::new();
        for i in 0..3 {
            let path = dir.join(format!("staged-{i}"));
            tokio::fs::write(&path, format!("content-{i}"))
                .await
                .unwrap();
            resources.push(DownloadedFile {
                path,
                filename: format!("page{i:03}.jpg"),
                bytes_downloaded: format!("content-{i}").len() as u64,
            });
        }
        let originals: Vec<PathBuf> = resources.iter().map(|r| r.path.clone()).collect();

        let bundled = bundle_into_one_archive(&dir, resources, "artwork.zip")
            .await
            .unwrap();

        assert_eq!(bundled.filename, "artwork.zip");
        assert_eq!(bundled.bytes_downloaded, 3 * "content-0".len() as u64);
        assert!(tokio::fs::metadata(&bundled.path).await.is_ok());

        for original in originals {
            assert!(
                tokio::fs::metadata(&original).await.is_err(),
                "original staged file must be removed after bundling"
            );
        }

        // Real zip: readable back with every page present.
        let file = std::fs::File::open(&bundled.path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.len(), 3);
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["page000.jpg", "page001.jpg", "page002.jpg"]);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[test]
    fn unique_entry_name_deduplicates_on_collision() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(unique_entry_name("page.jpg", &mut used), "page.jpg");
        assert_eq!(unique_entry_name("page.jpg", &mut used), "page (1).jpg");
        assert_eq!(unique_entry_name("page.jpg", &mut used), "page (2).jpg");
    }
}
