//! Plain-text books (web novels and the like): decoded from whatever encoding they were saved in,
//! one paragraph per non-empty line, chapter-heading lines becoming headings and the outline.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};

use super::{Container, Failure, Pages, Result};
use crate::flow::{is_chapter_heading, Block, FlowBook, ImageRef};
use crate::markup::joins_without_space;

/// Bytes fed to the encoding detector; plenty to be confident, bounded for huge files.
const DETECT_BYTES: usize = 1 << 20;

pub struct Txt;

impl Txt {
    pub fn open(path: &str) -> Result<(Self, Pages)> {
        let bytes = std::fs::read(path)?;
        let text = decode(&bytes);
        drop(bytes);
        Ok((Txt, Pages::Flow(flow_from_text(&text))))
    }
}

impl Container for Txt {
    fn image(&mut self, _image: ImageRef) -> Result<Vec<u8>> {
        Err(Failure::unreadable("plain text has no pictures"))
    }

    fn title(&self) -> Option<String> {
        None
    }

    fn author(&self) -> Option<String> {
        None
    }
}

/// Text from bytes in any common encoding: BOM first, then valid UTF-8, then a statistical guess
/// (GBK/GB18030, Big5, Shift-JIS, EUC-JP, EUC-KR, single-byte code pages).
pub fn decode(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding
            .decode_without_bom_handling(&bytes[bom..])
            .0
            .into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    let sample = &bytes[..bytes.len().min(DETECT_BYTES)];
    detector.feed(sample, sample.len() == bytes.len());
    let encoding = detector.guess(None, Utf8Detection::Allow);
    encoding.decode(bytes).0.into_owned()
}

/// Share of lines that must sit at the common line length for a text to count as hard-wrapped.
const WRAPPED_SHARE: f32 = 0.4;
/// Fewer measured lines than this say nothing reliable about wrapping.
const WRAPPED_MIN_LINES: usize = 30;

fn flow_from_text(text: &str) -> FlowBook {
    let wrap = wrap_width(text);
    let mut book = FlowBook::default();
    let mut para = String::new();
    let mut previous_full = false;
    for raw in text.lines() {
        let indented = raw.starts_with(char::is_whitespace);
        let line = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
        if line.is_empty() || is_chapter_heading(line) {
            flush(&mut book, &mut para);
            previous_full = false;
            if !line.is_empty() {
                book.push(Block::PageBreak);
                book.anchor_here(line, 0);
                book.push(Block::Heading {
                    level: 1,
                    text: line.to_owned(),
                });
            }
            continue;
        }
        match wrap {
            // A hard-wrapped line continues the paragraph unless it is indented or the previous
            // line stopped short of the wrap width.
            Some(width) if previous_full && !indented => {
                if !joins_without_space(&para, line) {
                    para.push(' ');
                }
                para.push_str(line);
                previous_full = is_full(raw, width);
            }
            Some(width) => {
                flush(&mut book, &mut para);
                para.push_str(line);
                previous_full = is_full(raw, width);
            }
            None => {
                flush(&mut book, &mut para);
                para.push_str(line);
            }
        }
    }
    flush(&mut book, &mut para);
    book
}

fn flush(book: &mut FlowBook, para: &mut String) {
    if !para.is_empty() {
        book.push(Block::Para {
            text: std::mem::take(para),
        });
    }
}

/// A line's width in display columns, the unit wrapping tools count in: CJK and emoji take two
/// columns, Latin one, combining marks none. Counting characters instead would make a full line of
/// Chinese in a mostly-English text look half empty.
fn line_width(line: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(line.trim_end())
}

/// Whether a line reaches (close to) the wrap width, i.e. was broken by the wrapper rather than at
/// the end of a paragraph.
fn is_full(line: &str, width: usize) -> bool {
    line_width(line) * 100 >= width * 85
}

/// The width a hard-wrapped text (Project Gutenberg and similar) was wrapped at: the most common
/// line width in display columns, when enough lines sit at it. `None` for one-paragraph-per-line text.
fn wrap_width(text: &str) -> Option<usize> {
    let mut counts = std::collections::HashMap::<usize, usize>::new();
    let mut lines = 0usize;
    for line in text.lines().map(line_width).filter(|&n| n >= 10) {
        *counts.entry(line).or_default() += 1;
        lines += 1;
    }
    if lines < WRAPPED_MIN_LINES {
        return None;
    }
    let (&width, _) = counts.iter().max_by_key(|&(len, n)| (*n, *len))?;
    if !(16..=240).contains(&width) {
        return None;
    }
    let near: usize = counts
        .iter()
        .filter(|&(&len, _)| len * 100 >= width * 85 && len <= width + 2)
        .map(|(_, n)| n)
        .sum();
    (near as f32 >= lines as f32 * WRAPPED_SHARE).then_some(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_legacy_chinese_and_japanese() {
        let chinese = "第一章　春天来了。小明在公园里散步，看见樱花开得很漂亮。".repeat(20);
        let (gbk, _, _) = encoding_rs::GBK.encode(&chinese);
        assert!(decode(&gbk).starts_with("第一章　春天来了"));
        let japanese = "吾輩は猫である。名前はまだ無い。".repeat(20);
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(&japanese);
        assert!(decode(&sjis).starts_with("吾輩は猫である"));
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "中文".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode(&utf16), "中文");
    }

    #[test]
    fn hard_wrapped_text_is_rejoined_into_paragraphs() {
        let line = "一二三四五六七八九十一二三四五六七八九十";
        let text = format!(
            "\u{3000}\u{3000}{line}\n{line}\n结束。\n\u{3000}\u{3000}{line}\n{line}\n{line}\n第二回\n{line}\n"
        )
        .repeat(10);
        let book = flow_from_text(&text);
        let paras: Vec<_> = book
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { text } => Some(text.chars().count()),
                _ => None,
            })
            .collect();
        assert_eq!(paras, [43, 60, 20].repeat(10));
    }

    #[test]
    fn line_per_paragraph_text_is_left_alone() {
        let text = "短い。\nこれはもう少し長い一行の段落です。\n三行目はさらに長く書かれた段落で、長さがばらばらです。\n";
        assert_eq!(wrap_width(text), None);
        assert_eq!(flow_from_text(text).blocks.len(), 3);
    }

    #[test]
    fn chapters_become_headings_and_anchors() {
        let book = flow_from_text(
            "书名\n\n序章\n很久以前。\n第一章 春天\n春天来了。\n第１２回　夏\n这是第一章里提到的事情，不是标题，因为它是一整句很长很长很长很长很长很长很长很长很长很长很长很长的话。\nChapter 3: Autumn\nText.\n",
        );
        let titles: Vec<_> = book.anchors.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            ["序章", "第一章 春天", "第１２回　夏", "Chapter 3: Autumn"]
        );
        assert!(matches!(book.blocks[0], Block::Para { .. }));
        assert!(
            book.blocks
                .iter()
                .filter(|b| matches!(b, Block::PageBreak))
                .count()
                >= 3
        );
    }

    #[test]
    fn rejoined_lines_keep_emoji_and_cjk_punctuation_whole() {
        // Wrapped at 20 characters, with one break inside a ZWJ family emoji and one right after
        // an ellipsis.
        let cjk = |n: usize| "春".repeat(n);
        let para = format!(
            "{}👨\u{200D}\n👩\u{200D}👧{}\n{}……\n{}\n完。\n",
            cjk(18),
            cjk(17),
            cjk(18),
            cjk(20)
        );
        let book = flow_from_text(&para.repeat(12));
        let texts: Vec<_> = book
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 12, "{texts:?}");
        let expected = format!(
            "{}👨\u{200D}👩\u{200D}👧{}{}……{}完。",
            cjk(18),
            cjk(17),
            cjk(18),
            cjk(20)
        );
        assert_eq!(texts[0], expected);
    }

    #[test]
    fn mixed_width_lines_are_measured_in_columns() {
        // Wrapped at 40 columns: 40 letters of English, or 20 Chinese characters.
        let en = format!("{}word", "words ".repeat(6));
        let zh = "春".repeat(20);
        let book = flow_from_text(&format!("{en}\n{en}\n{zh}\n{zh}\nend.\n").repeat(10));
        let texts: Vec<_> = book
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec![format!("{en} {en} {zh}{zh} end."); 10]);
    }
}
