//! 020 T021: the rebuild report's sidecar audit.
//!
//! Sidecars are a derived snapshot, so a disagreement never fails a rebuild — it is reported so an
//! operator can see that some records would not be recoverable from disk alone. Only hash-layout
//! archives have one; `_byname/` archives are addressed by their filename and never do.

use std::path::{Path, PathBuf};

use lanrurugi_storage::paths;
use lanrurugi_storage::repository::{ArchiveRepository, RepositoryError};

/// What the sidecar next to one hash-layout archive looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarState {
    /// Present, readable, and written for this id.
    Ok,
    /// No sidecar file at all.
    Missing,
    /// A sidecar file exists but cannot be parsed as one of ours.
    Unreadable,
    /// A sidecar exists, parses, but names a different archive.
    Mismatched(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarIssue {
    pub id: String,
    pub file: String,
    pub reason: &'static str,
    /// The id written inside the sidecar, when there is one to show.
    pub found: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SidecarAudit {
    pub checked: u64,
    pub missing: u64,
    pub unreadable: u64,
    pub mismatched: u64,
    /// How many of the problems above were rewritten from the record during this pass.
    pub repaired: u64,
    pub details: Vec<SidecarIssue>,
}

impl SidecarAudit {
    pub fn is_clean(&self) -> bool {
        self.missing == 0 && self.unreadable == 0 && self.mismatched == 0
    }
}

/// Classifies the sidecar for `id`. `Ok` also covers "the file is there but this archive predates
/// sidecars" only when no file exists — a missing file is `Missing`, not `Ok`.
pub fn classify(library: &Path, id: &str) -> SidecarState {
    if !paths::sidecar_path(library, id).is_file() {
        return SidecarState::Missing;
    }
    match lanrurugi_storage::sidecar::read_raw(library, id) {
        None => SidecarState::Unreadable,
        Some(sidecar) if sidecar.id != id => SidecarState::Mismatched(sidecar.id),
        Some(_) => SidecarState::Ok,
    }
}

/// Audits every hash-layout archive's sidecar. `_byname/` archives are skipped: by design they carry
/// their identity in the filename.
pub async fn audit_sidecars(
    archives: &ArchiveRepository,
    library: &Path,
) -> Result<SidecarAudit, RepositoryError> {
    let all = archives.list_all().await?;
    let mut audit = SidecarAudit::default();
    for archive in all {
        let path = PathBuf::from(&archive.file);
        if !paths::is_hash_layout_path(library, &path, archive.id.as_str()) {
            continue;
        }
        audit.checked += 1;
        let state = classify(library, archive.id.as_str());
        if state != SidecarState::Ok {
            // FR-011: a sidecar is derived, so the record can always regenerate it. Leaving the
            // disagreement in place until an operator acts is what the spec forbids.
            let extension = Path::new(&archive.file)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let original_filename = if extension.is_empty() {
                archive.name.clone()
            } else {
                format!("{}.{extension}", archive.name)
            };
            match lanrurugi_storage::sidecar::Sidecar::for_archive(
                archive.id.as_str(),
                &path,
                &original_filename,
                &archive.title,
            ) {
                Ok(sidecar) if lanrurugi_storage::sidecar::write(library, &sidecar).is_ok() => {
                    audit.repaired += 1
                }
                Ok(_) => tracing::warn!(id = %archive.id, "could not rewrite the sidecar"),
                Err(e) => {
                    tracing::warn!(id = %archive.id, error = %e, "could not rebuild the sidecar")
                }
            }
        }
        match state {
            SidecarState::Ok => {}
            SidecarState::Missing => audit.missing += 1,
            SidecarState::Unreadable => {
                audit.unreadable += 1;
                audit.details.push(SidecarIssue {
                    id: archive.id.to_string(),
                    file: archive.file.clone(),
                    reason: "unreadable",
                    found: None,
                });
            }
            SidecarState::Mismatched(found) => {
                audit.mismatched += 1;
                audit.details.push(SidecarIssue {
                    id: archive.id.to_string(),
                    file: archive.file.clone(),
                    reason: "id_mismatch",
                    found: Some(found),
                });
            }
        }
    }
    Ok(audit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::sidecar::Sidecar;

    fn library_with_sidecar(id: &str, written_id: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(&id[..2])).unwrap();
        let archive = paths::hash_layout_path(tmp.path(), id, "zip");
        std::fs::write(&archive, b"bytes").unwrap();
        let mut sidecar = Sidecar::for_archive(id, &archive, "Book.zip", "Book").unwrap();
        // Deliberately wrong id inside the file, while the file itself sits at `id`'s own path.
        sidecar.id = written_id.to_string();
        let dest = paths::sidecar_path(tmp.path(), id);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, serde_json::to_vec_pretty(&sidecar).unwrap()).unwrap();
        tmp
    }

    #[test]
    fn a_matching_sidecar_is_ok() {
        let id = "ab".to_string() + &"c".repeat(38);
        let tmp = library_with_sidecar(&id, &id);
        assert_eq!(classify(tmp.path(), &id), SidecarState::Ok);
    }

    #[test]
    fn a_sidecar_for_another_id_is_reported_with_the_id_it_holds() {
        let id = "ab".to_string() + &"c".repeat(38);
        let other = "de".to_string() + &"f".repeat(38);
        let tmp = library_with_sidecar(&id, &other);
        assert_eq!(classify(tmp.path(), &id), SidecarState::Mismatched(other));
    }

    #[test]
    fn a_missing_or_unparsable_sidecar_is_told_apart() {
        let id = "ab".to_string() + &"c".repeat(38);
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(classify(tmp.path(), &id), SidecarState::Missing);
        let sidecar = paths::sidecar_path(tmp.path(), &id);
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, b"{not json").unwrap();
        assert_eq!(classify(tmp.path(), &id), SidecarState::Unreadable);
    }
}
