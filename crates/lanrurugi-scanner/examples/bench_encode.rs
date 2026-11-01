//! Measures encode time and output size for one real composited page, across the formats and
//! encoder settings the translation pipeline actually has available.
//!
//! Written because "is JPEG XL worth it?" was being argued from memory: the same pages are on disk
//! in both formats, but not the *time* each took. Usage:
//!
//! `mise run debug-encode -- <page.webp|page.png>`
//!
//! Prints one line per variant: elapsed ms, output bytes, and the ratio against the WebP baseline.

use std::time::Instant;

use image::RgbImage;
use jixel_image_bindings::{EncodeConfig, JixelEncoder, Speed};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: debug_encode <image>");
    let bytes = std::fs::read(&path).expect("read input");
    let image: RgbImage = image::load_from_memory(&bytes)
        .expect("decode input")
        .to_rgb8();
    println!(
        "input: {path} ({}x{}, {} px)",
        image.width(),
        image.height(),
        u64::from(image.width()) * u64::from(image.height())
    );

    let quality = 85u8;

    // Baseline: what the pipeline does now.
    let started = Instant::now();
    let webp = encode_webp(&image, f32::from(quality)).expect("webp");
    report("webp q85 (1 thread)", started, webp.len(), None);

    let baseline = webp.len();
    for (label, threads, speed) in [
        ("jxl q85 Fast 1 thread (old default)", 1usize, Speed::Fast),
        ("jxl q85 Fast 8 threads", 8, Speed::Fast),
        ("jxl q85 Fastest 1 thread", 1, Speed::Fastest),
        ("jxl q85 Fastest 8 threads", 8, Speed::Fastest),
        ("jxl q85 Slow 8 threads (best size)", 8, Speed::Slow),
    ] {
        let mut config = EncodeConfig::default()
            .with_quality(f32::from(quality))
            .with_speed(speed);
        config.num_threads = threads;
        let started = Instant::now();
        let encoded = encode_jxl(&image, &config);
        report(label, started, encoded.len(), Some(baseline));
    }
}

/// Mirrors `lanrurugi_translate::composite::encode_webp`'s own settings so the baseline is the
/// pipeline's real one, not a different quality.
fn encode_webp(image: &RgbImage, quality: f32) -> Result<Vec<u8>, String> {
    webpx::Encoder::new_rgb(image.as_raw(), image.width(), image.height())
        .quality(quality)
        .encode(webpx::Unstoppable)
        .map_err(|e| format!("webp encode: {e}"))
}

fn encode_jxl(image: &RgbImage, config: &EncodeConfig) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut out = Vec::new();
    JixelEncoder::with_config(&mut out, config.clone())
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .expect("jxl encode");
    out
}

fn report(label: &str, started: Instant, bytes: usize, baseline: Option<usize>) {
    let elapsed = started.elapsed().as_millis();
    let size = match baseline {
        Some(base) => format!("{:>9.2} MB ({:+5.1}%)", mb(bytes), ratio(bytes, base)),
        None => format!("{:>9.2} MB", mb(bytes)),
    };
    println!("{label:<38} {elapsed:>6} ms  {size}");
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn ratio(bytes: usize, baseline: usize) -> f64 {
    (bytes as f64 / baseline as f64 - 1.0) * 100.0
}
