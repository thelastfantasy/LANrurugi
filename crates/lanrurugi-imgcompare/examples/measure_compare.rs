//! Measures one `compare_archives` run over two synthetic archives: peak resident memory
//! (`VmHWM`) plus the per-stage timings the library itself logs.
//!
//! The archives are generated into a temp dir and deleted afterwards, so this needs no real
//! library or fixtures. `PAGES` overrides the page count (default 188, the size of a real
//! comparison in the field reports).
//!
//! Run it through `mise run measure-compare`; a release build keeps the timings meaningful.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use image::ImageEncoder as _;

/// A page that is visually distinct per index (so perceptual hashing and alignment have real work
/// to do), drawn with arithmetic only — no fixtures.
fn draw_page(index: usize, width: u32, height: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(width, height);
    let index = index as u32;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let band = ((x / 48) + (y / 48) * 7 + index) % 11;
        let base = (x * 255 / width.max(1) + y * 255 / height.max(1)) / 2;
        let v = (base + band * 19) as u8;
        *px = image::Rgb([v, v.wrapping_add((band as u8).wrapping_mul(13)), 255 - v]);
    }
    img
}

/// The same page inside a white scan border — the crop/resolution difference the precise
/// alignment pass exists for.
fn with_border(img: &image::RgbImage, border: u32) -> image::RgbImage {
    let mut out = image::RgbImage::from_pixel(
        img.width() + border * 2,
        img.height() + border * 2,
        image::Rgb([246, 246, 246]),
    );
    image::imageops::overlay(&mut out, img, border as i64, border as i64);
    out
}

fn encode_jpeg(img: &image::RgbImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .expect("jpeg encode");
    bytes
}

fn write_archive(path: &Path, pages: impl Iterator<Item = (String, Vec<u8>)>) {
    let file = std::fs::File::create(path).expect("create archive");
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in pages {
        zip.start_file(name, options).expect("start entry");
        zip.write_all(&bytes).expect("write entry");
    }
    zip.finish().expect("finish archive");
}

/// Peak resident set size so far, in KiB (`VmHWM` counts the highest RSS ever reached, so it only
/// moves up — reading it after the run is enough).
fn vm_hwm_kib() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .unwrap_or(0)
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    // `mise run measure-compare -- <pages>` (an argument reaches the binary through the container
    // script), or `PAGES=<n>`.
    let pages: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            std::env::var("PAGES")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(188);
    let (width, height) = (1200u32, 1700u32);

    let dir = tempfile::tempdir().expect("temp dir");
    let a_path = dir.path().join("a.cbz");
    let b_path = dir.path().join("b.cbz");

    // One page at a time, dropped immediately: generation must not become the memory peak it is
    // supposed to measure.
    let generated = Instant::now();
    write_archive(
        &a_path,
        (1..=pages).map(|index| {
            (
                format!("p{index:04}.jpg"),
                encode_jpeg(&draw_page(index, width, height)),
            )
        }),
    );
    // B is the same book: identical pages (so alignment actually pairs them and the crop-alignment
    // sample pass runs), except one page that gained a scan border — the case the precise pass
    // exists for — plus two pages A does not have, so unmatched pages are reported too.
    // The first page is always among the crop-alignment samples (`sample_evenly` starts at the
    // first pair), so bordering it guarantees the precise pass runs.
    let bordered = 1;
    write_archive(
        &b_path,
        (1..=pages)
            .map(|index| {
                let page = draw_page(index, width, height);
                let page = if index == bordered {
                    with_border(&page, 120)
                } else {
                    page
                };
                (format!("p{index:04}.jpg"), encode_jpeg(&page))
            })
            .chain((1..=2).map(|index| {
                (
                    format!("x{index:03}.jpg"),
                    encode_jpeg(&draw_page(pages + index, width, height)),
                )
            })),
    );
    let generated_ms = generated.elapsed().as_millis();

    let hwm_before = vm_hwm_kib();
    let compared = Instant::now();
    let mut result = lanrurugi_imgcompare::compare_archives(
        &a_path,
        &b_path,
        "a.cbz".to_string(),
        0,
        "b.cbz".to_string(),
        0,
    )
    .expect("compare");
    let compare_ms = compared.elapsed().as_millis();
    let hwm_after = vm_hwm_kib();

    // The library logs its own stage timings; this is the summary a caller sees.
    let decoded_bytes_per_page = u64::from(width) * u64::from(height) * 3;
    let all_pages_decoded =
        decoded_bytes_per_page * (result.a_total_pages + result.b_total_pages) as u64;
    println!("\n=== compare_archives measurement ===");
    println!(
        "pages:                 {} vs {} ({} aligned pairs)",
        result.a_total_pages, result.b_total_pages, result.aligned_pairs
    );
    println!("samples:               {}", result.samples.len());
    println!("generation:            {generated_ms} ms");
    println!("compare wall time:     {compare_ms} ms");
    println!(
        "VmHWM before compare:  {:.1} MiB",
        hwm_before as f64 / 1024.0
    );
    println!(
        "VmHWM after compare:   {:.1} MiB  (peak)",
        hwm_after as f64 / 1024.0
    );
    println!(
        "one decoded page:      {:.1} MiB",
        decoded_bytes_per_page as f64 / (1024.0 * 1024.0)
    );
    println!(
        "all pages decoded:     {:.0} MiB  (what holding every page would cost)",
        all_pages_decoded as f64 / (1024.0 * 1024.0)
    );
    // Touch every sample so the compiler cannot drop the result before the measurement is read.
    result.samples.sort_by_key(|sample| sample.a_page_index);
    let _ = result.samples.len();
}
