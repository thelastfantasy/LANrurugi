use image::{Rgb, RgbImage};
use lanrurugi_inpaint::stroke_mask::local_background_stroke_mask;

/// Loads a crop, runs `local_background_stroke_mask` on it, and writes out a visualisation:
/// paste-mask pixels painted solid red, model-mask-only pixels (the wider ring fed to LaMa but
/// never actually replaced in the final page) painted solid yellow, everything else left as the
/// original crop — so the actual classification, and the split between the two masks, can be
/// inspected directly against the real image, not just trusted from a synthetic unit test.
fn main() {
    let in_path = std::env::args()
        .nth(1)
        .expect("usage: debug_stroke_mask <in.png> <out.png>");
    let out_path = std::env::args()
        .nth(2)
        .expect("usage: debug_stroke_mask <in.png> <out.png>");

    let crop = image::open(&in_path)
        .expect("failed to open crop")
        .to_rgb8();
    let (w, h) = crop.dimensions();
    eprintln!("crop size: {w}x{h}");

    // No real OCR pipeline here to source a `coarse_prior` from — this tool works straight off a
    // standalone crop file, not a `DetectedTextRegion` with its own `raw_text_mask` attached.
    let Some(mask) = local_background_stroke_mask(&crop, None) else {
        eprintln!("local_background_stroke_mask returned None — no local contrast found at all");
        return;
    };

    let raw_true: usize = mask.raw.iter().filter(|&&m| m).count();
    let paste_true: usize = mask.paste.iter().filter(|&&m| m).count();
    let model_true: usize = mask.model.iter().filter(|&&m| m).count();
    eprintln!(
        "raw stroke pixels: {raw_true}/{} ({:.1}%), paste: {paste_true}/{} ({:.1}%), model: {model_true}/{} ({:.1}%)",
        mask.raw.len(),
        100.0 * raw_true as f64 / mask.raw.len() as f64,
        mask.paste.len(),
        100.0 * paste_true as f64 / mask.paste.len() as f64,
        mask.model.len(),
        100.0 * model_true as f64 / mask.model.len() as f64,
    );

    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let px = if mask.paste[i] {
                Rgb([255, 0, 0])
            } else if mask.model[i] {
                Rgb([255, 255, 0])
            } else {
                *crop.get_pixel(x, y)
            };
            out.put_pixel(x, y, px);
        }
    }
    out.save(&out_path).expect("failed to save output");
    eprintln!("saved visualisation to {out_path}");
}
