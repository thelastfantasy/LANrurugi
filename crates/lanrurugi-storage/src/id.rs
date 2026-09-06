//! Archive-identity hashing. Constitution Principle I requires the legacy algorithm to remain
//! read-compatible forever, and the new default to be additive, not a silent in-place swap.
//!
//! **Verified against source** (`~/LANraragi/lib/LANraragi/Utils/Database.pm::compute_id`): the
//! legacy sample size is exactly **512000 bytes**, not `512 * 1024` (524288) despite the "512 KBs"
//! comment in the Perl source — `read $handle, $data, 512000`. Both algorithms below use the same
//! literal 512000-byte sample for this reason; using 524288 would silently produce different IDs
//! for every file smaller than ~512.5KB... well, for every file between the two sizes, which is
//! exactly the kind of discrepancy Principle I exists to prevent.

use std::io::Read;
use std::path::Path;

use sha1::{Digest, Sha1};
use thiserror::Error;

/// Legacy `compute_id`'s sample size, in bytes. Fixed by the verified Perl source — not
/// `512 * 1024`.
pub const SAMPLE_SIZE: usize = 512_000;

/// Length of a legitimate archive ID: a lowercase-hex SHA-1 digest, always exactly this many
/// characters (`legacy_id`/`size_aware_id` below both produce one). Shared so every ID-format
/// validity check across crates (`lanrurugi-api::archives::is_valid_archive_id`,
/// `lanrurugi-api::plugins::extract_archive_id`) agrees on the same value.
pub const ARCHIVE_ID_LEN: usize = 40;

#[derive(Debug, Error)]
pub enum IdError {
    #[error("I/O error reading file for archive ID computation: {0}")]
    Io(#[from] std::io::Error),
    #[error("computed ID is for a null/empty value, invalid source file")]
    NullDigest,
}

fn read_sample(path: &Path) -> Result<(Vec<u8>, u64), IdError> {
    let mut file = std::fs::File::open(path)?;
    let file_size = file.metadata()?.len();

    let mut buf = vec![0u8; SAMPLE_SIZE];
    let mut total_read = 0usize;
    loop {
        match file.read(&mut buf[total_read..])? {
            0 => break,
            n => {
                total_read += n;
                if total_read == buf.len() {
                    break;
                }
            }
        }
    }
    buf.truncate(total_read);
    Ok((buf, file_size))
}

/// The legacy archive ID: `SHA-1(first 512000 bytes of the file)`. Must remain computable
/// indefinitely so data migrated from LANraragi stays readable (Principle I) — never remove this.
pub fn legacy_id(path: &Path) -> Result<String, IdError> {
    let (sample, _size) = read_sample(path)?;
    if sample.is_empty() {
        return Err(IdError::NullDigest);
    }
    Ok(digest_hex(&sample))
}

/// The new, size-aware default archive ID for freshly-scanned content:
/// `SHA-1(first 512000 bytes ++ u64 big-endian file size)`. Fixes the false-merge defect where two
/// distinct files sharing a leading byte range hashed identically under `legacy_id` — appending the
/// size means files with different total lengths can no longer collide just because their first
/// 512000 bytes match (research.md §1, constitution Technology Stack Constraints).
pub fn size_aware_id(path: &Path) -> Result<String, IdError> {
    let (mut sample, size) = read_sample(path)?;
    if sample.is_empty() {
        return Err(IdError::NullDigest);
    }
    sample.extend_from_slice(&size.to_be_bytes());
    Ok(digest_hex(&sample))
}

fn digest_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Minimal hex encoding so we don't need a whole extra crate dependency for one helper.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(bytes.as_ref().len() * 2);
        for b in bytes.as_ref() {
            write!(s, "{b:02x}").expect("writing to a String cannot fail");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn legacy_id_matches_known_sha1_of_sample() {
        let data = vec![b'a'; 1000];
        let f = write_temp(&data);
        let id = legacy_id(f.path()).unwrap();

        let mut hasher = Sha1::new();
        hasher.update(&data);
        let expected = hex::encode(hasher.finalize());
        assert_eq!(id, expected);
        assert_eq!(id.len(), 40);
    }

    #[test]
    fn legacy_id_only_samples_first_512000_bytes() {
        let mut data = vec![b'x'; SAMPLE_SIZE];
        data.extend_from_slice(b"trailing content past the sample window");
        let f_short = write_temp(&vec![b'x'; SAMPLE_SIZE]);
        let f_long = write_temp(&data);

        assert_eq!(
            legacy_id(f_short.path()).unwrap(),
            legacy_id(f_long.path()).unwrap()
        );
    }

    #[test]
    fn size_aware_id_distinguishes_shared_prefix_different_length() {
        // This is the exact false-merge defect FR-005 fixes: two files sharing the same leading
        // 512000 bytes but differing afterward must no longer collide.
        let mut shorter = vec![b'y'; SAMPLE_SIZE];
        let mut longer = shorter.clone();
        longer.extend_from_slice(b"extra distinguishing tail content");
        shorter.truncate(SAMPLE_SIZE);

        let f_short = write_temp(&shorter);
        let f_long = write_temp(&longer);

        assert_eq!(
            legacy_id(f_short.path()).unwrap(),
            legacy_id(f_long.path()).unwrap(),
            "legacy_id is expected to still collide here (that's the bug being fixed)"
        );
        assert_ne!(
            size_aware_id(f_short.path()).unwrap(),
            size_aware_id(f_long.path()).unwrap(),
            "size_aware_id must NOT collide when file sizes differ"
        );
    }

    #[test]
    fn size_aware_id_is_stable_on_byte_identical_rescan() {
        let data = vec![b'z'; 12345];
        let f1 = write_temp(&data);
        let f2 = write_temp(&data);
        assert_eq!(
            size_aware_id(f1.path()).unwrap(),
            size_aware_id(f2.path()).unwrap(),
            "byte-identical files must still map to the same archive (Clarifications Q2)"
        );
    }

    #[test]
    fn empty_file_is_rejected() {
        let f = write_temp(&[]);
        assert!(matches!(legacy_id(f.path()), Err(IdError::NullDigest)));
        assert!(matches!(size_aware_id(f.path()), Err(IdError::NullDigest)));
    }
}
