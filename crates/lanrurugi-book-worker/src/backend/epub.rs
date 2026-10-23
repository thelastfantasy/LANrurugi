//! EPUB 2/3. Reading order is the package spine; pages are either one stored picture per spine
//! item (fixed-layout comics) or the spine's XHTML typeset as a text flow. Only the zip's central
//! directory is read up front; entries are streamed when needed.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::ZipArchive;

use super::{Container, Failure, Pages, Result};
use crate::flow::{Block, FlowBook, ImageRef, Lang};
use crate::markup::{self, Tag};
use lanrurugi_book::OutlineItem;

/// Bytes read from a picture entry to learn its format and size.
const HEAD_BYTES: u64 = 256 * 1024;
/// Font obfuscation (IDPF, Adobe) protects embedded fonts only — not DRM.
const FONT_OBFUSCATION: &[&str] = &[
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

pub struct Epub {
    zip: ZipArchive<File>,
    /// Zip entry of every picture referenced, indexed by [`ImageRef`].
    images: Vec<String>,
    title: Option<String>,
    author: Option<String>,
}

struct ManifestItem {
    path: String,
    media_type: String,
    properties: String,
}

struct TocEntry {
    title: String,
    /// Zip entry the entry links to, and the element id within it.
    path: String,
    fragment: Option<String>,
    depth: u32,
}

#[derive(Default)]
struct Package {
    title: Option<String>,
    author: Option<String>,
    lang: Option<String>,
    fixed_layout: bool,
    manifest: HashMap<String, ManifestItem>,
    /// Manifest ids of the linear spine items, in reading order.
    spine: Vec<String>,
    ncx_id: Option<String>,
}

impl Epub {
    pub fn open(path: &str) -> Result<(Self, Pages)> {
        let zip = ZipArchive::new(File::open(path)?)
            .map_err(|e| Failure::unreadable(format!("EPUB: {e}")))?;
        let mut epub = Epub {
            zip,
            images: Vec::new(),
            title: None,
            author: None,
        };
        epub.check_drm()?;

        let container = epub.read_text("META-INF/container.xml")?;
        let opf_path =
            rootfile(&container).ok_or_else(|| Failure::unreadable("EPUB: no package document"))?;
        let opf = epub.read_text(&opf_path)?;
        let package = parse_package(&opf, dir_of(&opf_path));
        epub.title = package.title.clone();
        epub.author = package.author.clone();

        let spine: Vec<&ManifestItem> = package
            .spine
            .iter()
            .filter_map(|id| package.manifest.get(id))
            .collect();
        let toc = epub.outline(&package);

        // Each linear spine item as blocks; an item that *is* a picture becomes an image block.
        // `targets` holds, per table-of-contents entry, its chapter and block within it.
        let mut chapters: Vec<Vec<Block>> = Vec::with_capacity(spine.len());
        let mut targets: Vec<Option<(usize, usize)>> = vec![None; toc.len()];
        for item in &spine {
            let chapter_index = chapters.len();
            let entries: Vec<usize> = (0..toc.len())
                .filter(|&e| toc[e].path == item.path)
                .collect();
            let mut chapter = FlowBook::default();
            if item.media_type.starts_with("image/") {
                chapter.push(Block::Image(epub.image_ref(&item.path)));
                for &e in &entries {
                    targets[e] = Some((chapter_index, 0));
                }
            } else if item.media_type.contains("html") || item.media_type.contains("xml") {
                let Ok(xhtml) = epub.read_text(&item.path) else {
                    continue;
                };
                let ids: Vec<markup::Target> = entries
                    .iter()
                    .filter_map(|&e| toc[e].fragment.as_deref().map(markup::Target::Id))
                    .collect();
                let base = dir_of(&item.path).to_owned();
                let images = &mut epub.images;
                let found = markup::html_to_blocks(
                    &xhtml,
                    &mut chapter,
                    &mut |tag: &Tag| {
                        let src = tag.attr("src").or_else(|| tag.attr("href"))?;
                        let path = resolve(&base, &src);
                        Some(intern(images, path))
                    },
                    &ids,
                );
                let mut found = found.into_iter();
                for &e in &entries {
                    let block = match toc[e].fragment {
                        Some(_) => found.next().flatten(),
                        None => None,
                    };
                    targets[e] = Some((chapter_index, block.unwrap_or(0)));
                }
            }
            chapters.push(chapter.blocks);
        }

        let single_picture = |blocks: &[Block]| matches!(blocks, [Block::Image(_)]);
        let image_book = !chapters.is_empty() && chapters.iter().all(|b| single_picture(b));
        let pages = if image_book {
            let pages = chapters
                .iter()
                .map(|blocks| match blocks[0] {
                    Block::Image(image) => image,
                    _ => unreachable!("checked above"),
                })
                .collect();
            let outline = toc
                .into_iter()
                .zip(targets)
                .filter_map(|(entry, target)| {
                    Some(OutlineItem {
                        title: entry.title,
                        page: target?.0 as u32,
                        depth: entry.depth,
                    })
                })
                .collect();
            Pages::Images { pages, outline }
        } else {
            let mut flow = FlowBook {
                lang: package.lang.as_deref().and_then(Lang::from_tag),
                ..Default::default()
            };
            // Where each chapter's blocks landed in the flow (pushing can drop a redundant break).
            let mut positions: Vec<Vec<usize>> = Vec::with_capacity(chapters.len());
            for blocks in chapters {
                flow.push(Block::PageBreak);
                let mut chapter_positions = Vec::with_capacity(blocks.len() + 1);
                for block in blocks {
                    chapter_positions.push(flow.blocks.len());
                    flow.push(block);
                }
                chapter_positions.push(flow.blocks.len());
                positions.push(chapter_positions);
            }
            for (entry, target) in toc.into_iter().zip(targets) {
                let Some((chapter, block)) = target else {
                    continue;
                };
                let chapter_positions = &positions[chapter];
                flow.anchors.push(crate::flow::Anchor {
                    title: entry.title,
                    block: chapter_positions[block.min(chapter_positions.len() - 1)],
                    depth: entry.depth,
                });
            }
            if flow.anchors.is_empty() {
                flow.anchors_from_headings();
            }
            flow.complete_sparse_outline();
            Pages::Flow(flow)
        };
        Ok((epub, pages))
    }

    fn check_drm(&mut self) -> Result<()> {
        if self.zip.index_for_name("META-INF/rights.xml").is_some() {
            return Err(Failure::encrypted("Adobe DRM (rights.xml)"));
        }
        if let Ok(encryption) = self.read_text("META-INF/encryption.xml") {
            let mut reader = Reader::from_str(&encryption);
            loop {
                match reader.read_event() {
                    Ok(Event::Start(e) | Event::Empty(e))
                        if e.local_name().as_ref() == b"EncryptionMethod" =>
                    {
                        let algorithm = attr(&e, "Algorithm").unwrap_or_default();
                        if !FONT_OBFUSCATION.contains(&algorithm.as_str()) {
                            return Err(Failure::encrypted(format!(
                                "encrypted content ({algorithm})"
                            )));
                        }
                    }
                    Ok(Event::Eof) | Err(_) => break,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn read_entry(&mut self, name: &str, limit: Option<u64>) -> Result<Vec<u8>> {
        let entry = self
            .zip
            .by_name(name)
            .map_err(|e| Failure::unreadable(format!("EPUB entry {name}: {e}")))?;
        let mut bytes = Vec::with_capacity(entry.size().min(limit.unwrap_or(u64::MAX)) as usize);
        match limit {
            Some(limit) => entry.take(limit).read_to_end(&mut bytes)?,
            None => {
                let mut entry = entry;
                entry.read_to_end(&mut bytes)?
            }
        };
        Ok(bytes)
    }

    fn read_text(&mut self, name: &str) -> Result<String> {
        let bytes = self.read_entry(name, None)?;
        Ok(match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => super::txt::decode(e.as_bytes()),
        })
    }

    fn image_ref(&mut self, path: &str) -> ImageRef {
        intern(&mut self.images, path.to_owned())
    }

    /// Table-of-contents entries from the EPUB 3 navigation document, else the NCX.
    fn outline(&mut self, package: &Package) -> Vec<TocEntry> {
        let nav = package
            .manifest
            .values()
            .find(|i| i.properties.split_whitespace().any(|p| p == "nav"));
        let ncx = package
            .ncx_id
            .as_ref()
            .and_then(|id| package.manifest.get(id))
            .or_else(|| {
                package
                    .manifest
                    .values()
                    .find(|i| i.media_type == "application/x-dtbncx+xml")
            });
        let (doc_path, entries) = if let Some(nav) = nav {
            let path = nav.path.clone();
            let Ok(text) = self.read_text(&path) else {
                return Vec::new();
            };
            (path, parse_nav(&text))
        } else if let Some(ncx) = ncx {
            let path = ncx.path.clone();
            let Ok(text) = self.read_text(&path) else {
                return Vec::new();
            };
            (path, parse_ncx(&text))
        } else {
            return Vec::new();
        };
        let base = dir_of(&doc_path);
        entries
            .into_iter()
            .filter(|(title, _, _)| !title.is_empty())
            .map(|(title, href, depth)| {
                let (file, fragment) = match href.split_once('#') {
                    Some((file, fragment)) => (file, Some(percent_decode(fragment))),
                    None => (href.as_str(), None),
                };
                TocEntry {
                    path: resolve(base, file),
                    fragment: fragment.filter(|f| !f.is_empty()),
                    title,
                    depth,
                }
            })
            .collect()
    }
}

impl Container for Epub {
    fn image(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        let name = self
            .images
            .get(image.0)
            .cloned()
            .ok_or_else(|| Failure::unreadable("unknown picture"))?;
        self.read_entry(&name, None)
    }

    fn image_head(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        let name = self
            .images
            .get(image.0)
            .cloned()
            .ok_or_else(|| Failure::unreadable("unknown picture"))?;
        self.read_entry(&name, Some(HEAD_BYTES))
    }

    fn title(&self) -> Option<String> {
        self.title.clone()
    }

    fn author(&self) -> Option<String> {
        self.author.clone()
    }
}

fn intern(images: &mut Vec<String>, path: String) -> ImageRef {
    match images.iter().position(|p| *p == path) {
        Some(i) => ImageRef(i),
        None => {
            images.push(path);
            ImageRef(images.len() - 1)
        }
    }
}

fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == local.as_bytes())
            .then(|| a.unescape_value().ok().map(|v| v.into_owned()))
            .flatten()
    })
}

fn reader(text: &str) -> Reader<&[u8]> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = false;
    reader
}

fn rootfile(container: &str) -> Option<String> {
    let mut reader = reader(container);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"rootfile" => {
                return attr(&e, "full-path");
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

fn parse_package(opf: &str, base: &str) -> Package {
    let mut package = Package::default();
    let mut reader = reader(opf);
    let mut current: Option<String> = None;
    let mut meta_property: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                text.clear();
                if name == "meta" {
                    meta_property = attr(&e, "property");
                }
                handle_package_element(&mut package, &name, &e, base);
                current = Some(name);
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                handle_package_element(&mut package, &name, &e, base);
            }
            Ok(Event::Text(t)) => text.push_str(&t.decode().unwrap_or_default()),
            Ok(Event::GeneralRef(r)) => text.push_str(&markup::decode_entities(&format!(
                "&{};",
                String::from_utf8_lossy(&r)
            ))),
            Ok(Event::End(_)) => {
                let value = text.trim().to_owned();
                match current.take().as_deref() {
                    Some("title") if package.title.is_none() && !value.is_empty() => {
                        package.title = Some(value)
                    }
                    Some("creator") if package.author.is_none() && !value.is_empty() => {
                        package.author = Some(value)
                    }
                    Some("language") if package.lang.is_none() && !value.is_empty() => {
                        package.lang = Some(value)
                    }
                    Some("meta") if meta_property.as_deref() == Some("rendition:layout") => {
                        package.fixed_layout |= value == "pre-paginated";
                    }
                    _ => {}
                }
                text.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    package
}

fn handle_package_element(package: &mut Package, name: &str, e: &BytesStart, base: &str) {
    match name {
        "item" => {
            if let (Some(id), Some(href)) = (attr(e, "id"), attr(e, "href")) {
                package.manifest.insert(
                    id,
                    ManifestItem {
                        path: resolve(base, &href),
                        media_type: attr(e, "media-type").unwrap_or_default(),
                        properties: attr(e, "properties").unwrap_or_default(),
                    },
                );
            }
        }
        "itemref" => {
            if attr(e, "linear").as_deref() != Some("no") {
                if let Some(idref) = attr(e, "idref") {
                    package.spine.push(idref);
                }
            }
        }
        "spine" => package.ncx_id = attr(e, "toc"),
        // EPUB 2 / Kindle-style `<meta name="fixed-layout" content="true"/>`.
        "meta" if attr(e, "name").as_deref() == Some("fixed-layout") => {
            package.fixed_layout |= attr(e, "content").as_deref() == Some("true");
        }
        _ => {}
    }
}

/// `(title, href, depth)` from the `toc` nav of an EPUB 3 navigation document.
fn parse_nav(xhtml: &str) -> Vec<(String, String, u32)> {
    let mut reader = reader(xhtml);
    let mut out = Vec::new();
    let mut in_toc = false;
    let mut ol_depth = 0u32;
    let mut link: Option<(String, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"nav" => {
                    if attr(&e, "type").is_some_and(|t| t.split_whitespace().any(|t| t == "toc")) {
                        in_toc = true;
                    }
                }
                b"ol" if in_toc => ol_depth += 1,
                b"a" if in_toc => {
                    link = Some((attr(&e, "href").unwrap_or_default(), String::new()))
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if let Some((_, title)) = &mut link {
                    title.push_str(&t.decode().unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some((_, title)) = &mut link {
                    title.push_str(&markup::decode_entities(&format!(
                        "&{};",
                        String::from_utf8_lossy(&r)
                    )));
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"nav" => in_toc = false,
                b"ol" if in_toc => ol_depth = ol_depth.saturating_sub(1),
                b"a" => {
                    if let Some((href, title)) = link.take() {
                        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                        out.push((title, href, ol_depth.saturating_sub(1)));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// `(title, src, depth)` from an EPUB 2 NCX `navMap`.
fn parse_ncx(ncx: &str) -> Vec<(String, String, u32)> {
    let mut reader = reader(ncx);
    let mut out: Vec<(String, String, u32)> = Vec::new();
    let mut depth = 0u32;
    let mut in_label = false;
    let mut label = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"navPoint" => {
                    depth += 1;
                    label.clear();
                }
                b"navLabel" => in_label = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if e.local_name().as_ref() == b"content" && depth > 0 => {
                let src = attr(&e, "src").unwrap_or_default();
                let title = label.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push((title, src, depth - 1));
            }
            Ok(Event::Text(t)) if in_label => label.push_str(&t.decode().unwrap_or_default()),
            Ok(Event::GeneralRef(r)) if in_label => label.push_str(&markup::decode_entities(
                &format!("&{};", String::from_utf8_lossy(&r)),
            )),
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"navPoint" => depth = depth.saturating_sub(1),
                b"navLabel" => in_label = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// `href` relative to directory `base`, percent-decoded and normalised, as a zip entry name.
fn resolve(base: &str, href: &str) -> String {
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|p| !p.is_empty()).collect()
    };
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_like_urls() {
        assert_eq!(
            resolve("OEBPS/text", "../images/a%20b.jpg"),
            "OEBPS/images/a b.jpg"
        );
        assert_eq!(resolve("", "chapter.xhtml"), "chapter.xhtml");
        assert_eq!(resolve("OEBPS", "./x/../y.xhtml"), "OEBPS/y.xhtml");
    }

    #[test]
    fn nav_and_ncx_outlines() {
        let nav = r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <nav epub:type="landmarks"><ol><li><a href="x.xhtml">skip</a></li></ol></nav>
            <nav epub:type="toc"><ol><li><a href="c1.xhtml">第一章</a><ol><li><a href="c1.xhtml#s">一 &amp; 二</a></li></ol></li>
            <li><a href="c2.xhtml">Chapter 2</a></li></ol></nav></body></html>"#;
        assert_eq!(
            parse_nav(nav),
            vec![
                ("第一章".into(), "c1.xhtml".into(), 0),
                ("一 & 二".into(), "c1.xhtml#s".into(), 1),
                ("Chapter 2".into(), "c2.xhtml".into(), 0),
            ]
        );
        let ncx = r#"<ncx><navMap><navPoint id="a"><navLabel><text>One</text></navLabel><content src="c1.xhtml"/>
            <navPoint id="b"><navLabel><text>One.1</text></navLabel><content src="c1.xhtml#x"/></navPoint></navPoint></navMap></ncx>"#;
        assert_eq!(
            parse_ncx(ncx),
            vec![
                ("One".into(), "c1.xhtml".into(), 0),
                ("One.1".into(), "c1.xhtml#x".into(), 1)
            ]
        );
    }
}
