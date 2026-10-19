//! Tolerant (X)HTML → [`Block`]s, shared by EPUB chapters and MOBI text. E-book markup in the wild
//! is frequently malformed, so this is a forgiving tokenizer that only cares about block
//! structure (headings, paragraphs, line breaks, pictures), not a validating XML/HTML parser.

use crate::flow::{is_chapter_heading, Block, FlowBook, ImageRef};

/// An opening tag as seen by the image resolver.
pub struct Tag<'a> {
    pub name: &'a str,
    raw_attrs: &'a str,
}

impl Tag<'_> {
    /// The value of attribute `key` (case-insensitive; `href` also matches `xlink:href`).
    pub fn attr(&self, key: &str) -> Option<String> {
        let mut rest = self.raw_attrs;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
            if rest.is_empty() {
                return None;
            }
            let name_end = rest
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')
                .unwrap_or(rest.len());
            let name = &rest[..name_end];
            rest = rest[name_end..].trim_start();
            let value = if let Some(after) = rest.strip_prefix('=') {
                let after = after.trim_start();
                let (value, remaining) = match after.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let body = &after[1..];
                        let end = body.find(q).unwrap_or(body.len());
                        (&body[..end], body.get(end + 1..).unwrap_or(""))
                    }
                    _ => {
                        let end = after.find(char::is_whitespace).unwrap_or(after.len());
                        (&after[..end], &after[end..])
                    }
                };
                rest = remaining;
                value
            } else {
                ""
            };
            let local = name.rsplit(':').next().unwrap_or(name);
            if name.eq_ignore_ascii_case(key) || local.eq_ignore_ascii_case(key) {
                return Some(decode_entities(value));
            }
        }
    }
}

/// Elements whose content is not reading text.
const SKIPPED: &[&str] = &[
    "head", "script", "style", "title", "rt", "rp", "noscript", "template",
];
/// Elements that end the current paragraph.
const BLOCK_LEVEL: &[&str] = &[
    "p",
    "div",
    "li",
    "blockquote",
    "section",
    "article",
    "tr",
    "dt",
    "dd",
    "pre",
    "figure",
    "figcaption",
    "table",
    "ul",
    "ol",
    "dl",
    "body",
    "header",
    "footer",
    "aside",
    "nav",
    "center",
    "hr",
    "main",
    "address",
    "td",
    "th",
    "caption",
];

/// A place in the markup a table of contents links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// An element `id` (or `<a name>`): an EPUB `file.xhtml#id` link.
    Id(&'a str),
    /// A byte offset into the markup: a MOBI `filepos` link.
    Offset(usize),
}

/// Appends the blocks of `html` to `book`. `resolve_image` turns an `<img>`/`<image>` tag into a
/// picture reference, or `None` to drop it. Returns, for each of `targets`, the index in
/// `book.blocks` of the first text or picture block at or after it (`None` when nothing follows).
pub fn html_to_blocks(
    html: &str,
    book: &mut FlowBook,
    resolve_image: &mut dyn FnMut(&Tag) -> Option<ImageRef>,
    targets: &[Target],
) -> Vec<Option<usize>> {
    let mut offsets: Vec<(usize, usize)> = targets
        .iter()
        .enumerate()
        .filter_map(|(t, target)| match target {
            Target::Offset(offset) => Some((*offset, t)),
            Target::Id(_) => None,
        })
        .collect();
    offsets.sort_unstable();
    let mut offsets = offsets.into_iter().peekable();
    let mut state = State {
        book,
        text: String::new(),
        heading: None,
        font_depth: 0,
        heading_font: None,
        skip: 0,
        pre: 0,
        targets,
        resolved: vec![None; targets.len()],
        pending: Vec::new(),
    };
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while let Some((_, t)) = offsets.next_if(|(offset, _)| *offset <= i) {
            state.pending.push(t);
        }
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |n| i + n);
            if state.skip == 0 {
                state.push_text(&html[i..end]);
            }
            i = end;
            continue;
        }
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |n| n + 3);
            continue;
        }
        if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let end = cdata.find("]]>").unwrap_or(cdata.len());
            if state.skip == 0 {
                state.text.push_str(&cdata[..end]);
            }
            i += 9 + end + 3.min(cdata.len() - end);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            i += rest.find('>').map_or(rest.len(), |n| n + 1);
            continue;
        }
        let Some(tag_len) = tag_end(rest) else {
            // A lone '<' in text.
            if state.skip == 0 {
                state.text.push('<');
            }
            i += 1;
            continue;
        };
        let inner = &rest[1..tag_len - 1];
        i += tag_len;
        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/');
        let self_closing = inner.trim_end().ends_with('/');
        let name_end = inner
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .unwrap_or(inner.len());
        let full_name = inner[..name_end].to_ascii_lowercase();
        let name = full_name
            .rsplit(':')
            .next()
            .unwrap_or(&full_name)
            .to_string();
        let tag = Tag {
            name: &name,
            raw_attrs: &inner[name_end..],
        };
        state.tag(&tag, closing, self_closing, resolve_image);
    }
    state.flush();
    state.resolved
}

/// Length of the tag starting at `s[0] == '<'`, honouring quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || !(bytes[1].is_ascii_alphabetic() || bytes[1] == b'/') {
        return None;
    }
    let mut quote = None;
    for (i, &b) in bytes.iter().enumerate().skip(1) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(i + 1),
            None => {}
        }
    }
    None
}

struct State<'b, 't> {
    book: &'b mut FlowBook,
    text: String,
    heading: Option<u8>,
    /// Open `<font>` elements, and the depth of the one that made the current paragraph a
    /// heading (MOBI 6 marks headings with `<font size="5+">` rather than `<hN>`).
    font_depth: u32,
    heading_font: Option<u32>,
    skip: u32,
    pre: u32,
    targets: &'t [Target<'t>],
    resolved: Vec<Option<usize>>,
    /// Targets passed and waiting for the next text or picture block.
    pending: Vec<usize>,
}

impl State<'_, '_> {
    fn push_block(&mut self, block: Block) {
        let anchors = !matches!(block, Block::PageBreak);
        let before = self.book.blocks.len();
        self.book.push(block);
        if anchors && self.book.blocks.len() > before {
            for t in self.pending.drain(..) {
                self.resolved[t] = Some(before);
            }
        }
    }

    fn tag(
        &mut self,
        tag: &Tag,
        closing: bool,
        self_closing: bool,
        resolve_image: &mut dyn FnMut(&Tag) -> Option<ImageRef>,
    ) {
        let name = tag.name;
        if SKIPPED.contains(&name) {
            if !self_closing {
                if closing {
                    self.skip = self.skip.saturating_sub(1);
                } else {
                    self.skip += 1;
                }
            }
            return;
        }
        if self.skip > 0 {
            return;
        }
        match name {
            "br" => self.text.push('\n'),
            "img" | "image" if !closing => {
                if let Some(image) = resolve_image(tag) {
                    self.flush();
                    self.push_block(Block::Image(image));
                }
            }
            "pagebreak" => {
                self.flush();
                self.push_block(Block::PageBreak);
            }
            "font" if closing => {
                if self.heading_font == Some(self.font_depth) {
                    self.flush();
                    self.heading_font = None;
                }
                self.font_depth = self.font_depth.saturating_sub(1);
            }
            "font" if !self_closing => {
                self.font_depth += 1;
                let level = tag.attr("size").and_then(|size| font_heading_level(&size));
                if let Some(level) = level.filter(|_| self.heading.is_none()) {
                    if self.text.trim().is_empty() {
                        self.heading = Some(level);
                        self.heading_font = Some(self.font_depth);
                    }
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                if !closing {
                    self.heading = Some(name.as_bytes()[1] - b'0');
                }
            }
            "pre" => {
                self.flush();
                if closing {
                    self.pre = self.pre.saturating_sub(1);
                } else if !self_closing {
                    self.pre += 1;
                }
            }
            _ if BLOCK_LEVEL.contains(&name) => self.flush(),
            _ => {}
        }
        if !closing && self.targets.iter().any(|t| matches!(t, Target::Id(_))) {
            let id = tag
                .attr("id")
                .or_else(|| (name == "a").then(|| tag.attr("name")).flatten());
            if let Some(id) = id {
                for (t, target) in self.targets.iter().enumerate() {
                    if *target == Target::Id(&id) && self.resolved[t].is_none() {
                        self.pending.push(t);
                    }
                }
            }
        }
    }

    fn push_text(&mut self, raw: &str) {
        let decoded = decode_entities(raw);
        if self.pre > 0 {
            self.text.push_str(&decoded);
            return;
        }
        // Collapse whitespace like a browser. A run containing a source line break is kept as
        // [`SOFT_BREAK`] and settled in `flush`; a plain run between two CJK characters is dropped.
        let mut pending: Option<bool> = None;
        for c in decoded.chars() {
            if c.is_whitespace() && c != '\u{3000}' && c != '\u{A0}' {
                pending = Some(c == '\n' || pending == Some(true));
                continue;
            }
            if let Some(newline) = pending.take() {
                self.separate(newline, Some(c));
            }
            self.text.push(if c == '\u{A0}' { ' ' } else { c });
        }
        if let Some(newline) = pending {
            self.separate(newline, None);
        }
    }

    fn separate(&mut self, newline: bool, next: Option<char>) {
        if self.text.is_empty() || self.text.ends_with([' ', '\n', SOFT_BREAK]) {
            return;
        }
        if newline {
            self.text.push(SOFT_BREAK);
        } else if !(self.text.chars().last().is_some_and(is_cjk) && next.is_some_and(is_cjk)) {
            self.text.push(' ');
        }
    }

    /// Ends the current paragraph. Source lines that read as chapter headings become paragraphs of
    /// their own: text converted from hard-wrapped plain text often glues "第三回" onto the end of
    /// the paragraph before it.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.text);
        let heading = self.heading.take();
        let mut para = String::new();
        for line in text.lines() {
            let mut joined = String::new();
            for segment in line
                .split(SOFT_BREAK)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if heading.is_none() && is_chapter_heading(segment) {
                    append_line(&mut para, &joined);
                    joined.clear();
                    self.emit(None, std::mem::take(&mut para));
                    self.emit(None, segment.to_owned());
                    continue;
                }
                if joined
                    .chars()
                    .last()
                    .is_some_and(|prev| !(is_cjk(prev) && segment.starts_with(is_cjk)))
                {
                    joined.push(' ');
                }
                joined.push_str(segment);
            }
            append_line(&mut para, &joined);
        }
        self.emit(heading, para);
    }

    fn emit(&mut self, heading: Option<u8>, text: String) {
        if text.is_empty() {
            return;
        }
        match heading {
            Some(level) => self.push_block(Block::Heading { level, text }),
            None => self.push_block(Block::Para { text }),
        }
    }
}

/// Stands for a source line break inside a paragraph until `flush` decides what it becomes.
const SOFT_BREAK: char = '\u{E000}';

fn append_line(para: &mut String, line: &str) {
    if line.is_empty() {
        return;
    }
    if !para.is_empty() {
        para.push('\n');
    }
    para.push_str(line);
}

/// Heading level for a `<font size>` (absolute 1–7, or relative to the default 3), if large
/// enough to be one.
fn font_heading_level(size: &str) -> Option<u8> {
    let size = size.trim();
    let size: i32 = match size.strip_prefix('+') {
        Some(relative) => 3 + relative.parse::<i32>().ok()?,
        None if size.starts_with('-') => return None,
        None => size.parse().ok()?,
    };
    match size {
        6.. => Some(1),
        5 => Some(2),
        _ => None,
    }
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF |
        0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// Decodes character references and the named entities common in e-books.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[1..].find(';').map(|n| n + 1).filter(|&n| n <= 12);
        let decoded = end.and_then(|end| {
            let entity = &rest[1..end];
            let c = if let Some(num) = entity.strip_prefix('#') {
                let value = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                };
                value.and_then(char::from_u32)
            } else {
                named_entity(entity)
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "times" => '×',
        "deg" => '°',
        "bull" => '•',
        "shy" => '\u{AD}',
        "ensp" | "emsp" | "thinsp" => ' ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(html: &str) -> Vec<Block> {
        let mut book = FlowBook::default();
        html_to_blocks(
            html,
            &mut book,
            &mut |tag: &Tag| {
                tag.attr("src")
                    .or_else(|| tag.attr("href"))
                    .map(|s| ImageRef(s.len()))
            },
            &[],
        );
        book.blocks
    }

    #[test]
    fn headings_paragraphs_and_breaks() {
        let html = r#"<?xml version="1.0"?><html><head><title>x</title><style>p{}</style></head>
            <body><h1 class="c">第一章 <b>春天</b></h1><p>春天
            来了。Spring&nbsp;is &amp; here.</p><p>line one<br/>line two</p><div>tail</div></body></html>"#;
        assert_eq!(
            blocks(html),
            vec![
                Block::Heading {
                    level: 1,
                    text: "第一章 春天".into()
                },
                Block::Para {
                    text: "春天来了。Spring is & here.".into()
                },
                Block::Para {
                    text: "line one\nline two".into()
                },
                Block::Para {
                    text: "tail".into()
                },
            ]
        );
    }

    #[test]
    fn chapter_lines_glued_into_paragraphs_are_split_out() {
        let html = "<p>雨村忙回頭看時\n第三回</p><p>第五回\n游幻境指迷十二釵</p><p>wrapped English\nline two</p>";
        assert_eq!(
            blocks(html),
            vec![
                Block::Para {
                    text: "雨村忙回頭看時".into()
                },
                Block::Para {
                    text: "第三回".into()
                },
                Block::Para {
                    text: "第五回".into()
                },
                Block::Para {
                    text: "游幻境指迷十二釵".into()
                },
                Block::Para {
                    text: "wrapped English line two".into()
                },
            ]
        );
    }

    #[test]
    fn ruby_annotations_are_dropped() {
        assert_eq!(
            blocks("<p><ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby>字</p>"),
            vec![Block::Para {
                text: "漢字".into()
            }]
        );
    }

    #[test]
    fn images_including_svg_wrappers() {
        let html = r#"<p>a</p><img src="i/1.jpg" alt="x"/><svg><image xlink:href="i/22.png" width="1"/></svg>"#;
        assert_eq!(
            blocks(html),
            vec![
                Block::Para { text: "a".into() },
                Block::Image(ImageRef("i/1.jpg".len())),
                Block::Image(ImageRef("i/22.png".len())),
            ]
        );
    }

    #[test]
    fn malformed_markup_does_not_lose_text() {
        assert_eq!(
            blocks("<p>a < b <i>unclosed<p>next"),
            vec![
                Block::Para {
                    text: "a < b unclosed".into()
                },
                Block::Para {
                    text: "next".into()
                }
            ]
        );
    }

    #[test]
    fn targets_resolve_to_the_block_that_follows_them() {
        let html =
            r#"<p>intro</p><h2 id="c1">One</h2><p>a <a id="mid"/>b</p><p>c</p><span id="end"/>"#;
        let mut book = FlowBook::default();
        let targets = [
            Target::Id("c1"),
            Target::Id("mid"),
            Target::Id("missing"),
            Target::Offset(html.find("<p>c").unwrap()),
            Target::Id("end"),
        ];
        let found = html_to_blocks(html, &mut book, &mut |_: &Tag| None, &targets);
        assert_eq!(found, vec![Some(1), Some(2), None, Some(3), None]);
    }

    #[test]
    fn mobi_font_headings_and_page_breaks() {
        let html = r#"<p><font size="7"><b>RASHOMON</b></font></p><p>By A</p><mbp:pagebreak/><p><font size="+2"><b>Licence</b></font> tail</p><p><font size="3">small</font></p>"#;
        assert_eq!(
            blocks(html),
            vec![
                Block::Heading {
                    level: 1,
                    text: "RASHOMON".into()
                },
                Block::Para {
                    text: "By A".into()
                },
                Block::PageBreak,
                Block::Heading {
                    level: 2,
                    text: "Licence".into()
                },
                Block::Para {
                    text: "tail".into()
                },
                Block::Para {
                    text: "small".into()
                },
            ]
        );
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("&#x6F22;&#23383;&hellip;&bogus;"),
            "漢字…&bogus;"
        );
    }
}
