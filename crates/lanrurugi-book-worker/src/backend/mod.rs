//! One backend per format behind a single dispatcher. PDF and DjVu have their own engines;
//! EPUB, MOBI, FB2 and TXT are *containers*: they either hold one picture per page or a text flow
//! that the shared [`Typesetter`] lays out, and everything above "give me picture N" / "give me
//! the flow" is written once here.

pub mod djvu;
pub mod epub;
pub mod fb2;
pub mod mobi;
pub mod pdf;
pub mod txt;

use std::fmt::Display;
use std::io::Cursor;
use std::time::SystemTime;

use image::RgbImage;
use lanrurugi_book::protocol::{BookInfo, ErrorKind, ImageKind, OutlineItem, PageSource};
use lanrurugi_book::BookFormat;

use crate::flow::typeset::{Layout, Typesetter, PAGE_HEIGHT, PAGE_WIDTH};
use crate::flow::{FlowBook, ImageRef};

const WEBP_QUALITY: f32 = 88.0;

#[derive(Debug)]
pub struct Failure {
    pub kind: ErrorKind,
    pub message: String,
}

impl Failure {
    pub fn unreadable(message: impl Display) -> Self {
        Self {
            kind: ErrorKind::Unreadable,
            message: message.to_string(),
        }
    }

    pub fn encrypted(message: impl Display) -> Self {
        Self {
            kind: ErrorKind::Encrypted,
            message: message.to_string(),
        }
    }

    pub fn internal(message: impl Display) -> Self {
        Self {
            kind: ErrorKind::Internal,
            message: message.to_string(),
        }
    }

    pub fn out_of_range(index: u32) -> Self {
        Self {
            kind: ErrorKind::PageOutOfRange,
            message: format!("page {index}"),
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::unreadable(e)
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

/// A file's identity for "is the open copy still current": size and mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp(u64, Option<SystemTime>);

impl Stamp {
    pub fn of(path: &str) -> Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self(meta.len(), meta.modified().ok()))
    }
}

/// What a container book's pages are.
pub enum Pages {
    /// One stored picture per page; the outline is already in page indices.
    Images {
        pages: Vec<ImageRef>,
        outline: Vec<OutlineItem>,
    },
    /// Text to typeset; the outline is anchored at blocks.
    Flow(FlowBook),
}

/// A book format that is a container of pictures and/or text. Opening one yields the container
/// (which reads pictures on demand) and its [`Pages`] (owned separately, so a text book's flow can
/// be read while pictures are fetched — no copies).
pub trait Container {
    /// The stored bytes of a picture.
    fn image(&mut self, image: ImageRef) -> Result<Vec<u8>>;
    /// Enough leading bytes of a picture to read its format and size; the whole picture unless a
    /// backend can do better.
    fn image_head(&mut self, image: ImageRef) -> Result<Vec<u8>> {
        self.image(image)
    }
    fn title(&self) -> Option<String>;
    fn author(&self) -> Option<String>;
}

fn boxed<C: Container + 'static>((container, pages): (C, Pages)) -> (Box<dyn Container>, Pages) {
    (Box::new(container), pages)
}

fn open_container(path: &str, format: BookFormat) -> Result<(Box<dyn Container>, Pages)> {
    Ok(match format {
        BookFormat::Epub => boxed(epub::Epub::open(path)?),
        BookFormat::Mobi => boxed(mobi::Mobi::open(path)?),
        BookFormat::Fb2 => boxed(fb2::Fb2::open(path)?),
        BookFormat::Txt => boxed(txt::Txt::open(path)?),
        BookFormat::Pdf | BookFormat::Djvu => unreachable!("not a container format"),
    })
}

/// The container book currently open, with its page layout once a text book has been typeset.
struct OpenBook {
    path: String,
    stamp: Stamp,
    book: Box<dyn Container>,
    pages: Pages,
    layout: Option<Layout>,
}

/// Everything one worker keeps between requests. Engines are created on first use, so a worker
/// that only ever sees EPUBs never loads PDFium or the font database.
#[derive(Default)]
pub struct Worker {
    pdf: Option<pdf::Pdf>,
    open: Option<OpenBook>,
    typesetter: Option<Typesetter>,
}

impl Worker {
    /// Called after a panic: nothing open can be trusted any more.
    pub fn reset(&mut self) {
        self.open = None;
        if let Some(pdf) = &mut self.pdf {
            pdf.forget_document();
        }
    }

    fn typesetter(slot: &mut Option<Typesetter>) -> Result<&mut Typesetter> {
        if slot.is_none() {
            *slot =
                Some(Typesetter::new().ok_or_else(|| {
                    Failure::internal("no fonts installed to typeset text books")
                })?);
        }
        Ok(slot.as_mut().expect("just created"))
    }

    fn pdf(&mut self) -> Result<&mut pdf::Pdf> {
        if self.pdf.is_none() {
            self.pdf = Some(pdf::Pdf::new()?);
        }
        Ok(self.pdf.as_mut().expect("just created"))
    }

    pub fn inspect(&mut self, path: &str, format: BookFormat, width: u32) -> Result<BookInfo> {
        match format {
            BookFormat::Pdf => self.pdf()?.inspect(path, width),
            BookFormat::Djvu => djvu::inspect(path, width),
            _ => self.inspect_container(path, format),
        }
    }

    pub fn page(
        &mut self,
        path: &str,
        format: BookFormat,
        index: u32,
        source: PageSource,
        width: u32,
    ) -> Result<(String, Vec<u8>)> {
        match format {
            BookFormat::Pdf => self.pdf()?.page(path, index, source, width),
            BookFormat::Djvu => djvu::page(path, index, width),
            _ => self.container_page(path, format, index, source),
        }
    }

    fn open(&mut self, path: &str, format: BookFormat) -> Result<&mut OpenBook> {
        let stamp = Stamp::of(path)?;
        if !matches!(&self.open, Some(o) if o.path == path && o.stamp == stamp) {
            self.open = None;
            let (book, pages) = open_container(path, format)?;
            self.open = Some(OpenBook {
                path: path.to_owned(),
                stamp,
                book,
                pages,
                layout: None,
            });
        }
        Ok(self.open.as_mut().expect("just opened"))
    }

    fn inspect_container(&mut self, path: &str, format: BookFormat) -> Result<BookInfo> {
        self.open(path, format)?;
        let Worker {
            open, typesetter, ..
        } = self;
        let open = open.as_mut().expect("opened above");
        let (pages, sizes, outline) = if matches!(open.pages, Pages::Flow(_)) {
            layout_of(open, typesetter)?;
            let (Pages::Flow(flow), Some(layout)) = (&open.pages, &open.layout) else {
                unreachable!()
            };
            let outline = flow
                .anchors
                .iter()
                .map(|a| OutlineItem {
                    title: a.title.clone(),
                    page: layout.page_of_block(a.block),
                    depth: a.depth,
                })
                .collect();
            let count = layout.page_count();
            (
                vec![PageSource::Render; count],
                vec![(PAGE_WIDTH, PAGE_HEIGHT); count],
                outline,
            )
        } else {
            let Pages::Images { pages, outline } = &open.pages else {
                unreachable!()
            };
            let mut sources = Vec::with_capacity(pages.len());
            let mut sizes = Vec::with_capacity(pages.len());
            for &image in pages {
                let head = open.book.image_head(image)?;
                let kind = ImageKind::sniff(&head);
                sources.push(kind.map_or(PageSource::Render, PageSource::Image));
                sizes.push(image_dimensions(&head).unwrap_or((PAGE_WIDTH, PAGE_HEIGHT)));
            }
            (sources, sizes, outline.clone())
        };
        if pages.is_empty() {
            return Err(Failure::unreadable("the book has no pages"));
        }
        Ok(BookInfo {
            format,
            pages,
            sizes,
            outline,
            title: open.book.title(),
            author: open.book.author(),
        })
    }

    fn container_page(
        &mut self,
        path: &str,
        format: BookFormat,
        index: u32,
        source: PageSource,
    ) -> Result<(String, Vec<u8>)> {
        self.open(path, format)?;
        let Worker {
            open, typesetter, ..
        } = self;
        let open = open.as_mut().expect("opened above");
        if let Pages::Images { pages, .. } = &open.pages {
            let image = *pages
                .get(index as usize)
                .ok_or(Failure::out_of_range(index))?;
            let bytes = open.book.image(image)?;
            return match source {
                PageSource::Image(kind) => Ok((kind.extension().into(), bytes)),
                // A picture in a format browsers may not show (TIFF, SVG fallbacks): convert.
                PageSource::Render => {
                    let picture = image::load_from_memory(&bytes).map_err(Failure::unreadable)?;
                    Ok(("webp".into(), encode_webp(&picture.to_rgb8())?))
                }
            };
        }
        layout_of(open, typesetter)?;
        let OpenBook {
            book,
            pages,
            layout,
            ..
        } = open;
        let layout = layout.as_ref().expect("laid out above");
        let Pages::Flow(flow) = pages else {
            unreachable!()
        };
        let typesetter = Worker::typesetter(typesetter)?;
        let canvas = typesetter
            .render(flow, layout, index as usize, |image| book.image(image).ok())
            .ok_or(Failure::out_of_range(index))?;
        Ok(("webp".into(), encode_webp(&canvas)?))
    }
}

/// The open text book's layout, typesetting it on first use.
fn layout_of<'a>(
    open: &'a mut OpenBook,
    typesetter: &mut Option<Typesetter>,
) -> Result<&'a Layout> {
    if open.layout.is_none() {
        let typesetter = Worker::typesetter(typesetter)?;
        let Pages::Flow(flow) = &open.pages else {
            unreachable!("not a text book")
        };
        let book = &mut open.book;
        let layout = typesetter.paginate(flow, |image| {
            book.image_head(image)
                .ok()
                .and_then(|head| image_dimensions(&head))
        });
        open.layout = Some(layout);
    }
    Ok(open.layout.as_ref().expect("just laid out"))
}

/// Width × height from a picture's header, without decoding its pixels.
pub fn image_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(head))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub fn encode_webp(image: &RgbImage) -> Result<Vec<u8>> {
    webpx::Encoder::new_rgb(image.as_raw(), image.width(), image.height())
        .quality(WEBP_QUALITY)
        .encode(webpx::Unstoppable)
        .map_err(Failure::internal)
}
