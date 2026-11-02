//! The format-independent shape of a text book: a flat list of blocks to typeset. EPUB, MOBI, FB2
//! and TXT each produce one; [`typeset`] turns it into pages.

pub mod typeset;

use std::sync::OnceLock;

use regex::Regex;

/// Longest line still considered a heading rather than a paragraph mentioning a chapter.
const HEADING_MAX_CHARS: usize = 50;
/// A book's own table of contents with fewer entries than this is completed from its text.
const SPARSE_OUTLINE: usize = 3;

/// A picture a text book refers to, as an index into the backend's own table of image sources
/// (zip entry, PalmDB record, FB2 binary). Never the decoded pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageRef(pub usize);

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `level` 1 is the largest.
    Heading {
        level: u8,
        text: String,
    },
    Para {
        text: String,
    },
    Image(ImageRef),
    /// Start the next block on a fresh page (chapter files, sections).
    PageBreak,
}

/// Script family a book is set in; picks the CJK font variant whose glyph shapes match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    Ja,
    Ko,
    Latin,
}

impl Lang {
    /// From a language tag (`zh-CN`, `ja`, `ko-KR`, `en`), if it names one.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        match primary.as_str() {
            "zh" | "zho" | "chi" | "yue" => Some(Self::Zh),
            "ja" | "jpn" => Some(Self::Ja),
            "ko" | "kor" => Some(Self::Ko),
            "" => None,
            _ => Some(Self::Latin),
        }
    }

    /// From the text itself: kana means Japanese, hangul Korean, other CJK Chinese.
    pub fn guess(sample: &str) -> Self {
        let (mut kana, mut hangul, mut han) = (0usize, 0usize, 0usize);
        for c in sample.chars().take(20_000) {
            match c as u32 {
                0x3040..=0x30FF => kana += 1,
                0xAC00..=0xD7AF | 0x1100..=0x11FF => hangul += 1,
                0x4E00..=0x9FFF | 0x3400..=0x4DBF => han += 1,
                _ => {}
            }
        }
        if kana > 20 {
            Self::Ja
        } else if hangul > 20 {
            Self::Ko
        } else if han > 20 {
            Self::Zh
        } else {
            Self::Latin
        }
    }
}

/// A table-of-contents entry anchored at a block; [`typeset`] resolves it to a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub title: String,
    pub block: usize,
    pub depth: u32,
}

#[derive(Debug, Clone, Default)]
pub struct FlowBook {
    pub blocks: Vec<Block>,
    pub anchors: Vec<Anchor>,
    pub lang: Option<Lang>,
}

impl FlowBook {
    /// Appends a block, dropping empty paragraphs and redundant page breaks.
    pub fn push(&mut self, block: Block) {
        match &block {
            Block::Para { text } | Block::Heading { text, .. } if text.trim().is_empty() => return,
            Block::PageBreak if matches!(self.blocks.last(), None | Some(Block::PageBreak)) => {
                return
            }
            _ => {}
        }
        self.blocks.push(block);
    }

    /// Records a heading as a table-of-contents entry at the next block pushed.
    pub fn anchor_here(&mut self, title: &str, depth: u32) {
        let title = collapse_ascii_whitespace(title);
        if !title.is_empty() {
            self.anchors.push(Anchor {
                title,
                block: self.blocks.len(),
                depth,
            });
        }
    }

    /// Headings of levels 1–3 as anchors, for formats with no table of contents of their own.
    pub fn anchors_from_headings(&mut self) {
        self.anchors = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| match b {
                Block::Heading { level, text } if *level <= 3 => Some(Anchor {
                    title: collapse_ascii_whitespace(text),
                    block: i,
                    depth: u32::from(level - 1),
                }),
                _ => None,
            })
            .collect();
    }

    /// Completes a missing or token table of contents (often only a licence or cover entry) with
    /// the book's own headings and chapter-like lines, when those are more numerous.
    pub fn complete_sparse_outline(&mut self) {
        if self.anchors.len() >= SPARSE_OUTLINE {
            return;
        }
        let detected: Vec<Anchor> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(block, b)| match b {
                Block::Heading { level, text } if *level <= 3 => Some((block, text)),
                Block::Para { text } if is_chapter_heading(text) => Some((block, text)),
                _ => None,
            })
            .map(|(block, text)| Anchor {
                title: collapse_ascii_whitespace(text),
                block,
                depth: 0,
            })
            .collect();
        if detected.len() <= self.anchors.len() {
            return;
        }
        for anchor in detected {
            if !self
                .anchors
                .iter()
                .any(|a| a.block == anchor.block || a.title == anchor.title)
            {
                self.anchors.push(anchor);
            }
        }
        self.anchors.sort_by_key(|a| a.block);
    }

    pub fn language(&self) -> Lang {
        self.lang.unwrap_or_else(|| {
            let sample: String = self
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Para { text } | Block::Heading { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .take(400)
                .collect();
            Lang::guess(&sample)
        })
    }
}

/// Whether a line reads like a chapter heading (第一章, 第十二回, 序章, Chapter 3, …). Keywords
/// must stand alone or be followed by a separator, and sentence punctuation disqualifies a line,
/// so hard-wrapped prose that merely starts with one ("第四回中既將…，", "Part of me…") is not
/// taken for a heading.
pub fn is_chapter_heading(line: &str) -> bool {
    const SENTENCE_PUNCTUATION: &[char] = &['，', '。', '；', '！', '？', '．'];
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"^(?:第\s*[0-9０-９零〇一二三四五六七八九十百千万萬两兩]+\s*[章回节節卷集部篇話话幕]",
            r"|(?:序章|序言|序幕|楔子|引子|尾声|尾聲|终章|終章|后记|後記|番外篇?|あとがき|プロローグ|エピローグ)(?:$|[\s：:・—-])",
            r"|(?i:(?:chapter|part|book)\s+(?:\d+|[ivxlcdm]+|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve)\b(?:$|\s*[:.\-–—]))",
            r"|(?i:(?:prologue|epilogue)(?:$|\s*[:.\-–—])))",
        ))
        .expect("valid chapter regex")
    });
    let line = line.trim();
    line.chars().count() <= HEADING_MAX_CHARS
        && !line.contains('\n')
        && !line.contains(SENTENCE_PUNCTUATION)
        && re.is_match(line)
}

#[cfg(test)]
mod chapter_tests {
    use super::is_chapter_heading;

    #[test]
    fn headings_and_prose_lines() {
        for heading in [
            "第一章 春天",
            "第１２回　夏",
            "第一零零回　　破好事香菱結深恨　悲遠嫁寶玉感离情",
            "序章",
            "番外篇　夏祭り",
            "Chapter 3: Autumn",
            "CHAPTER IV",
            "Part One",
            "Epilogue",
        ] {
            assert!(is_chapter_heading(heading), "{heading}");
        }
        for prose in [
            "第四回中既將薛家母子在榮府內寄居等事略已表明，此回則暫不能寫矣．如",
            "第二回了．你怎么嘗不出來？隔年蠲的雨水那有這樣輕浮，",
            "Part of me wanted to stay",
            "Chapters are short",
            "番外の話をしよう",
            "これは第一章のことだ",
        ] {
            assert!(!is_chapter_heading(prose), "{prose}");
        }
    }
}

/// Joins ASCII whitespace runs into single spaces; full-width spaces are part of CJK titles.
fn collapse_ascii_whitespace(s: &str) -> String {
    s.split(|c: char| c.is_ascii_whitespace())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_and_guesses() {
        assert_eq!(Lang::from_tag("ja-JP"), Some(Lang::Ja));
        assert_eq!(Lang::from_tag("zh_CN"), Some(Lang::Zh));
        assert_eq!(Lang::from_tag("en"), Some(Lang::Latin));
        assert_eq!(
            Lang::guess(&"吾輩は猫である。名前はまだ無い。".repeat(5)),
            Lang::Ja
        );
        assert_eq!(
            Lang::guess(&"春天来了，小明在公园里散步。".repeat(5)),
            Lang::Zh
        );
        assert_eq!(Lang::guess("Hello world"), Lang::Latin);
    }

    #[test]
    fn sparse_outlines_are_completed_from_the_text() {
        let mut book = FlowBook::default();
        for text in [
            "第一回　甄士隱",
            "正文。",
            "第二回　賈夫人",
            "正文。",
            "第三回　金陵城",
        ] {
            book.push(Block::Para { text: text.into() });
        }
        book.push(Block::Heading {
            level: 2,
            text: "LICENSE".into(),
        });
        book.anchors.push(Anchor {
            title: "LICENSE".into(),
            block: 5,
            depth: 0,
        });
        book.complete_sparse_outline();
        let titles: Vec<_> = book.anchors.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "第一回　甄士隱",
                "第二回　賈夫人",
                "第三回　金陵城",
                "LICENSE"
            ]
        );
    }

    #[test]
    fn rich_outlines_are_left_alone() {
        let mut book = FlowBook::default();
        book.push(Block::Para {
            text: "第一章 x".into(),
        });
        for title in ["a", "b", "c"] {
            book.anchors.push(Anchor {
                title: title.into(),
                block: 0,
                depth: 0,
            });
        }
        book.complete_sparse_outline();
        assert_eq!(book.anchors.len(), 3);
    }

    #[test]
    fn push_skips_empty_paragraphs_and_double_breaks() {
        let mut book = FlowBook::default();
        book.push(Block::PageBreak);
        book.push(Block::Para { text: "  ".into() });
        book.push(Block::Para { text: "a".into() });
        book.push(Block::PageBreak);
        book.push(Block::PageBreak);
        assert_eq!(book.blocks.len(), 2);
    }
}
