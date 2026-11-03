//! FictionBook 2: one XML file holding the text and, as base64 `<binary>` elements, its pictures.
//! The file is memory-mapped (or transcoded once if it is not UTF-8) and parsed in one streaming
//! pass; pictures are only located (byte ranges), and decoded when their page is drawn.

use std::collections::HashMap;
use std::ops::Range;

use base64::Engine;
use memmap2::Mmap;
use quick_xml::events::Event;
use quick_xml::Reader;

use super::{Container, Failure, Pages, Result};
use crate::flow::{Block, FlowBook, ImageRef, Lang};
use crate::markup::decode_entities;

/// Base64 characters decoded for a header read: ~256 KB of picture.
const HEAD_BASE64: usize = 350_000;

enum Data {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Mapped(m) => m,
            Data::Owned(v) => v,
        }
    }
}

pub struct Fb2 {
    data: Data,
    /// Picture references in document order → their binary's id.
    images: Vec<String>,
    binaries: HashMap<String, Range<usize>>,
    title: Option<String>,
    author: Option<String>,
}

impl Fb2 {
    pub fn open(path: &str) -> Result<(Self, Pages)> {
        let file = std::fs::File::open(path)?;
        // SAFETY: the library file is only read; if it is replaced underneath us the stamp check
        // reopens it, and a truncation mid-read surfaces as a parse error, not UB in safe code we
        // rely on (memmap2's documented caveat for concurrent modification).
        let mapped = unsafe { Mmap::map(&file)? };
        let data = match declared_encoding(&mapped) {
            Some(label) if !label.eq_ignore_ascii_case("utf-8") => {
                let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
                    .ok_or_else(|| Failure::unreadable(format!("unknown encoding {label}")))?;
                let (text, _, _) = encoding.decode(&mapped);
                // The declaration still names the old encoding; quick-xml without its `encoding`
                // feature reads UTF-8 regardless, which is what the bytes now are.
                Data::Owned(text.into_owned().into_bytes())
            }
            _ => Data::Mapped(mapped),
        };
        let mut fb2 = Fb2 {
            data,
            images: Vec::new(),
            binaries: HashMap::new(),
            title: None,
            author: None,
        };
        let mut flow = fb2.parse()?;
        flow.complete_sparse_outline();
        Ok((fb2, Pages::Flow(flow)))
    }

    fn parse(&mut self) -> Result<FlowBook> {
        let data: &[u8] = &self.data;
        let mut reader = Reader::from_reader(data);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = false;

        let mut book = FlowBook::default();
        let mut stack: Vec<String> = Vec::new();
        let mut section_depth = 0u32;
        let mut text = String::new();
        let mut title_parts: Vec<String> = Vec::new();
        let mut author_parts: Vec<String> = Vec::new();
        let mut lang = None;
        let mut images = Vec::new();
        let mut binaries = HashMap::new();
        let mut book_title = None;

        let in_any = |stack: &[String], name: &str| stack.iter().any(|s| s == name);
        let mut saw_root = false;

        loop {
            let event = reader
                .read_event()
                .map_err(|e| Failure::unreadable(format!("FB2: {e}")))?;
            match event {
                Event::Start(e) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    saw_root |= name == "FictionBook";
                    match name.as_str() {
                        "binary" => {
                            let id = attr(&e, "id").unwrap_or_default();
                            let span = reader
                                .read_to_end(e.name())
                                .map_err(|err| Failure::unreadable(format!("FB2 binary: {err}")))?;
                            binaries.insert(id, span.start as usize..span.end as usize);
                            continue;
                        }
                        "section" if in_any(&stack, "body") => {
                            section_depth += 1;
                            if section_depth == 1 {
                                book.push(Block::PageBreak);
                            }
                        }
                        "body" => {
                            book.push(Block::PageBreak);
                            section_depth = 0;
                        }
                        "title" if in_any(&stack, "body") => title_parts.clear(),
                        "p" | "v" | "subtitle" | "text-author" | "book-title" | "first-name"
                        | "middle-name" | "last-name" | "nickname" | "lang" => text.clear(),
                        _ => {}
                    }
                    stack.push(name);
                }
                Event::Empty(e) => {
                    let name = e.local_name();
                    if name.as_ref() == b"image" && in_any(&stack, "body") {
                        if let Some(href) = attr(&e, "href") {
                            flush_para(&mut book, &mut text);
                            images.push(href.trim_start_matches('#').to_owned());
                            book.push(Block::Image(ImageRef(images.len() - 1)));
                        }
                    }
                }
                Event::Text(t) => {
                    let s = t
                        .decode()
                        .map_err(|e| Failure::unreadable(format!("FB2 text: {e}")))?;
                    text.push_str(&s);
                }
                Event::CData(t) => text.push_str(&String::from_utf8_lossy(&t)),
                Event::GeneralRef(r) => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        text.push(c);
                    } else {
                        text.push_str(&decode_entities(&format!(
                            "&{};",
                            String::from_utf8_lossy(&r)
                        )));
                    }
                }
                Event::End(e) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    let inside = |n: &str| stack.iter().rev().skip(1).any(|s| s == n);
                    match name.as_str() {
                        "book-title" if inside("title-info") => book_title = Some(clean(&text)),
                        "first-name" | "middle-name" | "last-name" | "nickname"
                            if inside("author") && inside("title-info") =>
                        {
                            author_parts.push(clean(&text));
                        }
                        "lang" if inside("title-info") => lang = Lang::from_tag(&text),
                        "p" if inside("title") && in_any(&stack, "body") => {
                            title_parts.push(clean(&text))
                        }
                        "p" | "v" | "text-author" if in_any(&stack, "body") => {
                            flush_para(&mut book, &mut text)
                        }
                        "subtitle" if in_any(&stack, "body") => {
                            let line = clean(&text);
                            book.push(Block::Heading {
                                level: 4,
                                text: line,
                            });
                        }
                        "title" if in_any(&stack, "body") => {
                            let heading = title_parts.join(" ");
                            if section_depth > 0 {
                                book.anchor_here(&heading, section_depth - 1);
                            }
                            let level = section_depth.clamp(1, 3) as u8;
                            book.push(Block::Heading {
                                level,
                                text: heading,
                            });
                            title_parts.clear();
                        }
                        "section" if in_any(&stack, "body") => {
                            section_depth = section_depth.saturating_sub(1)
                        }
                        _ => {}
                    }
                    if matches!(
                        name.as_str(),
                        "book-title"
                            | "first-name"
                            | "middle-name"
                            | "last-name"
                            | "nickname"
                            | "lang"
                    ) {
                        text.clear();
                    }
                    stack.pop();
                }
                Event::Eof => break,
                _ => {}
            }
        }

        if !saw_root {
            return Err(Failure::unreadable("not a FictionBook document"));
        }
        book.lang = lang;
        self.images = images;
        self.binaries = binaries;
        self.title = book_title.filter(|t| !t.is_empty());
        let author = author_parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.author = (!author.is_empty()).then_some(author);
        Ok(book)
    }

    fn binary(&self, image: ImageRef, limit: Option<usize>) -> Result<Vec<u8>> {
        let id = self
            .images
            .get(image.0)
            .ok_or_else(|| Failure::unreadable("unknown picture"))?;
        let range = self
            .binaries
            .get(id)
            .ok_or_else(|| Failure::unreadable(format!("picture {id} is missing")))?;
        let encoded = &self.data[range.clone()];
        let mut clean: Vec<u8> = encoded
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        if let Some(limit) = limit {
            clean.truncate(limit - limit % 4);
        }
        base64::engine::general_purpose::STANDARD
            .decode(&clean)
            .map_err(|e| Failure::unreadable(format!("picture {id}: {e}")))
    }
}

impl Container for Fb2 {
    fn image(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        self.binary(image, None)
    }

    fn image_head(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        self.binary(image, Some(HEAD_BASE64))
    }

    fn title(&self) -> Option<String> {
        self.title.clone()
    }

    fn author(&self) -> Option<String> {
        self.author.clone()
    }
}

fn flush_para(book: &mut FlowBook, text: &mut String) {
    let para = clean(text);
    text.clear();
    book.push(Block::Para { text: para });
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(e: &quick_xml::events::BytesStart, local: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.local_name();
        (key.as_ref() == local.as_bytes())
            .then(|| a.unescape_value().ok().map(|v| v.into_owned()))
            .flatten()
    })
}

/// The `encoding` of the XML declaration, if any.
fn declared_encoding(data: &[u8]) -> Option<String> {
    let head = &data[..data.len().min(200)];
    let head = String::from_utf8_lossy(head);
    let decl = head.strip_prefix('\u{FEFF}').unwrap_or(&head);
    let decl = decl.strip_prefix("<?xml")?;
    let decl = &decl[..decl.find("?>")?];
    let at = decl.find("encoding")?;
    let value = decl[at + 8..].trim_start().strip_prefix('=')?.trim_start();
    let quote = value.chars().next()?;
    let value = &value[1..];
    Some(value[..value.find(quote)?].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_declaration() {
        assert_eq!(
            declared_encoding(b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><FictionBook/>")
                .as_deref(),
            Some("windows-1251")
        );
        assert_eq!(declared_encoding(b"<FictionBook/>"), None);
    }
}
