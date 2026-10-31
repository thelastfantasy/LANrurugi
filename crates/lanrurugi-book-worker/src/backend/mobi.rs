//! MOBI / AZW / AZW3 (KF8) / PRC: a PalmDB file of records. The file is memory-mapped; records
//! are byte slices of the mapping, so a stored picture is copied out exactly once, when served.
//!
//! Comic files (fixed-layout, or almost no text per picture) are image books whose page order is
//! the order the text references its pictures; everything else is typeset from the book's HTML.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use memmap2::Mmap;
use regex::Regex;

use super::{Container, Failure, Pages, Result};
use crate::flow::{Block, FlowBook, ImageRef, Lang};
use crate::markup::{self, Tag};
use lanrurugi_book::ImageKind;

const NO_INDEX: u32 = 0xFFFF_FFFF;
const COMPRESSION_NONE: u16 = 1;
const COMPRESSION_PALMDOC: u16 = 2;
/// Fewer visible characters than this per picture means a comic, not a text book with figures.
const COMIC_CHARS_PER_IMAGE: usize = 32;

pub struct Mobi {
    map: Mmap,
    records: Vec<Range<usize>>,
    /// Absolute record index of each picture referenced, indexed by [`ImageRef`].
    images: Vec<usize>,
    title: Option<String>,
    author: Option<String>,
}

/// The fields of a MOBI header (record 0 of a section) this reader uses.
struct Header {
    compression: u16,
    text_records: usize,
    text_length: usize,
    encryption: u16,
    encoding: u32,
    version: u32,
    first_image: u32,
    extra_flags: u16,
    /// KF8: record (within the section) of the FDST table that splits the text into flows; only
    /// flow 0 is the book's markup, the rest are stylesheets and SVG.
    fdst: Option<usize>,
    full_name: Option<String>,
    exth: HashMap<u32, Vec<u8>>,
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl Header {
    fn parse(record0: &[u8]) -> Option<Self> {
        let mut header = Header {
            compression: be16(record0, 0)?,
            text_length: be32(record0, 4)? as usize,
            text_records: be16(record0, 8)? as usize,
            encryption: be16(record0, 12)?,
            encoding: 1252,
            version: 0,
            first_image: NO_INDEX,
            extra_flags: 0,
            fdst: None,
            full_name: None,
            exth: HashMap::new(),
        };
        if record0.get(16..20) != Some(b"MOBI".as_slice()) {
            return Some(header); // plain PalmDOC
        }
        let mobi_len = be32(record0, 20)? as usize;
        header.encoding = be32(record0, 28).unwrap_or(1252);
        header.version = be32(record0, 36).unwrap_or(0);
        header.first_image = be32(record0, 108).unwrap_or(NO_INDEX);
        if mobi_len >= 0xE4 {
            header.extra_flags = be16(record0, 16 + 0xE2).unwrap_or(0);
        }
        if header.version >= 8 {
            let (index, count) = (be32(record0, 0xC0), be32(record0, 0xC4));
            if let (Some(index), Some(count)) = (index, count) {
                header.fdst = (index != NO_INDEX && count > 1).then_some(index as usize);
            }
        }
        let (name_at, name_len) = (be32(record0, 84)? as usize, be32(record0, 88)? as usize);
        header.full_name = record0
            .get(name_at..name_at + name_len)
            .map(|b| String::from_utf8_lossy(b).trim().to_owned())
            .filter(|s| !s.is_empty());
        let has_exth = be32(record0, 128).unwrap_or(0) & 0x40 != 0;
        let exth_at = 16 + mobi_len;
        if has_exth && record0.get(exth_at..exth_at + 4) == Some(b"EXTH".as_slice()) {
            let count = be32(record0, exth_at + 8).unwrap_or(0);
            let mut at = exth_at + 12;
            for _ in 0..count {
                let (Some(kind), Some(len)) = (be32(record0, at), be32(record0, at + 4)) else {
                    break;
                };
                let len = len as usize;
                if len < 8 {
                    break;
                }
                if let Some(data) = record0.get(at + 8..at + len) {
                    header.exth.entry(kind).or_insert_with(|| data.to_vec());
                }
                at += len;
            }
        }
        Some(header)
    }

    fn exth_string(&self, kind: u32) -> Option<String> {
        self.exth
            .get(&kind)
            .map(|b| String::from_utf8_lossy(b).trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    fn exth_u32(&self, kind: u32) -> Option<u32> {
        self.exth.get(&kind).and_then(|b| be32(b, 0))
    }
}

impl Mobi {
    pub fn open(path: &str) -> Result<(Self, Pages)> {
        let file = std::fs::File::open(path)?;
        // SAFETY: read-only mapping of a library file; see fb2.rs for the modification caveat.
        let map = unsafe { Mmap::map(&file)? };
        let records =
            palmdb_records(&map).ok_or_else(|| Failure::unreadable("not a PalmDB file"))?;
        let mut mobi = Mobi {
            map,
            records,
            images: Vec::new(),
            title: None,
            author: None,
        };

        let header = Header::parse(
            mobi.record(0)
                .ok_or_else(|| Failure::unreadable("no record 0"))?,
        )
        .ok_or_else(|| Failure::unreadable("bad MOBI header"))?;
        if header.encryption != 0 {
            return Err(Failure::encrypted(format!(
                "Kindle DRM (encryption type {})",
                header.encryption
            )));
        }
        mobi.title = header.exth_string(503).or_else(|| header.full_name.clone());
        mobi.author = header.exth_string(100);

        // A KF8 file (version 8) or the KF8 half of a combination file supplies `kindle:embed`
        // picture numbering; the MOBI 7 half (if any) supplies the most complete text.
        let kf8_section = match header.exth_u32(121) {
            Some(boundary) if header.version < 8 => Some(boundary as usize + 1),
            _ if header.version >= 8 => Some(0),
            _ => None,
        };
        let kf8_first_image = kf8_section
            .and_then(|s| {
                mobi.record(s)
                    .and_then(Header::parse)
                    .map(|h| (s, h.first_image))
            })
            .map(|(section, first)| {
                if first == NO_INDEX {
                    NO_INDEX
                } else {
                    first
                        + if header.version < 8 {
                            section as u32
                        } else {
                            0
                        }
                }
            });
        let first_image = header.first_image;

        let image_records: Vec<usize> = if first_image == NO_INDEX {
            Vec::new()
        } else {
            (first_image as usize..mobi.records.len())
                .filter(|&i| mobi.record(i).and_then(ImageKind::sniff).is_some())
                .collect()
        };
        let thumbnail = header
            .exth_u32(202)
            .map(|n| first_image as usize + n as usize);

        let text = mobi.text(&header, 0);
        let mut flow = FlowBook {
            lang: header.exth_string(524).as_deref().and_then(Lang::from_tag),
            ..Default::default()
        };
        if let Ok(html) = &text {
            let toc = filepos_toc(html);
            let targets: Vec<markup::Target> = toc
                .iter()
                .map(|(_, offset)| markup::Target::Offset(*offset))
                .collect();
            let images = &mut mobi.images;
            let found = markup::html_to_blocks(
                html,
                &mut flow,
                &mut |tag: &Tag| {
                    let record = if let Some(n) = tag
                        .attr("recindex")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                    {
                        (first_image != NO_INDEX && n > 0).then(|| first_image as usize + n - 1)
                    } else {
                        let src = tag.attr("src")?;
                        let n = kindle_embed_index(&src)?;
                        let base = kf8_first_image
                            .filter(|&f| f != NO_INDEX)
                            .unwrap_or(first_image);
                        (base != NO_INDEX && n > 0).then(|| base as usize + n - 1)
                    }?;
                    Some(intern(images, record))
                },
                &targets,
            );
            for ((title, _), block) in toc.into_iter().zip(found) {
                if let Some(block) = block {
                    flow.anchors.push(crate::flow::Anchor {
                        title,
                        block,
                        depth: 0,
                    });
                }
            }
        }

        let referenced: Vec<ImageRef> = flow
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Image(image) => Some(*image),
                _ => None,
            })
            .collect();
        let visible_chars: usize = flow
            .blocks
            .iter()
            .map(|b| match b {
                Block::Para { text } | Block::Heading { text, .. } => {
                    text.chars().filter(|c| !c.is_whitespace()).count()
                }
                _ => 0,
            })
            .sum();
        let fixed_layout = header
            .exth_string(122)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let picture_count = referenced.len().max(image_records.len());
        let comic = picture_count > 0
            && (fixed_layout
                || text.is_err()
                || visible_chars < COMIC_CHARS_PER_IMAGE * picture_count);

        let pages = if comic {
            let mut pages: Vec<ImageRef> = Vec::new();
            if referenced.is_empty() {
                // Text unreadable (or references nothing): every picture record in order.
                for record in image_records.into_iter().filter(|r| Some(*r) != thumbnail) {
                    pages.push(intern(&mut mobi.images, record));
                }
            } else {
                for image in referenced {
                    if pages.last() != Some(&image) {
                        pages.push(image);
                    }
                }
            }
            Pages::Images {
                pages,
                outline: Vec::new(),
            }
        } else {
            match text {
                Ok(_) => {
                    if flow.anchors.is_empty() {
                        flow.anchors_from_headings();
                    }
                    flow.complete_sparse_outline();
                    Pages::Flow(flow)
                }
                Err(e) => return Err(e),
            }
        };
        Ok((mobi, pages))
    }

    fn record(&self, index: usize) -> Option<&[u8]> {
        self.records.get(index).map(|r| &self.map[r.clone()])
    }

    /// The decompressed, decoded HTML of the text records following section record `section`.
    fn text(&self, header: &Header, section: usize) -> Result<String> {
        let mut out = Vec::with_capacity(header.text_length);
        for i in 1..=header.text_records {
            let Some(record) = self.record(section + i) else {
                break;
            };
            let record = &record
                [..record.len() - trailing_size(record, header.extra_flags).min(record.len())];
            match header.compression {
                COMPRESSION_NONE => out.extend_from_slice(record),
                COMPRESSION_PALMDOC => palmdoc_decompress(record, &mut out),
                other => {
                    return Err(Failure::unreadable(format!(
                        "unsupported text compression {other}"
                    )))
                }
            }
        }
        out.truncate(header.text_length.max(1).min(out.len()));
        if let Some(end) = header
            .fdst
            .and_then(|i| self.record(section + i))
            .and_then(first_flow_end)
        {
            out.truncate(end.min(out.len()));
        }
        Ok(if header.encoding == 65001 {
            String::from_utf8_lossy(&out).into_owned()
        } else {
            encoding_rs::WINDOWS_1252.decode(&out).0.into_owned()
        })
    }
}

/// End of flow 0 from a KF8 `FDST` record (`"FDST"`, table offset, count, then `(start, end)`
/// pairs).
fn first_flow_end(fdst: &[u8]) -> Option<usize> {
    if fdst.get(..4) != Some(b"FDST".as_slice()) {
        return None;
    }
    let table = be32(fdst, 4)? as usize;
    Some(be32(fdst, table + 4)? as usize)
}

/// The table of contents a MOBI 6 book links from its guide: `<a filepos=N>title</a>` entries on
/// the page the `<reference type="toc" filepos=…>` points at, as `(title, byte offset)`.
fn filepos_toc(html: &str) -> Vec<(String, usize)> {
    static REFERENCE: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let filepos = r#"filepos\s*=\s*["']?0*(\d+)"#;
    let reference = REFERENCE.get_or_init(|| {
        Regex::new(&format!(r#"(?i)<reference\b[^>]*\btype\s*=\s*["']?toc\b[^>]*?{filepos}|<reference\b[^>]*?{filepos}[^>]*\btype\s*=\s*["']?toc\b"#))
            .expect("valid regex")
    });
    let link = LINK.get_or_init(|| {
        Regex::new(&format!(r"(?is)<a\b[^>]*?{filepos}[^>]*>(.*?)</a>")).expect("valid regex")
    });
    let tag = TAG.get_or_init(|| Regex::new(r"<[^>]*>").expect("valid regex"));

    let Some(start) = reference.captures(html).and_then(|c| {
        c.get(1)
            .or_else(|| c.get(2))
            .and_then(|m| m.as_str().parse::<usize>().ok())
    }) else {
        return Vec::new();
    };
    let Some(page) = html.get(start..) else {
        return Vec::new();
    };
    // The list ends at the next page break (skipping one right at its start).
    let end = page
        .get(1..)
        .and_then(|rest| rest.find("<mbp:pagebreak"))
        .map_or(page.len(), |n| n + 1);
    link.captures_iter(&page[..end])
        .filter_map(|c| {
            let offset = c[1].parse::<usize>().ok()?;
            let title = markup::decode_entities(&tag.replace_all(&c[2], ""));
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            (!title.is_empty()).then_some((title, offset))
        })
        .collect()
}

impl Container for Mobi {
    fn image(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        let record = *self
            .images
            .get(image.0)
            .ok_or_else(|| Failure::unreadable("unknown picture"))?;
        let bytes = self
            .record(record)
            .ok_or_else(|| Failure::unreadable(format!("record {record} missing")))?;
        if ImageKind::sniff(bytes).is_none() {
            return Err(Failure::unreadable(format!(
                "record {record} is not a picture"
            )));
        }
        Ok(bytes.to_vec())
    }

    fn title(&self) -> Option<String> {
        self.title.clone()
    }

    fn author(&self) -> Option<String> {
        self.author.clone()
    }
}

fn intern(images: &mut Vec<usize>, record: usize) -> ImageRef {
    match images.iter().position(|&r| r == record) {
        Some(i) => ImageRef(i),
        None => {
            images.push(record);
            ImageRef(images.len() - 1)
        }
    }
}

/// Record byte ranges from the PalmDB header (78 bytes, then 8 bytes per record).
fn palmdb_records(data: &[u8]) -> Option<Vec<Range<usize>>> {
    let count = be16(data, 76)? as usize;
    let offsets: Vec<usize> = (0..count)
        .map(|i| be32(data, 78 + 8 * i).map(|o| o as usize))
        .collect::<Option<_>>()?;
    let mut records = Vec::with_capacity(count);
    for (i, &start) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(data.len());
        if start > end || end > data.len() {
            return None;
        }
        records.push(start..end);
    }
    (!records.is_empty()).then_some(records)
}

/// `kindle:embed:XXXX?mime=…` → the 1-based picture number (base 32, digits `0-9A-V`).
fn kindle_embed_index(src: &str) -> Option<usize> {
    let code = src.strip_prefix("kindle:embed:")?;
    let code = code.split(['?', '#']).next()?;
    usize::from_str_radix(code, 32).ok()
}

/// Bytes at the end of a text record that are not text (multibyte overlap and trailing entries,
/// per the MOBI header's extra-data flags).
fn trailing_size(record: &[u8], flags: u16) -> usize {
    let entry_size = |data: &[u8], end: usize| -> usize {
        let (mut result, mut shift, mut pos) = (0usize, 0u32, end);
        while pos > 0 {
            let v = data[pos - 1];
            result |= usize::from(v & 0x7F) << shift;
            shift += 7;
            pos -= 1;
            if v & 0x80 != 0 || shift >= 28 {
                break;
            }
        }
        result
    };
    let mut num = 0usize;
    let mut bits = flags >> 1;
    while bits != 0 {
        if bits & 1 != 0 && num < record.len() {
            num += entry_size(record, record.len() - num);
        }
        bits >>= 1;
    }
    if flags & 1 != 0 && num < record.len() {
        num += usize::from(record[record.len() - num - 1] & 0x3) + 1;
    }
    num
}

/// PalmDOC LZ77.
fn palmdoc_decompress(input: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let end = (i + c as usize).min(input.len());
                out.extend_from_slice(&input[i..end]);
                i = end;
            }
            0x00 | 0x09..=0x7F => out.push(c),
            0x80..=0xBF => {
                let Some(&next) = input.get(i) else { break };
                i += 1;
                let pair = (u16::from(c) << 8 | u16::from(next)) & 0x3FFF;
                let distance = usize::from(pair >> 3);
                let length = usize::from(pair & 7) + 3;
                if distance == 0 || distance > out.len() - start {
                    continue;
                }
                for _ in 0..length {
                    out.push(out[out.len() - distance]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdst_first_flow() {
        let mut fdst = b"FDST".to_vec();
        for n in [12u32, 2, 0, 100, 100, 140] {
            fdst.extend_from_slice(&n.to_be_bytes());
        }
        assert_eq!(first_flow_end(&fdst), Some(100));
        assert_eq!(first_flow_end(b"BOUNDARY"), None);
    }

    #[test]
    fn guide_table_of_contents() {
        let body = r#"<html><head><guide><reference title="Contents" type="toc" filepos=0000000XXX /></guide></head><body><p>One</p><mbp:pagebreak/><p><font size="5"><b>Table of Contents</b></font></p><p><a filepos=0000000010>RASHOMON</a></p><p><a filepos="0000000020"><b>THE &amp; END</b></a></p><mbp:pagebreak/><p><a filepos=30>not toc</a></p></body></html>"#;
        let at = body.find("<mbp:pagebreak/><p><font").unwrap();
        let html = body.replace("0000000XXX", &format!("{at:010}"));
        let at = html.find("<mbp:pagebreak/><p><font").unwrap();
        assert_eq!(&html[at..at + 4], "<mbp");
        assert_eq!(
            filepos_toc(&html),
            vec![("RASHOMON".into(), 10), ("THE & END".into(), 20)]
        );
        assert!(filepos_toc("<p>no guide</p>").is_empty());
    }

    #[test]
    fn palmdoc_all_token_kinds() {
        // literal "ab", 2 raw bytes "cd", space+'a', then copy 3 from distance 2.
        let input = [b'a', b'b', 0x02, b'c', b'd', 0xE1, 0x80, 0x10];
        let mut out = Vec::new();
        palmdoc_decompress(&input, &mut out);
        assert_eq!(out, b"abcd a a ");
    }

    #[test]
    fn trailing_entries_and_multibyte() {
        // One trailing entry of size 3 (encoded as 0x83 at the very end), flag bit 1.
        let record = [b'x', b'y', 0xAA, 0xBB, 0x83];
        assert_eq!(trailing_size(&record, 0b10), 3);
        // Multibyte only: last byte & 3 = 1 → 2 bytes.
        assert_eq!(trailing_size(&[b'x', 0, 1], 0b1), 2);
    }

    #[test]
    fn kindle_embed() {
        assert_eq!(
            kindle_embed_index("kindle:embed:0001?mime=image/jpeg"),
            Some(1)
        );
        assert_eq!(kindle_embed_index("kindle:embed:000A"), Some(10));
        assert_eq!(kindle_embed_index("images/1.jpg"), None);
    }
}
