//! Wire format between [`crate::client`] and `lanrurugi-book-worker`: a JSON header frame, then —
//! for a page response only — one raw payload frame. Every frame is a big-endian `u32` length
//! followed by that many bytes. Plain blocking I/O on purpose: the callers (`archive_format`'s
//! page functions) are synchronous and also run on rayon threads that have no async runtime.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::format::BookFormat;

/// Largest frame either side accepts. A rendered page is a few MB; a picture stored in a book from
/// a high-resolution scan can be larger, but nothing legitimate comes near this.
pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Page sources, sizes, outline and metadata.
    Inspect { path: String, format: BookFormat },
    /// One page's image bytes. `source` is what `Inspect` reported for this page, so the page's
    /// entry name (and its extension) and the bytes returned always agree.
    Page {
        path: String,
        format: BookFormat,
        index: u32,
        source: PageSource,
        target_width: u32,
    },
}

/// Encoding of a picture stored inside a book, from its leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    Jpeg,
    Png,
    Gif,
    Webp,
    Bmp,
}

impl ImageKind {
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [0xFF, 0xD8, 0xFF, ..] => Some(Self::Jpeg),
            [0x89, b'P', b'N', b'G', ..] => Some(Self::Png),
            [b'G', b'I', b'F', b'8', ..] => Some(Self::Gif),
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(Self::Webp),
            [b'B', b'M', ..] => Some(Self::Bmp),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
        }
    }
}

/// How a page's image is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSource {
    /// The page is a picture stored in the book (a scanned PDF page, a fixed-layout EPUB page, a
    /// comic MOBI page, an FB2 illustration): its stored bytes are served untouched.
    Image(ImageKind),
    /// Anything else: a vector/text PDF page, a DjVu page, or a typeset page of a text book,
    /// rendered to WebP.
    Render,
}

impl PageSource {
    pub fn extension(self) -> &'static str {
        match self {
            PageSource::Image(kind) => kind.extension(),
            PageSource::Render => "webp",
        }
    }
}

/// One outline (table of contents) entry, flattened in document order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    /// 0-based page index (the archive ToC numbers pages from 1).
    pub page: u32,
    /// 0 for a top-level entry.
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookInfo {
    pub format: BookFormat,
    pub pages: Vec<PageSource>,
    /// Pixel size of each page as served: the stored picture's own size, or the rendered size.
    pub sizes: Vec<(u32, u32)>,
    pub outline: Vec<OutlineItem>,
    pub title: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// DRM or a password protects the book.
    Encrypted,
    /// Not a readable book of its format (truncated, corrupt, unsupported variant).
    Unreadable,
    PageOutOfRange,
    /// Anything else, including a panic caught inside the worker.
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Info {
        info: BookInfo,
    },
    /// Followed by one payload frame with the image bytes.
    Page {
        extension: String,
    },
    Error {
        kind: ErrorKind,
        message: String,
    },
}

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn write_json(w: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_frame(w, &bytes)
}

pub fn read_json<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<T> {
    let bytes = read_frame(r)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The entry name a book page is listed under: its 1-based number, zero-padded to at least four
/// digits so plain and natural sorting agree, plus the extension of the bytes it serves.
pub fn page_entry_name(index: usize, page_count: usize, source: PageSource) -> String {
    let width = page_count.to_string().len().max(4);
    format!("{:0width$}.{}", index + 1, source.extension())
}

/// Inverse of [`page_entry_name`]: the 0-based page index, or `None` for any other name.
pub fn parse_page_entry_name(name: &str) -> Option<usize> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !matches!(ext, "jpg" | "png" | "gif" | "webp" | "bmp") {
        return None;
    }
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse::<usize>().ok()?.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_round_trip_and_sort_naturally() {
        let sources = [
            PageSource::Image(ImageKind::Jpeg),
            PageSource::Render,
            PageSource::Image(ImageKind::Png),
        ];
        let names: Vec<String> = (0..12)
            .map(|i| page_entry_name(i, 12, sources[i % 3]))
            .collect();
        assert_eq!(names[0], "0001.jpg");
        assert_eq!(names[1], "0002.webp");
        assert_eq!(names[2], "0003.png");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, names);
        for (i, name) in names.iter().enumerate() {
            assert_eq!(parse_page_entry_name(name), Some(i));
        }
    }

    #[test]
    fn padding_grows_past_9999_pages() {
        assert_eq!(
            page_entry_name(0, 12_000, PageSource::Image(ImageKind::Jpeg)),
            "00001.jpg"
        );
    }

    #[test]
    fn foreign_names_are_rejected() {
        for name in [
            "cover.jpg",
            "0000.jpg",
            "0001.tiff",
            "12a.jpg",
            ".jpg",
            "0001",
        ] {
            assert_eq!(parse_page_entry_name(name), None, "{name}");
        }
    }

    #[test]
    fn image_kinds_are_sniffed() {
        assert_eq!(
            ImageKind::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(ImageKind::sniff(b"\x89PNG\r\n"), Some(ImageKind::Png));
        assert_eq!(
            ImageKind::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageKind::Webp)
        );
        assert_eq!(ImageKind::sniff(b"FLIS"), None);
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_json(
            &mut buf,
            &Request::Inspect {
                path: "/a.epub".into(),
                format: BookFormat::Epub,
            },
        )
        .unwrap();
        write_frame(&mut buf, b"payload").unwrap();
        let mut r = buf.as_slice();
        let req: Request = read_json(&mut r).unwrap();
        assert!(
            matches!(req, Request::Inspect { path, format: BookFormat::Epub } if path == "/a.epub")
        );
        assert_eq!(read_frame(&mut r).unwrap(), b"payload");
    }
}
