//! Shared setup and fixture builders for the book worker's end-to-end tests. Fixtures are built
//! in code so every byte of every format under test is visible here.
#![allow(dead_code)]

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lanrurugi_book::BookConfig;

/// Configures the client once per test binary: the worker built for this test run, PDFium if
/// available, and a cache directory that lives as long as the process.
pub fn setup() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        lanrurugi_book::configure(BookConfig {
            worker_bin: Some(env!("CARGO_BIN_EXE_lanrurugi-book-worker").into()),
            pdfium_lib: pdfium_lib(),
            djvu_bin_dir: None,
            cache_dir: dir.path().join("cache"),
            max_workers: 2,
            render_width: 600,
        });
        dir
    });
    dir.path()
}

/// A fresh file path inside the shared test directory.
pub fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = setup().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

pub fn pdfium_lib() -> Option<PathBuf> {
    if let Some(lib) = std::env::var_os("LANRURUGI_PDFIUM_LIB") {
        return Some(lib.into());
    }
    // Matches the directories `mise run fetch-pdfium` writes.
    let (platform, lib) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => ("linux-x64", "libpdfium.so"),
        ("linux", "aarch64") => ("linux-arm64", "libpdfium.so"),
        ("macos", "aarch64") => ("mac-arm64", "libpdfium.dylib"),
        _ => return None,
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../.pdfium/{platform}/lib"));
    dir.join(lib).is_file().then_some(dir)
}

pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .unwrap();
    out
}

/// UTF-16BE hex string with BOM, how PDF stores non-Latin outline titles.
pub fn utf16_hex(s: &str) -> String {
    let mut hex = String::from("<FEFF");
    for unit in s.encode_utf16() {
        hex.push_str(&format!("{unit:04X}"));
    }
    hex.push('>');
    hex
}

/// A three-page PDF: (1) a full-page JPEG, (2) text only, (3) the JPEG with visible text over it;
/// outline: "Chapter 1" → p1 with child "Section 1.1" → p2, then "第2話" → p3.
pub fn fixture_pdf(jpeg: &[u8], w: u32, h: u32) -> Vec<u8> {
    let page_image = format!("q {w} 0 0 {h} 0 0 cm /Im1 Do Q");
    let page_text = "BT /F1 24 Tf 72 720 Td (Hello PDF) Tj ET".to_string();
    let page_mixed = format!("{page_image} BT /F1 24 Tf 20 20 Td (Note) Tj ET");
    let stream = |body: &[u8], dict: &str| {
        let mut out = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendstream");
        out
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 9 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im1 6 0 R >> >> /Contents 7 0 R >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 8 0 R >> >> /Contents 10 0 R >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im1 6 0 R >> /Font << /F1 8 0 R >> >> /Contents 11 0 R >>").into_bytes(),
        stream(jpeg, &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode")),
        stream(page_image.as_bytes(), ""),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Outlines /First 12 0 R /Last 14 0 R /Count 2 >>".to_vec(),
        stream(page_text.as_bytes(), ""),
        stream(page_mixed.as_bytes(), ""),
        b"<< /Title (Chapter 1) /Parent 9 0 R /Next 14 0 R /First 13 0 R /Last 13 0 R /Count 1 /Dest [3 0 R /Fit] >>".to_vec(),
        b"<< /Title (Section 1.1) /Parent 12 0 R /Dest [4 0 R /Fit] >>".to_vec(),
        format!("<< /Title {} /Parent 9 0 R /Prev 12 0 R /Dest [5 0 R /Fit] >>", utf16_hex("第2話")).into_bytes(),
    ];

    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

pub fn png(width: u32, height: u32) -> Vec<u8> {
    let image =
        image::RgbImage::from_fn(width, height, |x, _| image::Rgb([200, (x % 256) as u8, 40]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// An EPUB: `mimetype` first and stored (OCF), a container pointing at `OEBPS/content.opf`, then
/// `files` as given (paths relative to the zip root).
pub fn epub(opf: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    zip.start_file("META-INF/container.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
    zip.start_file("OEBPS/content.opf", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(opf.as_bytes()).unwrap();
    for (name, bytes) in files {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A MOBI 6 file: uncompressed UTF-8 text records holding `html`, then `images` as records, with
/// the given EXTH entries and record-0 encryption type.
pub fn mobi(html: &str, images: &[Vec<u8>], exth: &[(u32, &[u8])], encryption: u16) -> Vec<u8> {
    let text = html.as_bytes();
    let text_records: Vec<&[u8]> = text.chunks(4096).collect();
    let first_image = 1 + text_records.len();

    let mut rec0 = vec![0u8; 16 + 232];
    put16(&mut rec0, 0, 1); // no compression
    put32(&mut rec0, 4, text.len() as u32);
    put16(&mut rec0, 8, text_records.len() as u16);
    put16(&mut rec0, 10, 4096);
    put16(&mut rec0, 12, encryption);
    rec0[16..20].copy_from_slice(b"MOBI");
    put32(&mut rec0, 20, 232);
    put32(&mut rec0, 24, 2);
    put32(&mut rec0, 28, 65001);
    put32(&mut rec0, 36, 6);
    put32(&mut rec0, 108, first_image as u32);
    if !exth.is_empty() {
        put32(&mut rec0, 128, 0x40);
        let mut body = Vec::new();
        for (kind, data) in exth {
            body.extend_from_slice(&kind.to_be_bytes());
            body.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
        }
        rec0.extend_from_slice(b"EXTH");
        rec0.extend_from_slice(&(12 + body.len() as u32).to_be_bytes());
        rec0.extend_from_slice(&(exth.len() as u32).to_be_bytes());
        rec0.extend_from_slice(&body);
    }
    let name = b"Test Book";
    let name_at = rec0.len();
    rec0.extend_from_slice(name);
    put32(&mut rec0, 84, name_at as u32);
    put32(&mut rec0, 88, name.len() as u32);

    let mut records: Vec<Vec<u8>> = vec![rec0];
    records.extend(text_records.iter().map(|r| r.to_vec()));
    records.extend(images.iter().cloned());

    let mut out = vec![0u8; 78];
    out[..4].copy_from_slice(b"test");
    out[60..68].copy_from_slice(b"BOOKMOBI");
    put16(&mut out, 76, records.len() as u16);
    let mut offset = 78 + 8 * records.len() + 2;
    for (i, record) in records.iter().enumerate() {
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, (i * 2) as u8]);
        offset += record.len();
    }
    out.extend_from_slice(&[0, 0]);
    for record in &records {
        out.extend_from_slice(record);
    }
    out
}

/// Decodes a served page and reports whether it has any dark ink on it.
pub fn has_ink(bytes: &[u8]) -> bool {
    image::load_from_memory(bytes)
        .unwrap()
        .to_luma8()
        .pixels()
        .any(|p| p.0[0] < 100)
}
