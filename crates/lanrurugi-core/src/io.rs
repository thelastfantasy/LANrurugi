//! File-copy helpers shared by the download manager and the scanner's relocation pass, plus the
//! durability primitives (027) that keep a move from losing the only copy: a copy is checked and
//! flushed before anyone may delete its source, and a rename is flushed on both sides.
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

/// What a finished copy proved about itself (027).
#[derive(Debug, Clone)]
pub struct CopyOutcome {
    pub bytes: u64,
    /// BLAKE3 of the bytes actually read from the source.
    pub blake3: blake3::Hash,
    /// Strict verification was asked for but could not read past the cache here; the reason.
    pub readback_skipped: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CopyOptions {
    /// Read each written block back, bypassing the cache, and compare (027 FR-014).
    pub strict_verify: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("copy failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("copy came out {copied} bytes (destination {dest_len}), expected {expected}")]
    LengthMismatch {
        expected: u64,
        copied: u64,
        dest_len: u64,
    },
    #[error("could not flush the copy to disk: {0}")]
    Durability(#[source] std::io::Error),
    #[error("read-back differs at byte {offset} ({len} bytes)")]
    ReadbackMismatch { offset: u64, len: u64 },
}

/// Copies `from` to `to`, checks the length (027 FR-001) and flushes `to` to disk (FR-002).
/// Never removes `from`; on any failure `to` is removed. The caller still owes `to`'s directory a
/// [`sync_dir`] once the file has its final name.
pub async fn copy_file(
    from: &Path,
    to: &Path,
    opts: &CopyOptions,
) -> Result<CopyOutcome, CopyError> {
    let result = copy_file_with(from, to, opts, readback::open).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(to).await;
    }
    result
}

async fn copy_file_with(
    from: &Path,
    to: &Path,
    opts: &CopyOptions,
    open_readback: readback::Opener,
) -> Result<CopyOutcome, CopyError> {
    let source = tokio::fs::File::open(from).await.map_err(CopyError::Io)?;
    let len = source.metadata().await.map_err(CopyError::Io)?.len();
    let outcome = copy_from_reader(source, len, to, opts, open_readback).await?;
    // A source that grew or was replaced during the copy is not the file the copy describes.
    let now = tokio::fs::metadata(from)
        .await
        .map_err(CopyError::Io)?
        .len();
    if now != len {
        return Err(CopyError::LengthMismatch {
            expected: len,
            copied: outcome.bytes,
            dest_len: outcome.bytes,
        });
    }
    Ok(outcome)
}

async fn copy_from_reader<R: tokio::io::AsyncRead + Unpin>(
    mut source: R,
    len: u64,
    to: &Path,
    opts: &CopyOptions,
    open_readback: readback::Opener,
) -> Result<CopyOutcome, CopyError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut dest = tokio::fs::File::create(to).await.map_err(CopyError::Io)?;
    let mut verifier = None;
    let mut readback_skipped = None;
    if opts.strict_verify {
        match open_readback(to) {
            Ok(reader) => verifier = Some(readback::Verifier::spawn(reader)),
            Err(reason) => {
                readback::log_skipped(reason);
                readback_skipped = Some(reason);
            }
        }
    }

    let cap = copy_buffer_bytes(len);
    let mut buffer = vec![0_u8; cap];
    let mut hasher = blake3::Hasher::new();
    let mut remaining = len;
    let mut copied = 0_u64;
    while remaining > 0 {
        let want = chunk_bytes(remaining, cap);
        let read = source
            .read(&mut buffer[..want])
            .await
            .map_err(CopyError::Io)?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        hasher.update(chunk);
        dest.write_all(chunk).await.map_err(CopyError::Io)?;
        if let Some(verifier) = &verifier {
            // `tokio::fs::File` finishes a write in the background; the block must reach the OS
            // before anyone can read it back.
            dest.flush().await.map_err(CopyError::Io)?;
            // A closed channel means the verifier already found a mismatch; `finish` reports it.
            if !verifier.check(copied, chunk).await {
                break;
            }
        }
        remaining = remaining.saturating_sub(read as u64);
        copied += read as u64;
    }
    dest.flush().await.map_err(CopyError::Io)?;
    sync_file(&dest).await.map_err(CopyError::Durability)?;
    if let Some(verifier) = verifier {
        match verifier.finish().await {
            readback::Verdict::Match => {}
            readback::Verdict::Mismatch { offset, len } => {
                return Err(CopyError::ReadbackMismatch { offset, len })
            }
            readback::Verdict::Skipped(reason) => {
                readback::log_skipped(reason);
                readback_skipped = Some(reason);
            }
            readback::Verdict::Failed(e) => return Err(CopyError::Io(e)),
        }
    }
    let dest_len = dest.metadata().await.map_err(CopyError::Io)?.len();
    if copied != len || dest_len != len {
        return Err(CopyError::LengthMismatch {
            expected: len,
            copied,
            dest_len,
        });
    }
    Ok(CopyOutcome {
        bytes: copied,
        blake3: hasher.finalize(),
        readback_skipped,
    })
}

/// Flushes a file's data and metadata to stable storage. On macOS plain `fsync` only reaches the
/// drive's cache, so `F_FULLFSYNC` is tried first.
pub async fn sync_file(file: &tokio::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let std_file = file.try_clone().await?.into_std().await;
        return tokio::task::spawn_blocking(move || {
            full_fsync_or(
                || {
                    // SAFETY: a plain fcntl on a descriptor this closure owns.
                    let r = unsafe { libc::fcntl(std_file.as_raw_fd(), libc::F_FULLFSYNC) };
                    if r == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                },
                || std_file.sync_all(),
            )
        })
        .await
        .map_err(std::io::Error::other)?;
    }
    #[cfg(not(target_os = "macos"))]
    file.sync_all().await
}

/// `F_FULLFSYNC` is refused by some filesystems (network mounts, FAT); fall back to `fsync` then.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn full_fsync_or(
    full: impl FnOnce() -> std::io::Result<()>,
    fallback: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    full().or_else(|_| fallback())
}

/// `fsync` on a directory, so a rename or a new entry in it survives a crash.
pub async fn sync_dir(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::File::open(&dir)?.sync_all())
        .await
        .map_err(std::io::Error::other)?
}

/// Same-filesystem move that survives a crash in either order: `from`'s bytes are flushed before
/// the rename, and the target directory after it (027 FR-003). Fails with the rename's own error
/// (see [`is_cross_device`]) when the two paths are on different filesystems.
pub async fn durable_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let file = tokio::fs::File::open(from).await?;
    sync_file(&file).await?;
    drop(file);
    tokio::fs::rename(from, to).await?;
    if let Some(parent) = to.parent() {
        sync_dir(parent).await?;
    }
    Ok(())
}

/// A rename refused because the paths are on different filesystems (or bind mounts).
pub fn is_cross_device(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::EXDEV)
}

/// Whole-file BLAKE3 for bytes the system did not see arrive (watcher finds, backfill). Blocking.
pub fn blake3_file(path: &Path) -> std::io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize())
}

/// Cache-bypassing read-back for strict verification (027 US4).
mod readback {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Reads `len` bytes at `offset` from the medium, not from the page cache.
    pub trait ReadBack: Send {
        fn read_at(&mut self, offset: u64, len: usize) -> std::io::Result<Vec<u8>>;
    }

    pub type Opener = fn(&Path) -> Result<Box<dyn ReadBack>, &'static str>;

    /// Blocks queued for checking; the copy waits beyond this (FR-015).
    const QUEUE: usize = 3;

    pub enum Verdict {
        Match,
        Mismatch { offset: u64, len: u64 },
        Skipped(&'static str),
        Failed(std::io::Error),
    }

    pub struct Verifier {
        tx: tokio::sync::mpsc::Sender<(u64, usize, blake3::Hash)>,
        task: tokio::task::JoinHandle<Verdict>,
    }

    impl Verifier {
        pub fn spawn(mut reader: Box<dyn ReadBack>) -> Self {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, usize, blake3::Hash)>(QUEUE);
            let task = tokio::task::spawn_blocking(move || {
                while let Some((offset, len, expected)) = rx.blocking_recv() {
                    match reader.read_at(offset, len) {
                        Ok(bytes) if bytes.len() == len && blake3::hash(&bytes) == expected => {}
                        Ok(_) => {
                            return Verdict::Mismatch {
                                offset,
                                len: len as u64,
                            }
                        }
                        // An alignment the device refuses is "cannot verify", not a bad copy.
                        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
                            return Verdict::Skipped("the device refused an uncached read")
                        }
                        Err(e) => return Verdict::Failed(e),
                    }
                }
                Verdict::Match
            });
            Self { tx, task }
        }

        /// Queues one written block; `false` once the verifier has stopped.
        pub async fn check(&self, offset: u64, chunk: &[u8]) -> bool {
            self.tx
                .send((offset, chunk.len(), blake3::hash(chunk)))
                .await
                .is_ok()
        }

        pub async fn finish(self) -> Verdict {
            drop(self.tx);
            self.task
                .await
                .unwrap_or_else(|e| Verdict::Failed(std::io::Error::other(e)))
        }
    }

    pub fn log_skipped(reason: &'static str) {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            tracing::info!(
                reason,
                "strict verification skipped: the read-back would only see the cache"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn open(path: &Path) -> Result<Box<dyn ReadBack>, &'static str> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            if is_zfs(dir) {
                return Err("ZFS cannot bypass the ARC");
            }
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(path)
            .map_err(|_| "O_DIRECT is not supported here")?;
        Ok(Box::new(DirectReader {
            file,
            buf: Vec::new(),
        }))
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn open(_path: &Path) -> Result<Box<dyn ReadBack>, &'static str> {
        // macOS `F_NOCACHE` stops new caching but still serves the pages this copy just wrote.
        Err("this platform cannot read past the page cache")
    }

    #[cfg(target_os = "linux")]
    // `f_type` is `i64` on x86_64 glibc but narrower on other targets/libcs.
    #[allow(clippy::unnecessary_cast)]
    fn is_zfs(dir: &Path) -> bool {
        const ZFS_SUPER_MAGIC: i64 = 0x2fc1_2fc1;
        statfs(dir).is_some_and(|s| s.f_type as i64 == ZFS_SUPER_MAGIC)
    }

    #[cfg(target_os = "freebsd")]
    fn is_zfs(dir: &Path) -> bool {
        statfs(dir).is_some_and(|s| fs_type_name(&s.f_fstypename) == "zfs")
    }

    #[cfg(target_os = "freebsd")]
    fn fs_type_name(raw: &[libc::c_char]) -> String {
        raw.iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect()
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn statfs(dir: &Path) -> Option<libc::statfs> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut out = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `c` is NUL-terminated and `out` is written by the call before it is read.
        let r = unsafe { libc::statfs(c.as_ptr(), out.as_mut_ptr()) };
        (r == 0).then(|| unsafe { out.assume_init() })
    }

    /// `O_DIRECT` wants offset, length and buffer aligned; reads are widened to 4 KiB pages.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    struct DirectReader {
        file: std::fs::File,
        buf: Vec<u8>,
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    impl ReadBack for DirectReader {
        fn read_at(&mut self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
            use std::os::unix::fs::FileExt;
            const ALIGN: u64 = 4096;
            let start = offset / ALIGN * ALIGN;
            let end = (offset + len as u64).div_ceil(ALIGN) * ALIGN;
            let window = (end - start) as usize;
            if self.buf.len() < window + ALIGN as usize {
                self.buf = vec![0; window + ALIGN as usize];
            }
            let pad =
                (ALIGN as usize - self.buf.as_ptr() as usize % ALIGN as usize) % ALIGN as usize;
            let aligned = &mut self.buf[pad..pad + window];
            let mut got = 0;
            while got < window {
                let n = self.file.read_at(&mut aligned[got..], start + got as u64)?;
                if n == 0 {
                    break;
                }
                got += n;
            }
            let skip = (offset - start) as usize;
            let stop = (skip + len).min(got);
            Ok(aligned[skip.min(stop)..stop].to_vec())
        }
    }
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

        let copied = copy_file(&from, &to, &CopyOptions::default())
            .await
            .unwrap();
        assert_eq!(copied.bytes, bytes.len() as u64);
        assert_eq!(copied.blake3, blake3::hash(&bytes));
        assert!(copied.readback_skipped.is_none());
        assert_eq!(std::fs::read(&to).unwrap(), bytes);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lrr-io-{tag}-{}-{}", std::process::id(), rand_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_short_source_is_a_length_mismatch() {
        let dir = scratch("short");
        let to = dir.join("to.bin");
        let err = copy_from_reader(
            &b"only ten b"[..],
            4096,
            &to,
            &CopyOptions::default(),
            readback::open,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                CopyError::LengthMismatch {
                    expected: 4096,
                    copied: 10,
                    ..
                }
            ),
            "{err:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_failed_copy_keeps_the_source_and_leaves_no_target() {
        let dir = scratch("fail");
        let from = dir.join("from.bin");
        std::fs::write(&from, b"precious").unwrap();
        let to = dir.join("missing-dir").join("to.bin");
        let err = copy_file(&from, &to, &CopyOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, CopyError::Io(_)), "{err:?}");
        assert_eq!(std::fs::read(&from).unwrap(), b"precious");
        assert!(!to.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    static READBACKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Reads through the cache and optionally flips a byte in the second block.
    struct FakeReadBack {
        file: std::fs::File,
        corrupt: bool,
    }

    impl readback::ReadBack for FakeReadBack {
        fn read_at(&mut self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
            use std::os::unix::fs::FileExt;
            READBACKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut out = vec![0; len];
            self.file.read_exact_at(&mut out, offset)?;
            if self.corrupt && offset > 0 {
                out[0] ^= 0xff;
            }
            Ok(out)
        }
    }

    fn fake_clean(path: &Path) -> Result<Box<dyn readback::ReadBack>, &'static str> {
        Ok(Box::new(FakeReadBack {
            file: std::fs::File::open(path).unwrap(),
            corrupt: false,
        }))
    }

    fn fake_corrupt(path: &Path) -> Result<Box<dyn readback::ReadBack>, &'static str> {
        Ok(Box::new(FakeReadBack {
            file: std::fs::File::open(path).unwrap(),
            corrupt: true,
        }))
    }

    #[tokio::test]
    async fn strict_verification_reads_back_and_catches_a_bad_block() {
        let dir = scratch("strict");
        let from = dir.join("from.bin");
        // Three 512 KiB reads at this size, so there is a second block to corrupt.
        let bytes: Vec<u8> = (0..(1536 * 1024)).map(|i| (i % 253) as u8).collect();
        std::fs::write(&from, &bytes).unwrap();
        let strict = CopyOptions {
            strict_verify: true,
        };

        let off = dir.join("off.bin");
        let before = READBACKS.load(std::sync::atomic::Ordering::SeqCst);
        copy_file_with(&from, &off, &CopyOptions::default(), fake_clean)
            .await
            .unwrap();
        assert_eq!(
            READBACKS.load(std::sync::atomic::Ordering::SeqCst),
            before,
            "off by default"
        );

        let good = dir.join("good.bin");
        copy_file_with(&from, &good, &strict, fake_clean)
            .await
            .unwrap();
        assert!(READBACKS.load(std::sync::atomic::Ordering::SeqCst) > before);

        let bad = dir.join("bad.bin");
        let err = copy_file_with(&from, &bad, &strict, fake_corrupt)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CopyError::ReadbackMismatch { offset, .. } if offset > 0),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(&from).unwrap(),
            bytes,
            "the source is untouched"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn real_strict_verification_passes_or_says_why_it_could_not() {
        let dir = scratch("direct");
        let from = dir.join("from.bin");
        let bytes: Vec<u8> = (0..(MIB + 777)).map(|i| (i % 241) as u8).collect();
        std::fs::write(&from, &bytes).unwrap();
        let to = dir.join("to.bin");
        let outcome = copy_file(
            &from,
            &to,
            &CopyOptions {
                strict_verify: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.blake3, blake3::hash(&bytes));
        assert_eq!(std::fs::read(&to).unwrap(), bytes);
        if let Some(reason) = outcome.readback_skipped {
            assert!(!reason.is_empty());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn full_fsync_falls_back_to_fsync() {
        let refused = || Err(std::io::Error::from_raw_os_error(libc::ENOTSUP));
        assert!(full_fsync_or(refused, || Ok(())).is_ok());
        let both = full_fsync_or(refused, || Err(std::io::Error::other("disk gone")));
        assert!(both.is_err());
    }

    #[tokio::test]
    async fn a_durable_rename_moves_the_file() {
        let dir = scratch("rename");
        let from = dir.join("a");
        let to = dir.join("b");
        std::fs::write(&from, b"x").unwrap();
        durable_rename(&from, &to).await.unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"x");
        assert!(is_cross_device(&std::io::Error::from_raw_os_error(
            libc::EXDEV
        )));
        assert!(!is_cross_device(&std::io::Error::from_raw_os_error(
            libc::ENOENT
        )));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn rand_id() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    }
}
