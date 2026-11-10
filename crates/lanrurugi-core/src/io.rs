//! File-copy helpers shared by the download manager and the scanner's relocation pass.
//!
//! Both copy across filesystem boundaries (container temp dir → library pool, library root →
//! `_byname/` on another mount), and both used `tokio::fs::copy`, which silently falls back to an
//! 8 KiB buffer whenever `copy_file_range`/`sendfile` cannot cross that boundary — the only case
//! these two callers are in. Measured on a NAS deployment that fallback moved a 1.4 GB file at
//! 2.4 MB/s (ten minutes); 1 MiB reached 170 MB/s and 4 MiB added nothing measurable. The buffer is
//! per copy and downloads run concurrently, which is why the ceiling stays at 4 MiB rather than
//! scaling with the file without bound.
//!
//! Every size here is a power of two ≥ 4 KiB, so it is already aligned to 512-byte and 4 KiB
//! sectors — the reason not to chase sector alignment further is that this path is buffered, not
//! `O_DIRECT`: the kernel coalesces dirty pages and the filesystem (ZFS's records, ext4's extents)
//! decides the on-disk layout, so a 4 KiB write and a 1 MiB write cost the same number of media
//! writes per byte while the 1 MiB one costs 256× fewer syscalls.

use std::path::Path;

/// Smallest read size for a small file.
const MIN_BUFFER: usize = 64 * 1024;
/// Largest read size; see the module doc for why this is a ceiling rather than a fraction of RAM.
const MAX_BUFFER: usize = 4 * 1024 * 1024;
/// Floor for the shrinking tail chunk (one page), so a long tail is not chopped into thousands of
/// tiny reads.
const MIN_CHUNK: usize = 4096;

const MIB: u64 = 1024 * 1024;

/// Buffer size for copying a `len`-byte file: big enough to keep a NAS device busy, small enough
/// that a handful of concurrent copies do not add up to hundreds of megabytes.
pub const fn copy_buffer_bytes(len: u64) -> usize {
    if len >= 1024 * MIB {
        MAX_BUFFER
    } else if len >= 64 * MIB {
        MIB as usize
    } else if len >= MIB {
        512 * 1024
    } else {
        MIN_BUFFER
    }
}

/// Read size for one iteration, `remaining` bytes from the end with `cap` as the file's own buffer
/// size. Rounded up to the next power of two (never past `cap`), so the last 1025 KiB of a 4 MiB
/// copy is handed to `read` as a 2 MiB slice instead of the full 4 MiB — the kernel returns the
/// 1025 KiB it actually has, and the tail does not look like a full-size transfer.
pub fn chunk_bytes(remaining: u64, cap: usize) -> usize {
    let want = remaining.next_power_of_two().min(cap as u64) as usize;
    want.max(MIN_CHUNK).min(cap)
}

/// Buffered copy that also removes `from` on success. Returns the number of bytes copied.
pub async fn move_file(from: &Path, to: &Path) -> std::io::Result<u64> {
    let copied = copy_file(from, to).await?;
    tokio::fs::remove_file(from).await?;
    Ok(copied)
}

/// Buffered file copy whose read size follows [`copy_buffer_bytes`] / [`chunk_bytes`].
pub async fn copy_file(from: &Path, to: &Path) -> std::io::Result<u64> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut source = tokio::fs::File::open(from).await?;
    let len = source.metadata().await?.len();
    let mut dest = tokio::fs::File::create(to).await?;
    let cap = copy_buffer_bytes(len);
    let mut buffer = vec![0_u8; cap];
    let mut remaining = len;
    let mut copied = 0_u64;
    while remaining > 0 {
        let want = chunk_bytes(remaining, cap);
        let read = source.read(&mut buffer[..want]).await?;
        if read == 0 {
            break;
        }
        dest.write_all(&buffer[..read]).await?;
        remaining = remaining.saturating_sub(read as u64);
        copied += read as u64;
    }
    dest.flush().await?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_grows_with_the_file() {
        assert_eq!(copy_buffer_bytes(100 * 1024), MIN_BUFFER);
        assert_eq!(copy_buffer_bytes(2 * MIB), 512 * 1024);
        assert_eq!(copy_buffer_bytes(100 * MIB), MIB as usize);
        assert_eq!(copy_buffer_bytes(2 * 1024 * MIB), MAX_BUFFER);
    }

    #[test]
    fn the_tail_is_read_in_powers_of_two() {
        let cap = MAX_BUFFER;
        assert_eq!(chunk_bytes(4 * MIB, cap), MAX_BUFFER);
        // 1025 KiB left of a 4 MiB copy is handed to `read` as a 2 MiB slice, not 4 MiB.
        assert_eq!(chunk_bytes(1025 * 1024, cap), 2 * 1024 * 1024);
        assert_eq!(chunk_bytes(300 * 1024, cap), 512 * 1024);
        assert_eq!(chunk_bytes(64 * 1024, cap), 64 * 1024);
        // A tiny remainder still gets a page-sized slice.
        assert_eq!(chunk_bytes(1024, cap), MIN_CHUNK);
    }

    #[tokio::test]
    async fn a_copy_moves_every_byte_and_the_reads_shrink() {
        let dir = std::env::temp_dir().join(format!(
            "lrr-copy-test-{}",
            std::process::id() as u64 * 1_000 + rand_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("from.bin");
        let to = dir.join("to.bin");
        let bytes: Vec<u8> = (0..(MIB + 1234)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&from, &bytes).unwrap();

        let copied = copy_file(&from, &to).await.unwrap();
        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(std::fs::read(&to).unwrap(), bytes);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn rand_id() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    }
}
