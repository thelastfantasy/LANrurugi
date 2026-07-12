//! Archive-format handling (research.md §3). Extraction goes through **libarchive** via the
//! `libarchive2-sys` FFI bindings — exactly what legacy LANraragi uses (`Archive::Libarchive` with
//! `support_filter_all` / `support_format_all`, i.e. format auto-detect, no extension branching).
//! One backend reads ZIP/CBZ/EPUB, RAR/CBR (incl. RAR5), 7z/CB7, LZH/LHA, tar… etc., so there's no
//! per-format shell-out (the old `unrar`/`7z` process spawn) and no extension→tool dispatch table.
//!
//! This also fixes a real bug the old shell-out had: the runtime image ships `unrar-free`, whose
//! getopt-style CLI doesn't accept the RARLAB `unrar lb` / `unrar p -inul` syntax the old code
//! used — so RAR/CBR listing and single-entry extraction were silently broken in production.
//! libarchive reads RAR5 itself (free, DFSG), so `unrar-free` is no longer needed at all.
//!
//! `libarchive2-sys` bundles the full libarchive C source and builds it as a static library via
//! CMake (build dep: `cmake` + `libclang-dev` for bindgen — in `Dockerfile.build`, not the runtime
//! image, which links libarchive statically). Page listing follows legacy `get_filelist`'s
//! image-extension filter (verified: `Utils/Generic.pm::is_image`), with natural sort but without
//! legacy's cover/credit-page reordering nicety (a deliberate Phase 1 simplification — pagecount
//! and ordering-by-name are what FR-002/FR-005 actually depend on).
//!
//! Entry names go through [`decode_archive_name`], which is why [`RawArchiveReader`] talks to
//! `libarchive2_sys` directly instead of using the (otherwise more convenient) `libarchive2` safe
//! wrapper crate: that crate's `Entry::pathname()` tries `archive_entry_pathname_utf8()` and, on
//! failure, falls back to lossy-UTF-8-decoding the raw bytes — turning any non-UTF-8-flagged
//! legacy CJK zip entry (Shift-JIS/EUC-JP/Big5/GBK/EUC-KR — common from older Japanese/Chinese/
//! Korean-locale zip tools that never set the ZIP general-purpose UTF-8 flag) into permanent
//! mojibake: once lossy-decoded to U+FFFD replacement characters, the original bytes are gone, so
//! no later step can recover the real name. `libarchive2::Entry`'s inner pointer is `pub(crate)`,
//! so there's no way to reach the raw (pre-lossy-conversion) bytes through that crate at all —
//! [`RawArchiveReader`] reads `archive_entry_pathname()`'s bytes straight from the FFI layer
//! before any lossy conversion happens, so `decode_archive_name` still has real bytes to work
//! with. (Tried first: asking libarchive itself to convert via `hdrcharset=<charset>` — but that
//! goes through the system's iconv/locale data, which a minimal container image typically doesn't
//! ship for CJK charsets; verified this actually fails at runtime with "Pathname cannot be
//! converted from CP932 to current locale" in this project's own `lanrurugi-dev` image, which
//! only has `C`/`C.utf8`/`POSIX` locales installed. `chardetng`+`encoding_rs` decode purely in
//! Rust, so they don't depend on the deployment environment having any particular locale data.)
//!
//! Separately, [`Utf8LocaleGuard`] reproduces `libarchive2::locale::UTF8LocaleGuard` (Unix-only —
//! this project only ships a Linux Docker image): libarchive validates UTF-8-flagged entry names
//! against the process's *current* `LC_CTYPE` inside `archive_read_next_header` itself, before
//! this module's own code ever sees the entry, and a bare `C`/`POSIX` locale (the default for a
//! process that hasn't called `setlocale`, which is exactly the Rust runtime's starting state)
//! makes that validation reject perfectly valid UTF-8 names with "Pathname cannot be converted
//! from UTF-8 to current locale" — an entirely separate failure mode from the CJK mojibake this
//! module otherwise fixes, caught by this module's own test suite once real non-ASCII fixtures
//! were added.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveFormatError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("libarchive error: {0}")]
    Libarchive(String),
    #[error("unsupported archive extension: {0:?}")]
    UnsupportedExtension(String),
    #[error("entry {0:?} not found in archive")]
    EntryNotFound(String),
}

type Result<T> = std::result::Result<T, ArchiveFormatError>;

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "gif", "bmp", "jpeg", "jfif", "webp", "avif", "heif", "heic", "jxl",
];

/// Extensions libarchive is asked to open. Kept as an allowlist (rather than handing libarchive
/// every file) so a non-archive upload comes back as a clean `UnsupportedExtension` instead of a
/// opaque libarchive parse error.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "cbz", "epub", "rar", "cbr", "7z", "cb7", "lzh", "lha", "tar", "gz", "bz2", "xz",
];

fn is_image_name(name: &str) -> bool {
    name.rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_supported_archive(path: &Path) -> bool {
    ARCHIVE_EXTENSIONS.contains(&extension_of(path).as_str())
}

/// Decodes a raw archive-entry filename byte string, trying UTF-8 first (the overwhelming common
/// case — modern zip tools set the UTF-8 flag, and every other format libarchive reads stores
/// names as UTF-8 natively) before falling back to CJK legacy-encoding detection.
///
/// `chardetng` (Mozilla/Firefox's charset detector) covers exactly the encoding families a
/// mismatched CJK-locale zip tool could have used — Shift-JIS/EUC-JP (Japanese), Big5 (Traditional
/// Chinese), GBK/GB18030 (Simplified Chinese), EUC-KR (Korean) — so no manual byte-range
/// pre-filtering is needed to cover the "other encoding" cases the user asked about. Confidence on
/// short strings (a filename, not a whole document) is inherently lower than on long text, but
/// `decode()`'s own `had_errors` flag still catches a wrong guess: an encoding that produces
/// invalid sequences from these bytes reports `had_errors=true`, so this only trusts the guess
/// when the decode actually succeeded cleanly. If even that fails, lossy-UTF-8 is the last resort
/// (matches the previous, pre-this-fix behavior exactly, so nothing regresses versus before).
fn decode_archive_name(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(raw, true);
    let guessed = detector.guess(None, Utf8Detection::Deny);
    let (decoded, _, had_errors) = guessed.decode(raw);
    if !had_errors {
        return decoded.into_owned();
    }
    String::from_utf8_lossy(raw).into_owned()
}

/// Minimal read-only libarchive wrapper that, unlike `libarchive2::ReadArchive`/`Entry`, exposes
/// each entry's *raw* pathname bytes rather than an already-UTF-8-converted (and, on failure,
/// already-lossy-mangled) `String` — needed so [`decode_archive_name`] gets a real shot at the
/// original bytes. `libarchive2::Entry`'s inner pointer is `pub(crate)`, so there's no way to get
/// this via the safe wrapper; this duplicates just the handful of calls `list_pages`/`read_entry`/
/// `find_entry_by_suffix` actually need, directly against `libarchive2_sys`.
struct RawArchiveReader {
    handle: *mut libarchive2_sys::archive,
}

struct RawEntry {
    is_regular_file: bool,
    name_bytes: Vec<u8>,
}

impl RawArchiveReader {
    fn open(path: &Path) -> Result<Self> {
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| ArchiveFormatError::Libarchive("path contains a NUL byte".to_string()))?;
        unsafe {
            let handle = libarchive2_sys::archive_read_new();
            if handle.is_null() {
                return Err(ArchiveFormatError::Libarchive(
                    "archive_read_new returned null".to_string(),
                ));
            }
            libarchive2_sys::archive_read_support_filter_all(handle);
            libarchive2_sys::archive_read_support_format_all(handle);
            let ret = libarchive2_sys::archive_read_open_filename(handle, c_path.as_ptr(), 10240);
            if ret < libarchive2_sys::ARCHIVE_OK as i32 {
                let msg = Self::error_string(handle);
                libarchive2_sys::archive_read_free(handle);
                return Err(ArchiveFormatError::Libarchive(msg));
            }
            Ok(Self { handle })
        }
    }

    unsafe fn error_string(handle: *mut libarchive2_sys::archive) -> String {
        unsafe {
            let ptr = libarchive2_sys::archive_error_string(handle);
            if ptr.is_null() {
                "unknown libarchive error".to_string()
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    /// Advances to the next entry, returning its file-type flag and raw (possibly non-UTF-8)
    /// pathname bytes. `None` at end of archive.
    fn next_entry(&mut self) -> Result<Option<RawEntry>> {
        let _locale_guard = Utf8LocaleGuard::new();
        unsafe {
            let mut entry: *mut libarchive2_sys::archive_entry = ptr::null_mut();
            let ret = libarchive2_sys::archive_read_next_header(self.handle, &mut entry);
            if ret == libarchive2_sys::ARCHIVE_EOF as i32 {
                return Ok(None);
            }
            if ret < libarchive2_sys::ARCHIVE_OK as i32 {
                return Err(ArchiveFormatError::Libarchive(Self::error_string(
                    self.handle,
                )));
            }
            let mode = libarchive2_sys::archive_entry_filetype(entry);
            const S_IFMT: u32 = 0o170000;
            const S_IFREG: u32 = 0o100000;
            let is_regular_file = (mode as u32 & S_IFMT) == S_IFREG;

            let name_ptr: *const c_char = libarchive2_sys::archive_entry_pathname(entry);
            let name_bytes = if name_ptr.is_null() {
                Vec::new()
            } else {
                CStr::from_ptr(name_ptr).to_bytes().to_vec()
            };
            Ok(Some(RawEntry {
                is_regular_file,
                name_bytes,
            }))
        }
    }

    fn read_data_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        unsafe {
            loop {
                let n = libarchive2_sys::archive_read_data(
                    self.handle,
                    buf.as_mut_ptr() as *mut std::os::raw::c_void,
                    buf.len(),
                );
                if n < 0 {
                    return Err(ArchiveFormatError::Libarchive(Self::error_string(
                        self.handle,
                    )));
                }
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n as usize]);
            }
        }
        Ok(data)
    }

    fn skip_data(&mut self) -> Result<()> {
        unsafe {
            let ret = libarchive2_sys::archive_read_data_skip(self.handle);
            if ret < libarchive2_sys::ARCHIVE_OK as i32 {
                return Err(ArchiveFormatError::Libarchive(Self::error_string(
                    self.handle,
                )));
            }
        }
        Ok(())
    }
}

impl Drop for RawArchiveReader {
    fn drop(&mut self) {
        unsafe {
            libarchive2_sys::archive_read_free(self.handle);
        }
    }
}

/// Temporarily swaps the calling thread's `LC_CTYPE` to a UTF-8 locale for the duration of a
/// libarchive call. Without this, `archive_read_next_header` itself refuses UTF-8-flagged
/// non-ASCII entry names under the bare `C`/`POSIX` locale libarchive otherwise starts in
/// ("Pathname cannot be converted from UTF-8 to current locale") — this is libarchive's own
/// internal validation, independent of and prior to anything [`decode_archive_name`] does.
///
/// Tries `"C.utf8"` by name explicitly, rather than `libarchive2::locale::UTF8LocaleGuard`'s own
/// approach of asking `newlocale` for `""` (meaning "whatever `LANG`/`LC_CTYPE` says") — a
/// container that never sets those environment variables at all (verified: this project's own
/// `lanrurugi-dev` image, and very plausibly the deployed runtime image too, since nothing in
/// this repo's `Dockerfile` sets `LANG`) still resolves `""` successfully, just to `POSIX`
/// (`newlocale` doesn't fail just because the requested locale isn't UTF-8), which doesn't fix
/// anything. Naming `"C.utf8"` directly resolves correctly regardless of the environment. Falls
/// back to leaving the locale untouched (rather than erroring) if even that name is unavailable,
/// so archives with only ASCII/already-correct names still work everywhere; the difference only
/// matters for the CJK/non-ASCII cases this module exists to fix in the first place.
struct Utf8LocaleGuard {
    saved: libc::locale_t,
    new_locale: libc::locale_t,
}

impl Utf8LocaleGuard {
    fn new() -> Self {
        unsafe {
            let new_locale = libc::newlocale(
                libc::LC_CTYPE_MASK,
                c"C.utf8".as_ptr(),
                std::ptr::null_mut(),
            );
            let saved = if !new_locale.is_null() {
                libc::uselocale(new_locale)
            } else {
                std::ptr::null_mut()
            };
            Self { saved, new_locale }
        }
    }
}

impl Drop for Utf8LocaleGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.new_locale.is_null() {
                libc::uselocale(self.saved);
                libc::freelocale(self.new_locale);
            }
        }
    }
}

/// Natural sort key: splits into alternating non-digit/digit runs so `page2` sorts before
/// `page10` (legacy's `expand` helper does the equivalent zero-padding trick).
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut chars = s.chars().peekable();
    while chars.peek().is_some() {
        let is_digit_run = chars.peek().unwrap().is_ascii_digit();
        let mut run = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() == is_digit_run {
                run.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if is_digit_run {
            key.push((String::new(), run.parse().unwrap_or(0)));
        } else {
            key.push((run, 0));
        }
    }
    key
}

/// Lists image entry names inside the archive, in natural-sorted (page) order. Works for every
/// format libarchive understands (zip/cbz/epub, rar/cbr incl. RAR5, 7z/cb7, lzh/lha, tar…).
pub fn list_pages(path: &Path) -> Result<Vec<String>> {
    if !is_supported_archive(path) {
        return Err(ArchiveFormatError::UnsupportedExtension(extension_of(path)));
    }
    let mut archive = RawArchiveReader::open(path)?;
    let mut names = Vec::new();
    while let Some(entry) = archive.next_entry()? {
        // Only regular files with image extensions are pages; skip directories/macros.
        if entry.is_regular_file {
            let name = decode_archive_name(&entry.name_bytes);
            if is_image_name(&name) {
                names.push(name);
            }
        }
        archive.skip_data()?;
    }
    names.sort_by_key(|n| natural_key(n));
    Ok(names)
}

/// Reads the raw bytes of a single named entry (used for thumbnail extraction). Iterates the
/// archive until the entry is found, then reads its data into memory.
pub fn read_entry(path: &Path, entry_name: &str) -> Result<Vec<u8>> {
    if !is_supported_archive(path) {
        return Err(ArchiveFormatError::UnsupportedExtension(extension_of(path)));
    }
    let mut archive = RawArchiveReader::open(path)?;
    while let Some(entry) = archive.next_entry()? {
        let matches = decode_archive_name(&entry.name_bytes) == entry_name;
        if matches {
            return archive.read_data_to_vec();
        }
        archive.skip_data()?;
    }
    Err(ArchiveFormatError::EntryNotFound(entry_name.to_string()))
}

/// Finds the first entry whose basename ends with `wanted_suffix` — matches legacy
/// `is_file_in_archive`'s own matching rule exactly (`Utils/Archive.pm`: `"$name$suffix" =~
/// /$wantedname$/`, i.e. a basename-suffix match, not a full-name equality check — so e.g.
/// `wanted_suffix = "info.json"` also matches a real basename of `"some-info.json"`). Unlike
/// [`list_pages`], this considers *every* regular file in the archive, not just image-named ones
/// — sidecar metadata files (`api.json`, `ComicInfo.xml`, ...) live alongside the pages, not
/// among them.
pub fn find_entry_by_suffix(path: &Path, wanted_suffix: &str) -> Result<Option<String>> {
    if !is_supported_archive(path) {
        return Err(ArchiveFormatError::UnsupportedExtension(extension_of(path)));
    }
    let mut archive = RawArchiveReader::open(path)?;
    while let Some(entry) = archive.next_entry()? {
        if entry.is_regular_file {
            let name = decode_archive_name(&entry.name_bytes);
            let basename = name.rsplit('/').next().unwrap_or(&name).to_string();
            if basename.ends_with(wanted_suffix) {
                return Ok(Some(name));
            }
        }
        archive.skip_data()?;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // A real 7z archive (built with `7z a`, not this crate's own test-only `zip::ZipWriter` path)
    // containing two UTF-8-named CJK entries — `zip`'s own writer can only exercise the ZIP
    // format's own encoding quirks, so this is here specifically to confirm `list_pages`/
    // `read_entry` work identically end-to-end on a *different* container format libarchive reads
    // (7z has no legacy Shift-JIS-without-a-flag ambiguity the way old zip tools do; this fixture
    // exists to catch a libarchive-level regression in 7z entry-name handling generally, not to
    // re-test `decode_archive_name`'s CJK-encoding-guessing itself, which the zip-based tests
    // above already cover thoroughly).
    const CJK_NAMES_7Z: &[u8] = include_bytes!("../tests-fixtures/cjk-names.7z");

    fn write_fixture(bytes: &[u8], suffix: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(suffix).unwrap();
        file.reopen().unwrap().write_all(bytes).unwrap();
        file
    }

    #[test]
    fn list_pages_and_read_entry_recover_cjk_names_from_a_real_7z() {
        let f = write_fixture(CJK_NAMES_7Z, ".7z");
        let mut pages = list_pages(f.path()).unwrap();
        pages.sort();
        let mut expected = vec![
            "測試 檔案.jpg".to_string(),
            "空白 テスト「1」.jpg".to_string(),
        ];
        expected.sort();
        assert_eq!(pages, expected);

        assert_eq!(
            read_entry(f.path(), "測試 檔案.jpg").unwrap(),
            b"fake jpeg content 2"
        );
    }

    fn make_test_zip_with_ext(ext: &str, entries: &[&str]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(format!(".{ext}")).unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for name in entries {
            writer.start_file(*name, options).unwrap();
            writer
                .write_all(format!("content of {name}").as_bytes())
                .unwrap();
        }
        writer.finish().unwrap();
        file
    }

    /// Hand-builds a single-entry zip with the given raw *bytes* as the filename and the ZIP
    /// general-purpose UTF-8 flag (bit 11 of the flags field) left unset — the `zip` crate's own
    /// `ZipWriter::start_file` always writes UTF-8 names with that flag set, so it can't produce
    /// the "legacy CJK-locale zip tool" shape these tests need to exercise. This mirrors exactly
    /// what an old Shift-JIS/GBK/EUC-*-locale zip tool actually put on disk: raw legacy-encoded
    /// bytes, no UTF-8 flag, nothing to signal which charset they're in.
    fn make_test_zip_with_raw_name_bytes(
        name_bytes: &[u8],
        content: &[u8],
    ) -> tempfile::NamedTempFile {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc: u32 = 0xFFFF_FFFF;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }

        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let crc = crc32(content);
        let name_len = name_bytes.len() as u16;
        let content_len = content.len() as u32;

        let mut local = Vec::new();
        local.extend_from_slice(&0x04034b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0u16.to_le_bytes()); // flags: no UTF-8 bit (0x0800) set
        local.extend_from_slice(&0u16.to_le_bytes()); // compression: stored
        local.extend_from_slice(&0u16.to_le_bytes()); // mod time
        local.extend_from_slice(&0u16.to_le_bytes()); // mod date
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&content_len.to_le_bytes());
        local.extend_from_slice(&content_len.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        local.extend_from_slice(name_bytes);
        local.extend_from_slice(content);

        let local_header_offset = 0u32;
        let mut central = Vec::new();
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // compression
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&0u16.to_le_bytes()); // mod date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&content_len.to_le_bytes());
        central.extend_from_slice(&content_len.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&local_header_offset.to_le_bytes());
        central.extend_from_slice(name_bytes);

        let mut eocd = Vec::new();
        eocd.extend_from_slice(&0x06054b50u32.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // disk number
        eocd.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
        eocd.extend_from_slice(&1u16.to_le_bytes()); // entries on this disk
        eocd.extend_from_slice(&1u16.to_le_bytes()); // total entries
        eocd.extend_from_slice(&(central.len() as u32).to_le_bytes());
        eocd.extend_from_slice(&(local.len() as u32).to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // comment length

        let mut f = file.reopen().unwrap();
        f.write_all(&local).unwrap();
        f.write_all(&central).unwrap();
        f.write_all(&eocd).unwrap();
        file
    }

    // --- pure helpers ---

    #[test]
    fn is_image_name_matches_known_image_extensions_case_insensitively() {
        for ok in [
            "page.jpg", "Page.JPG", "a.png", "b.webp", "c.avif", "d.jxl", "e.jpeg", "f.gif",
            "g.bmp", "h.heic",
        ] {
            assert!(is_image_name(ok), "{ok} should be recognized as an image");
        }
        for no in ["readme.txt", "page", "noext", "index.html", "page.jpg.bak"] {
            assert!(
                !is_image_name(no),
                "{no} should NOT be recognized as an image"
            );
        }
    }

    #[test]
    fn extension_of_is_lowercased_and_empty_when_absent() {
        assert_eq!(extension_of(Path::new("foo.ZIP")), "zip");
        assert_eq!(extension_of(Path::new("a/b/c.CbZ")), "cbz");
        assert_eq!(extension_of(Path::new("archive.tar.lzh")), "lzh");
        assert_eq!(extension_of(Path::new("noext")), "");
    }

    #[test]
    fn natural_key_orders_numerically_not_lexicographically() {
        let mut v = vec!["page10.jpg", "page2.jpg", "page1.jpg", "page20.jpg"];
        v.sort_by_key(|n| natural_key(n));
        assert_eq!(
            v,
            vec!["page1.jpg", "page2.jpg", "page10.jpg", "page20.jpg"]
        );
        assert!(natural_key("page2.jpg") < natural_key("page10.jpg"));
    }

    // --- libarchive read path (zip/cbz/epub exercise the same list_pages/read_entry code every
    //     format goes through; libarchive itself owns rar5/7z/lzh decode correctness) ---

    #[test]
    fn list_pages_returns_only_images_in_natural_order() {
        let f = make_test_zip_with_ext(
            "zip",
            &["page10.jpg", "page2.jpg", "readme.txt", "page1.jpg"],
        );
        assert_eq!(
            list_pages(f.path()).unwrap(),
            vec!["page1.jpg", "page2.jpg", "page10.jpg"]
        );
    }

    #[test]
    fn read_entry_returns_file_bytes() {
        let f = make_test_zip_with_ext("zip", &["page1.jpg", "page2.jpg"]);
        assert_eq!(
            read_entry(f.path(), "page2.jpg").unwrap(),
            b"content of page2.jpg"
        );
    }

    #[test]
    fn list_pages_works_across_zip_family_extensions() {
        for ext in ["zip", "cbz", "epub"] {
            let f = make_test_zip_with_ext(ext, &["page1.jpg", "page2.jpg"]);
            assert_eq!(
                list_pages(f.path()).unwrap(),
                vec!["page1.jpg", "page2.jpg"],
                "extension .{ext} should be readable via libarchive"
            );
        }
    }

    #[test]
    fn read_entry_reports_missing_entry() {
        let f = make_test_zip_with_ext("zip", &["page1.jpg"]);
        let err = read_entry(f.path(), "does-not-exist.jpg").unwrap_err();
        assert!(matches!(err, ArchiveFormatError::EntryNotFound(_)));
    }

    #[test]
    fn find_entry_by_suffix_matches_basename_suffix_not_full_equality() {
        let f = make_test_zip_with_ext("zip", &["page1.jpg", "meta/some-api.json"]);
        assert_eq!(
            find_entry_by_suffix(f.path(), "api.json").unwrap(),
            Some("meta/some-api.json".to_string())
        );
    }

    #[test]
    fn find_entry_by_suffix_returns_none_when_absent() {
        let f = make_test_zip_with_ext("zip", &["page1.jpg"]);
        assert_eq!(find_entry_by_suffix(f.path(), "api.json").unwrap(), None);
    }

    // --- special characters in entry names: spaces, Japanese punctuation, CJK (all UTF-8-flagged,
    //     the normal/modern case — legacy-encoding mojibake is covered separately below) ---

    #[test]
    fn list_pages_handles_spaces_in_entry_names() {
        let f = make_test_zip_with_ext("zip", &["page 01.jpg", "page 02.jpg", "read me.txt"]);
        assert_eq!(
            list_pages(f.path()).unwrap(),
            vec!["page 01.jpg", "page 02.jpg"]
        );
    }

    #[test]
    fn list_pages_handles_japanese_punctuation_and_cjk_entry_names() {
        let f = make_test_zip_with_ext(
            "zip",
            &["第一話「始まり」.jpg", "第二話【終わり】.jpg", "説明。txt"],
        );
        let mut pages = list_pages(f.path()).unwrap();
        pages.sort();
        let mut expected = vec!["第一話「始まり」.jpg", "第二話【終わり】.jpg"];
        expected.sort();
        assert_eq!(pages, expected);
    }

    #[test]
    fn read_entry_round_trips_a_name_with_spaces_and_japanese_punctuation() {
        let f = make_test_zip_with_ext("zip", &["空白 テスト「1」.jpg"]);
        assert_eq!(
            read_entry(f.path(), "空白 テスト「1」.jpg").unwrap(),
            b"content of \xe7\xa9\xba\xe7\x99\xbd \xe3\x83\x86\xe3\x82\xb9\xe3\x83\x88\xe3\x80\x8c1\xe3\x80\x8d.jpg"
        );
    }

    #[test]
    fn find_entry_by_suffix_matches_cjk_basename() {
        let f = make_test_zip_with_ext("zip", &["メタ/情報-api.json"]);
        assert_eq!(
            find_entry_by_suffix(f.path(), "api.json").unwrap(),
            Some("メタ/情報-api.json".to_string())
        );
    }

    // --- non-UTF-8-flagged legacy CJK encodings inside the zip (the mojibake bug fix) ---

    #[test]
    fn decode_archive_name_passes_through_plain_ascii() {
        assert_eq!(decode_archive_name(b"page001.jpg"), "page001.jpg");
    }

    #[test]
    fn decode_archive_name_passes_through_utf8_bytes_unchanged() {
        let bytes = "第一話.jpg".as_bytes();
        assert_eq!(decode_archive_name(bytes), "第一話.jpg");
    }

    #[test]
    fn decode_archive_name_detects_shift_jis() {
        let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode("空白 テスト.jpg");
        assert!(!had_errors);
        assert_eq!(decode_archive_name(&bytes), "空白 テスト.jpg");
    }

    #[test]
    fn decode_archive_name_detects_euc_jp() {
        let (bytes, _, had_errors) = encoding_rs::EUC_JP.encode("第一話「始まり」.jpg");
        assert!(!had_errors);
        assert_eq!(decode_archive_name(&bytes), "第一話「始まり」.jpg");
    }

    #[test]
    fn decode_archive_name_detects_gbk_simplified_chinese() {
        let (bytes, _, had_errors) = encoding_rs::GBK.encode("测试 文件.jpg");
        assert!(!had_errors);
        assert_eq!(decode_archive_name(&bytes), "测试 文件.jpg");
    }

    #[test]
    fn decode_archive_name_detects_big5_traditional_chinese() {
        let (bytes, _, had_errors) = encoding_rs::BIG5.encode("測試 檔案.jpg");
        assert!(!had_errors);
        assert_eq!(decode_archive_name(&bytes), "測試 檔案.jpg");
    }

    #[test]
    fn decode_archive_name_detects_euc_kr_korean() {
        let (bytes, _, had_errors) = encoding_rs::EUC_KR.encode("테스트 파일.jpg");
        assert!(!had_errors);
        assert_eq!(decode_archive_name(&bytes), "테스트 파일.jpg");
    }

    #[test]
    fn list_pages_recovers_shift_jis_filename_without_utf8_flag() {
        let (name_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode("空白 テスト.jpg");
        assert!(!had_errors);
        let f = make_test_zip_with_raw_name_bytes(&name_bytes, b"fake jpeg content");
        assert_eq!(list_pages(f.path()).unwrap(), vec!["空白 テスト.jpg"]);
    }

    #[test]
    fn read_entry_recovers_gbk_filename_without_utf8_flag() {
        let (name_bytes, _, had_errors) = encoding_rs::GBK.encode("测试图片.jpg");
        assert!(!had_errors);
        let f = make_test_zip_with_raw_name_bytes(&name_bytes, b"fake jpeg content");
        assert_eq!(
            read_entry(f.path(), "测试图片.jpg").unwrap(),
            b"fake jpeg content"
        );
    }

    // --- unsupported extension ---

    #[test]
    fn list_pages_rejects_unknown_extension() {
        let f = tempfile::NamedTempFile::with_suffix(".docx").unwrap();
        let err = list_pages(f.path()).unwrap_err();
        assert!(matches!(err, ArchiveFormatError::UnsupportedExtension(_)));
    }

    #[test]
    fn list_pages_accepts_known_but_unparseable_rar_as_libarchive_error() {
        // A `.rar` with bogus bytes is a *known* extension, so it must reach libarchive (not be
        // rejected as UnsupportedExtension) — libarchive then fails to parse it.
        let f = tempfile::NamedTempFile::with_suffix(".rar").unwrap();
        std::fs::write(f.path(), b"this is not a rar").unwrap();
        let err = list_pages(f.path()).unwrap_err();
        assert!(
            matches!(err, ArchiveFormatError::Libarchive(_)),
            "expected Libarchive error for bogus .rar, got {err:?}"
        );
    }
}
