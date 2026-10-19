//! Typesets a [`FlowBook`] into fixed-size pages and rasterises one page at a time.
//!
//! [`Typesetter::paginate`] shapes every block once to measure it, records where each page's
//! pieces go, and drops the shaped text immediately — memory is bounded by one block, not by the
//! book. [`Typesetter::render`] re-shapes only the blocks on the requested page. Shaping is
//! deterministic, so both passes break lines identically.

use std::collections::HashSet;

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use image::{imageops, Rgb, RgbImage};

use super::{Block, FlowBook, ImageRef, Lang};

pub const PAGE_WIDTH: u32 = 1200;
pub const PAGE_HEIGHT: u32 = 1700;
const MARGIN: f32 = 96.0;
const BODY_SIZE: f32 = 36.0;
const LINE_HEIGHT: f32 = 1.6;
const PARA_GAP: f32 = BODY_SIZE * 0.5;
/// Pictures are scaled to the text width but never blown up more than this.
const MAX_IMAGE_UPSCALE: f32 = 2.0;
const PAPER: Rgb<u8> = Rgb([0xFF, 0xFF, 0xFF]);
const INK: Color = Color::rgb(0x1A, 0x1A, 0x1A);

fn content_width() -> f32 {
    PAGE_WIDTH as f32 - 2.0 * MARGIN
}

fn content_height() -> f32 {
    PAGE_HEIGHT as f32 - 2.0 * MARGIN
}

/// Where one piece of a page goes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Piece {
    /// Visual lines `from..to` of text block `block`, whose line `from` starts at `y` on the page.
    Lines {
        block: u32,
        from: u32,
        to: u32,
        y: f32,
    },
    Image {
        block: u32,
        x: f32,
        y: f32,
        w: u32,
        h: u32,
    },
}

/// The result of pagination: what each page holds, and the page each block starts on.
#[derive(Debug, Default)]
pub struct Layout {
    pages: Vec<Vec<Piece>>,
    block_page: Vec<u32>,
}

impl Layout {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page block `block` starts on (the last page for a trailing anchor).
    pub fn page_of_block(&self, block: usize) -> u32 {
        self.block_page
            .get(block)
            .copied()
            .unwrap_or_else(|| self.pages.len().saturating_sub(1) as u32)
    }
}

pub struct Typesetter {
    fonts: FontSystem,
    glyphs: SwashCache,
    families: Families,
}

/// Directories scanned when the system font lookup finds nothing (slim images often ship fonts
/// without `/etc/fonts/fonts.conf`), plus `LANRURUGI_FONT_DIRS` (colon-separated).
const FONT_DIRS: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/System/Library/Fonts",
    "/Library/Fonts",
];

impl Typesetter {
    /// Loads the font database; created once per worker, on the first text book. `None` when the
    /// system has no fonts at all — text books then fail with a clear error instead of a panic.
    pub fn new() -> Option<Self> {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_system_fonts();
        if db.is_empty() {
            for dir in FONT_DIRS {
                db.load_fonts_dir(dir);
            }
        }
        if let Some(extra) = std::env::var_os("LANRURUGI_FONT_DIRS") {
            for dir in std::env::split_paths(&extra) {
                db.load_fonts_dir(dir);
            }
        }
        if db.is_empty() {
            return None;
        }
        let installed: HashSet<String> = db
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect();
        Some(Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            glyphs: SwashCache::new(),
            families: Families::resolve(&installed),
        })
    }

    pub fn paginate(
        &mut self,
        book: &FlowBook,
        mut image_size: impl FnMut(ImageRef) -> Option<(u32, u32)>,
    ) -> Layout {
        let lang = book.language();
        let (width, height) = (content_width(), content_height());
        let mut layout = Layout {
            pages: vec![Vec::new()],
            block_page: Vec::with_capacity(book.blocks.len()),
        };
        let mut y = 0.0f32;

        fn new_page(layout: &mut Layout, y: &mut f32) {
            if !layout.pages.last().is_some_and(Vec::is_empty) {
                layout.pages.push(Vec::new());
            }
            *y = 0.0;
        }

        for (index, block) in book.blocks.iter().enumerate() {
            let block_id = index as u32;
            match block {
                Block::PageBreak => {
                    new_page(&mut layout, &mut y);
                    layout.block_page.push((layout.pages.len() - 1) as u32);
                }
                Block::Image(image) => {
                    let Some((w, h)) = image_size(*image).filter(|&(w, h)| w > 0 && h > 0) else {
                        layout.block_page.push((layout.pages.len() - 1) as u32);
                        continue;
                    };
                    let mut scale = (width / w as f32).min(MAX_IMAGE_UPSCALE);
                    if h as f32 * scale > height {
                        scale = height / h as f32;
                    }
                    let (dw, dh) = (w as f32 * scale, h as f32 * scale);
                    if y > 0.0 && y + dh > height {
                        new_page(&mut layout, &mut y);
                    }
                    layout.block_page.push((layout.pages.len() - 1) as u32);
                    layout
                        .pages
                        .last_mut()
                        .expect("at least one page")
                        .push(Piece::Image {
                            block: block_id,
                            x: MARGIN + (width - dw) / 2.0,
                            y: MARGIN + y,
                            w: dw.round().max(1.0) as u32,
                            h: dh.round().max(1.0) as u32,
                        });
                    y += dh + PARA_GAP;
                }
                Block::Heading { text, .. } | Block::Para { text } => {
                    let (metrics, gap_before) = style(block);
                    let buffer = self.shape(text, metrics, block, lang, width);
                    let lines: Vec<(f32, f32)> = buffer
                        .layout_runs()
                        .map(|run| (run.line_top, run.line_height))
                        .collect();
                    drop(buffer);
                    if lines.is_empty() {
                        layout.block_page.push((layout.pages.len() - 1) as u32);
                        continue;
                    }
                    if y > 0.0 {
                        y += gap_before;
                        // A heading keeps at least one body line under it on the same page.
                        let needed = if matches!(block, Block::Heading { .. }) {
                            lines[0].1 + BODY_SIZE * LINE_HEIGHT
                        } else {
                            lines[0].1
                        };
                        if y + needed > height {
                            new_page(&mut layout, &mut y);
                        }
                    }
                    layout.block_page.push((layout.pages.len() - 1) as u32);

                    let mut from = 0usize;
                    while from < lines.len() {
                        let start_top = lines[from].0;
                        let bottom = |i: usize| lines[i].0 + lines[i].1 - start_top;
                        let mut to = from;
                        while to < lines.len() && y + bottom(to) <= height + 0.5 {
                            to += 1;
                        }
                        if to == from {
                            if y > 0.0 {
                                new_page(&mut layout, &mut y);
                                continue;
                            }
                            to = from + 1; // a single line taller than the page
                        }
                        layout
                            .pages
                            .last_mut()
                            .expect("at least one page")
                            .push(Piece::Lines {
                                block: block_id,
                                from: from as u32,
                                to: to as u32,
                                y: MARGIN + y,
                            });
                        y += bottom(to - 1);
                        from = to;
                        if from < lines.len() {
                            new_page(&mut layout, &mut y);
                        }
                    }
                    y += PARA_GAP;
                }
            }
        }
        if layout.pages.len() > 1 && layout.pages.last().is_some_and(Vec::is_empty) {
            layout.pages.pop();
            let last = (layout.pages.len() - 1) as u32;
            for page in &mut layout.block_page {
                *page = (*page).min(last);
            }
        }
        layout
    }

    /// Rasterises page `page` of `book`. `image_bytes` fetches a picture's stored bytes.
    pub fn render(
        &mut self,
        book: &FlowBook,
        layout: &Layout,
        page: usize,
        mut image_bytes: impl FnMut(ImageRef) -> Option<Vec<u8>>,
    ) -> Option<RgbImage> {
        let pieces = layout.pages.get(page)?;
        let lang = book.language();
        let mut canvas = RgbImage::from_pixel(PAGE_WIDTH, PAGE_HEIGHT, PAPER);
        for piece in pieces {
            match *piece {
                Piece::Image { block, x, y, w, h } => {
                    let Some(Block::Image(image)) = book.blocks.get(block as usize) else {
                        continue;
                    };
                    let Some(picture) =
                        image_bytes(*image).and_then(|bytes| image::load_from_memory(&bytes).ok())
                    else {
                        continue;
                    };
                    let scaled =
                        imageops::resize(&picture.to_rgb8(), w, h, imageops::FilterType::Triangle);
                    imageops::overlay(&mut canvas, &scaled, x.round() as i64, y.round() as i64);
                }
                Piece::Lines { block, from, to, y } => {
                    let Some(block_ref) = book.blocks.get(block as usize) else {
                        continue;
                    };
                    let (Block::Heading { text, .. } | Block::Para { text }) = block_ref else {
                        continue;
                    };
                    let (metrics, _) = style(block_ref);
                    let buffer = self.shape(text, metrics, block_ref, lang, content_width());
                    let mut first_top = None;
                    for (i, run) in buffer.layout_runs().enumerate() {
                        if i < from as usize || i >= to as usize {
                            continue;
                        }
                        let top = *first_top.get_or_insert(run.line_top);
                        let dy = y - top + run.line_y;
                        for glyph in run.glyphs {
                            let physical = glyph.physical((MARGIN, dy), 1.0);
                            self.glyphs.with_pixels(
                                &mut self.fonts,
                                physical.cache_key,
                                INK,
                                |px, py, color| {
                                    blend(&mut canvas, physical.x + px, physical.y + py, color);
                                },
                            );
                        }
                    }
                }
            }
        }
        Some(canvas)
    }

    fn shape(
        &mut self,
        text: &str,
        metrics: Metrics,
        block: &Block,
        lang: Lang,
        width: f32,
    ) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_wrap(&mut self.fonts, Wrap::WordOrGlyph);
        buffer.set_size(&mut self.fonts, Some(width), None);
        let heading = matches!(block, Block::Heading { .. });
        let attrs = Attrs::new()
            .family(Family::Name(self.families.get(lang, heading)))
            .weight(if heading {
                Weight::BOLD
            } else {
                Weight::NORMAL
            });
        buffer.set_text(&mut self.fonts, text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }
}

/// Text size and the space above a block.
fn style(block: &Block) -> (Metrics, f32) {
    let size = match block {
        Block::Heading { level: 1, .. } => BODY_SIZE * 1.5,
        Block::Heading { level: 2, .. } => BODY_SIZE * 1.3,
        Block::Heading { .. } => BODY_SIZE * 1.15,
        _ => BODY_SIZE,
    };
    let gap = if matches!(block, Block::Heading { .. }) {
        BODY_SIZE
    } else {
        0.0
    };
    (Metrics::new(size, size * LINE_HEIGHT), gap)
}

/// Candidate families per language, body (serif) then heading (sans), best first: Noto CJK (Linux
/// distributions), the per-language Noto packages (FreeBSD and others), Source Han, then the CJK
/// fonts macOS ships. CJK fonts carry Latin too, so one family sets mixed text without gaps.
const ZH: [&[&str]; 2] = [
    &[
        "Noto Serif CJK SC",
        "Noto Serif SC",
        "Source Han Serif SC",
        "Source Han Serif CN",
        "Songti SC",
        "STSong",
    ],
    &[
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "Source Han Sans SC",
        "Source Han Sans CN",
        "PingFang SC",
        "Hiragino Sans GB",
    ],
];
const JA: [&[&str]; 2] = [
    &[
        "Noto Serif CJK JP",
        "Noto Serif JP",
        "Source Han Serif JP",
        "Source Han Serif",
        "Hiragino Mincho ProN",
    ],
    &[
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "Source Han Sans JP",
        "Source Han Sans",
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
    ],
];
const KO: [&[&str]; 2] = [
    &[
        "Noto Serif CJK KR",
        "Noto Serif KR",
        "Source Han Serif KR",
        "Source Han Serif K",
        "AppleMyungjo",
    ],
    &[
        "Noto Sans CJK KR",
        "Noto Sans KR",
        "Source Han Sans KR",
        "Source Han Sans K",
        "Apple SD Gothic Neo",
    ],
];

/// The family used for each language and role, chosen once from what is installed.
struct Families {
    zh: [&'static str; 2],
    ja: [&'static str; 2],
    ko: [&'static str; 2],
}

impl Families {
    fn resolve(installed: &HashSet<String>) -> Self {
        let pick = |set: [&'static [&'static str]; 2]| set.map(|c| first_installed(c, installed));
        Self {
            zh: pick(ZH),
            ja: pick(JA),
            ko: pick(KO),
        }
    }

    fn get(&self, lang: Lang, heading: bool) -> &'static str {
        let set = match lang {
            Lang::Ja => &self.ja,
            Lang::Ko => &self.ko,
            Lang::Zh | Lang::Latin => &self.zh,
        };
        set[usize::from(heading)]
    }
}

/// The first candidate that is installed, else the first one (cosmic-text then falls back per
/// glyph to whatever face covers it).
fn first_installed(
    candidates: &'static [&'static str],
    installed: &HashSet<String>,
) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|name| installed.contains(*name))
        .unwrap_or(candidates[0])
}

fn blend(canvas: &mut RgbImage, x: i32, y: i32, color: Color) {
    if x < 0 || y < 0 || x >= canvas.width() as i32 || y >= canvas.height() as i32 {
        return;
    }
    let alpha = u32::from(color.a());
    if alpha == 0 {
        return;
    }
    let pixel = canvas.get_pixel_mut(x as u32, y as u32);
    let src = [color.r(), color.g(), color.b()];
    for (dst, src) in pixel.0.iter_mut().zip(src) {
        *dst = ((u32::from(src) * alpha + u32::from(*dst) * (255 - alpha)) / 255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_families_fall_back_to_what_is_installed() {
        let set = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<HashSet<_>>();
        // A Linux distribution with Noto CJK.
        let linux = Families::resolve(&set(&[
            "Noto Serif CJK SC",
            "Noto Sans CJK JP",
            "DejaVu Sans",
        ]));
        assert_eq!(linux.get(Lang::Zh, false), "Noto Serif CJK SC");
        assert_eq!(linux.get(Lang::Ja, true), "Noto Sans CJK JP");
        // FreeBSD's per-language Noto packages.
        let bsd = Families::resolve(&set(&["Noto Serif SC", "Noto Sans JP"]));
        assert_eq!(bsd.get(Lang::Latin, false), "Noto Serif SC");
        assert_eq!(bsd.get(Lang::Ja, true), "Noto Sans JP");
        // macOS with only its own fonts.
        let mac = Families::resolve(&set(&[
            "Songti SC",
            "PingFang SC",
            "Hiragino Mincho ProN",
            "Apple SD Gothic Neo",
        ]));
        assert_eq!(mac.get(Lang::Zh, false), "Songti SC");
        assert_eq!(mac.get(Lang::Zh, true), "PingFang SC");
        assert_eq!(mac.get(Lang::Ja, false), "Hiragino Mincho ProN");
        assert_eq!(mac.get(Lang::Ko, true), "Apple SD Gothic Neo");
        // Nothing CJK installed: the first candidate, left to per-glyph fallback.
        assert_eq!(
            Families::resolve(&set(&[])).get(Lang::Ko, false),
            "Noto Serif CJK KR"
        );
    }

    fn paragraphs(n: usize, text: &str) -> FlowBook {
        let mut book = FlowBook::default();
        book.anchor_here("第一章", 0);
        book.push(Block::Heading {
            level: 1,
            text: "第一章　春天".into(),
        });
        for _ in 0..n {
            book.push(Block::Para { text: text.into() });
        }
        book
    }

    #[test]
    fn long_text_spans_pages_and_short_text_does_not() {
        let mut ts = Typesetter::new().expect("fonts installed");
        let short = paragraphs(1, "春天来了。Spring is here.");
        assert_eq!(ts.paginate(&short, |_| None).page_count(), 1);
        let long = paragraphs(
            200,
            "春天来了，小明在公园里散步，看见樱花开得很漂亮。Mixed text 123 で混在。",
        );
        let layout = ts.paginate(&long, |_| None);
        assert!(layout.page_count() > 5, "{}", layout.page_count());
        assert_eq!(layout.page_of_block(0), 0);
        // Every page renders and has ink on it.
        for page in [0, layout.page_count() - 1] {
            let img = ts.render(&long, &layout, page, |_| None).unwrap();
            assert!(img.pixels().any(|p| p.0[0] < 128), "page {page} is blank");
        }
    }

    #[test]
    fn images_are_scaled_and_placed() {
        let mut book = FlowBook::default();
        book.push(Block::Para {
            text: "before".into(),
        });
        book.push(Block::Image(ImageRef(0)));
        book.push(Block::PageBreak);
        book.push(Block::Para {
            text: "after".into(),
        });
        let mut ts = Typesetter::new().expect("fonts installed");
        let layout = ts.paginate(&book, |_| Some((4000, 8000)));
        // The tall picture does not fit under the first paragraph, so it moves to page 2; the
        // page break then starts page 3.
        assert_eq!(layout.page_count(), 3);
        assert_eq!(layout.page_of_block(1), 1);
        assert_eq!(layout.page_of_block(3), 2);
    }
}
