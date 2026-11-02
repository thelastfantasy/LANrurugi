//! Which e-book format a file is. The extension decides whether a file is a book at all (so a
//! `.zip` never becomes one); within the book extensions the file's own bytes have the last word,
//! because e-books are often renamed between formats (`.azw3` that is really an EPUB, `.pdf`
//! saved as `.djvu`).

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Plain-text files smaller than this are notes and readmes, not books.
pub const TXT_MIN_BYTES: u64 = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookFormat {
    Pdf,
    Epub,
    /// MOBI, AZW, AZW3 (KF8) and PRC — one PalmDB container family.
    Mobi,
    Djvu,
    Fb2,
    Txt,
}

const EXTENSIONS: &[(&str, BookFormat)] = &[
    ("pdf", BookFormat::Pdf),
    ("epub", BookFormat::Epub),
    ("mobi", BookFormat::Mobi),
    ("azw", BookFormat::Mobi),
    ("azw3", BookFormat::Mobi),
    ("prc", BookFormat::Mobi),
    ("djvu", BookFormat::Djvu),
    ("djv", BookFormat::Djvu),
    ("fb2", BookFormat::Fb2),
    ("txt", BookFormat::Txt),
];

impl BookFormat {
    /// Every extension that marks a file as a book.
    pub fn extensions() -> impl Iterator<Item = &'static str> {
        EXTENSIONS.iter().map(|(ext, _)| *ext)
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        EXTENSIONS.iter().find(|(e, _)| *e == ext).map(|(_, f)| *f)
    }

    pub fn from_path_extension(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// The format a file's leading bytes prove, if any. Plain text has no signature.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(b"AT&TFORM") {
            return Some(Self::Djvu);
        }
        if head.get(60..68) == Some(b"BOOKMOBI".as_slice()) {
            return Some(Self::Mobi);
        }
        // EPUB's first zip entry is the uncompressed `mimetype` file (OCF §4.3).
        if head.starts_with(b"PK\x03\x04")
            && head.get(30..38) == Some(b"mimetype".as_slice())
            && head.get(38..58) == Some(b"application/epub+zip".as_slice())
        {
            return Some(Self::Epub);
        }
        // PDF allows junk before the header; readers search the first KB.
        if find(&head[..head.len().min(1024)], b"%PDF-").is_some() {
            return Some(Self::Pdf);
        }
        if find(head, b"<FictionBook").is_some() {
            return Some(Self::Fb2);
        }
        None
    }

    /// The format of a file that has a book extension: its content if recognisable, otherwise
    /// what the extension says. `None` for anything else (archives, images, unknown files).
    pub fn detect(path: &Path) -> Option<Self> {
        let by_extension = Self::from_path_extension(path)?;
        let mut head = [0u8; 4096];
        let n = std::fs::File::open(path)
            .and_then(|mut f| read_up_to(&mut f, &mut head))
            .unwrap_or(0);
        Some(Self::sniff(&head[..n]).unwrap_or(by_extension))
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_case_insensitively() {
        assert_eq!(BookFormat::from_extension("AZW3"), Some(BookFormat::Mobi));
        assert_eq!(BookFormat::from_extension("djv"), Some(BookFormat::Djvu));
        assert_eq!(BookFormat::from_extension("zip"), None);
    }

    #[test]
    fn content_wins_over_extension() {
        let mut epub = b"PK\x03\x04".to_vec();
        epub.resize(30, 0);
        epub.extend_from_slice(b"mimetypeapplication/epub+zip");
        assert_eq!(BookFormat::sniff(&epub), Some(BookFormat::Epub));

        let mut mobi = vec![0u8; 60];
        mobi.extend_from_slice(b"BOOKMOBI");
        assert_eq!(BookFormat::sniff(&mobi), Some(BookFormat::Mobi));

        assert_eq!(BookFormat::sniff(b"AT&TFORM\0\0"), Some(BookFormat::Djvu));
        assert_eq!(BookFormat::sniff(b"\n%PDF-1.7"), Some(BookFormat::Pdf));
        assert_eq!(
            BookFormat::sniff(b"\xEF\xBB\xBF<?xml version=\"1.0\"?><FictionBook xmlns"),
            Some(BookFormat::Fb2)
        );
        assert_eq!(BookFormat::sniff("第一章".as_bytes()), None);
    }

    #[test]
    fn non_book_extension_is_never_a_book() {
        let dir = std::env::temp_dir().join(format!("lrr-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("x.zip");
        std::fs::write(&zip, b"%PDF-1.7").unwrap();
        assert_eq!(BookFormat::detect(&zip), None);
        let renamed = dir.join("x.azw3");
        std::fs::write(&renamed, b"%PDF-1.7").unwrap();
        assert_eq!(BookFormat::detect(&renamed), Some(BookFormat::Pdf));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
