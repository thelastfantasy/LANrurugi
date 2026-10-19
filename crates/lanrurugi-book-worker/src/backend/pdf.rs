//! PDF through PDFium. A page that is exactly one upright full-page JPEG (a scanned book) is served
//! as that JPEG's stored bytes; every other page is rendered.
//!
//! `LANRURUGI_PDFIUM_LIB` names the PDFium shared library (a file, or a directory holding
//! `libpdfium.so`); without it the system library search path is used.

use std::path::Path;

use lanrurugi_book::protocol::{BookInfo, ImageKind, OutlineItem, PageSource};
use lanrurugi_book::BookFormat;
use pdfium_render::prelude::*;

use super::{encode_webp, Failure, Result, Stamp};

/// Rendered pages taller than this many times their width (webtoon strips) are capped by height.
const MAX_ASPECT: f32 = 8.0;
/// A full-page image must cover at least this share of the page to be passed through as-is.
const FULL_PAGE_COVERAGE: f32 = 0.97;
/// Guards against cyclic or absurd outlines.
const OUTLINE_MAX_DEPTH: u32 = 6;
const OUTLINE_MAX_ITEMS: usize = 5_000;

impl From<PdfiumError> for Failure {
    fn from(e: PdfiumError) -> Self {
        match e {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                Failure::encrypted("password required")
            }
            PdfiumError::PdfiumLibraryInternalError(
                PdfiumInternalError::FormatError | PdfiumInternalError::FileError,
            ) => Failure::unreadable(format!("{e:?}")),
            other => Failure::internal(format!("{other:?}")),
        }
    }
}

/// PDFium, bound once per worker, plus the most recently opened document: a reader paging through
/// one book sends many requests for the same file.
pub struct Pdf {
    pdfium: &'static Pdfium,
    open: Option<(String, Stamp, PdfDocument<'static>)>,
}

impl Pdf {
    pub fn new() -> Result<Self> {
        let bindings = match std::env::var_os("LANRURUGI_PDFIUM_LIB") {
            Some(lib) => {
                let lib = Path::new(&lib);
                if lib.is_dir() {
                    Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(lib))
                } else {
                    Pdfium::bind_to_library(lib)
                }
            }
            None => Pdfium::bind_to_system_library(),
        }
        .map_err(|e| {
            Failure::unreadable(format!(
                "PDF support is not available on this system: the PDFium library could not be \
                 loaded ({e:?}); install it or set LANRURUGI_PDFIUM_LIB"
            ))
        })?;
        // Leaked once per worker process: documents borrow the library for their whole life.
        Ok(Self {
            pdfium: Box::leak(Box::new(Pdfium::new(bindings))),
            open: None,
        })
    }

    /// Dropped after a panic: the document may be half-way through a call.
    pub fn forget_document(&mut self) {
        self.open = None;
    }

    fn document(&mut self, path: &str) -> Result<&PdfDocument<'static>> {
        let stamp = Stamp::of(path)?;
        if !matches!(&self.open, Some((p, s, _)) if p == path && *s == stamp) {
            self.open = None;
            let document = self.pdfium.load_pdf_from_file(Path::new(path), None)?;
            self.open = Some((path.to_owned(), stamp, document));
        }
        Ok(&self.open.as_ref().expect("just opened").2)
    }

    pub fn inspect(&mut self, path: &str, width: u32) -> Result<BookInfo> {
        let document = self.document(path)?;
        let mut pages = Vec::new();
        let mut sizes = Vec::new();
        for page in document.pages().iter() {
            match sole_full_page_jpeg(&page)
                .and_then(|image| Some((image.width().ok()? as u32, image.height().ok()? as u32)))
            {
                Some(size) => {
                    pages.push(PageSource::Image(ImageKind::Jpeg));
                    sizes.push(size);
                }
                None => {
                    pages.push(PageSource::Render);
                    sizes.push(rendered_size(&page, width));
                }
            }
        }

        let mut outline = Vec::new();
        collect_outline(document.bookmarks().root(), 0, &mut outline);

        let metadata = document.metadata();
        let tag = |t| {
            metadata
                .get(t)
                .map(|tag| tag.value().trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Ok(BookInfo {
            format: BookFormat::Pdf,
            pages,
            sizes,
            outline,
            title: tag(PdfDocumentMetadataTagType::Title),
            author: tag(PdfDocumentMetadataTagType::Author),
        })
    }

    pub fn page(
        &mut self,
        path: &str,
        index: u32,
        source: PageSource,
        width: u32,
    ) -> Result<(String, Vec<u8>)> {
        let document = self.document(path)?;
        let page = document
            .pages()
            .get(index as i32)
            .map_err(|_| Failure::out_of_range(index))?;
        if let PageSource::Image(_) = source {
            if let Some(bytes) = sole_full_page_jpeg(&page)
                .and_then(|image| image.get_raw_image_data().ok())
                .filter(|bytes| bytes.starts_with(&[0xFF, 0xD8]))
            {
                return Ok(("jpg".into(), bytes));
            }
            // The page no longer looks like it did at inspection time; the client named it
            // `.jpg`, so encode the render as JPEG rather than return mismatched bytes.
            let mut out = Vec::new();
            render(&page, width)?
                .to_rgb8()
                .write_to(
                    &mut std::io::Cursor::new(&mut out),
                    image::ImageFormat::Jpeg,
                )
                .map_err(|e| Failure::internal(e.to_string()))?;
            return Ok(("jpg".into(), out));
        }
        Ok((
            "webp".into(),
            encode_webp(&render(&page, width)?.to_rgb8())?,
        ))
    }
}

fn render(page: &PdfPage, width: u32) -> Result<image::DynamicImage> {
    let config = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_maximum_height((width as f32 * MAX_ASPECT) as i32)
        .render_form_data(true);
    Ok(page.render_with_config(&config)?.as_image()?)
}

fn rendered_size(page: &PdfPage, width: u32) -> (u32, u32) {
    let (w, h) = (page.width().value.max(1.0), page.height().value.max(1.0));
    let height = (width as f32 * h / w).min(width as f32 * MAX_ASPECT);
    let width = if height < width as f32 * h / w {
        height * w / h
    } else {
        width as f32
    };
    (width.round() as u32, height.round() as u32)
}

/// The page's image if it is exactly one upright, RGB/gray JPEG filling the page, with nothing
/// visible drawn over it (invisible text — a scan's OCR layer — is allowed).
fn sole_full_page_jpeg<'p>(page: &'p PdfPage) -> Option<PdfPageImageObject<'p>> {
    if !matches!(page.rotation(), Ok(PdfPageRenderRotation::None)) {
        return None;
    }
    let mut found = None;
    for object in page.objects().iter() {
        match object {
            PdfPageObject::Image(image) if found.is_none() => found = Some(image),
            PdfPageObject::Text(text) if text.render_mode() == PdfPageTextRenderMode::Invisible => {
            }
            _ => return None,
        }
    }
    let image = found?;

    let filters: Vec<String> = image
        .filters()
        .iter()
        .map(|f| f.name().to_owned())
        .collect();
    if filters != ["DCTDecode"] {
        return None;
    }
    if !matches!(
        image.color_space(),
        Ok(PdfColorSpace::DeviceRGB | PdfColorSpace::DeviceGray)
    ) {
        return None;
    }
    let matrix = image.matrix().ok()?;
    if matrix.a() <= 0.0 || matrix.d() <= 0.0 || matrix.b().abs() > 1e-3 || matrix.c().abs() > 1e-3
    {
        return None;
    }
    let bounds = image.bounds().ok()?;
    let page_area = page.width().value * page.height().value;
    let covered = bounds.width().value * bounds.height().value;
    (page_area > 0.0 && covered / page_area >= FULL_PAGE_COVERAGE).then_some(image)
}

fn collect_outline(first: Option<PdfBookmark>, depth: u32, out: &mut Vec<OutlineItem>) {
    let mut current = first;
    while let Some(bookmark) = current {
        if out.len() >= OUTLINE_MAX_ITEMS {
            return;
        }
        let page = bookmark
            .destination()
            .and_then(|d| d.page_index().ok())
            .or_else(|| {
                let action = bookmark.action()?;
                let local = action.as_local_destination_action()?;
                local.destination().ok()?.page_index().ok()
            });
        let title = bookmark
            .title()
            .map(|t| t.trim().to_owned())
            .unwrap_or_default();
        if let (Some(page), false) = (page, title.is_empty()) {
            out.push(OutlineItem {
                title,
                page: page as u32,
                depth,
            });
        }
        if depth < OUTLINE_MAX_DEPTH {
            collect_outline(bookmark.first_child(), depth + 1, out);
        }
        current = bookmark.next_sibling();
    }
}
