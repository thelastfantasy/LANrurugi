use lanrurugi_inpaint::stroke_mask::local_background_stroke_mask;
use lanrurugi_inpaint::Inpainter;
use std::path::Path;

/// Every text region OCR/translation actually detected on the real page 102 (archive
/// 47f05ecc07e3ff98b2b7021c6a3eef9cdfedaf38), captured live via a temporary `tracing::warn!` in
/// `lanrurugi-api::translation_pipeline::composite_and_cache` — `(x, y, w, h)`.
const PAGE_102_REGIONS: &[(u32, u32, u32, u32)] = &[
    (124, 112, 90, 292),
    (124, 140, 138, 234),
    (204, 1346, 92, 174),
    (259, 771, 50, 223),
    (305, 1143, 79, 155),
    (310, 586, 73, 158),
    (402, 1238, 16, 17),
    (411, 1055, 53, 265),
    (438, 120, 88, 217),
    (472, 606, 131, 231),
    (493, 90, 110, 152),
    (496, 1158, 108, 253),
    (521, 1302, 38, 38),
    (575, 588, 107, 105),
    (73, 1314, 109, 148),
    (753, 83, 118, 179),
    (773, 445, 89, 92),
    (780, 131, 136, 234),
    (827, 1157, 95, 167),
    (82, 60, 396, 62),
    (864, 766, 94, 174),
    (90, 501, 74, 177),
    (910, 859, 120, 166),
    (911, 127, 130, 200),
    (919, 546, 152, 206),
    (934, 1418, 42, 43),
    (958, 1082, 125, 231),
    (980, 119, 91, 367),
];

fn main() {
    let model_dir = std::env::var("LANRURUGI_INPAINT_MODEL_DIR").expect("model dir env not set");
    let model_path = Path::new(&model_dir).join("lama-manga-dynamic.onnx");
    let inpainter =
        Inpainter::load(&model_path, 4, 4 * 1024 * 1024 * 1024).expect("failed to load model");

    let page_path = std::env::args()
        .nth(1)
        .expect("usage: debug_erase_page <page.jpg> <out.png>");
    let out_path = std::env::args()
        .nth(2)
        .expect("usage: debug_erase_page <page.jpg> <out.png>");

    let page = image::open(&page_path)
        .expect("failed to open page")
        .to_rgb8();
    let (pw, ph) = page.dimensions();
    eprintln!("page size: {pw}x{ph}");

    // Sanity check (DeepSeek's suggestion, 2026-09-08): an all-zero mask should make erase_page
    // reproduce the input almost exactly — the model's own ONNX graph computes
    // `mask * reconstructed + (1 - mask) * image`, which collapses to plain `image` when
    // `mask == 0` everywhere. A real mismatch here would mean the tensor/decode pipeline is still
    // broken regardless of any masked-region result.
    let zero_mask = vec![false; (pw * ph) as usize];
    let sanity = inpainter
        .erase_page(&page, &zero_mask, &zero_mask)
        .expect("sanity-check erase_page (all-zero mask) failed");
    let mut max_diff = 0i32;
    let mut sum_diff = 0i64;
    for (orig, out) in page.pixels().zip(sanity.pixels()) {
        for c in 0..3 {
            let d = (i32::from(orig.0[c]) - i32::from(out.0[c])).abs();
            max_diff = max_diff.max(d);
            sum_diff += i64::from(d);
        }
    }
    let mean_diff = sum_diff as f64 / (pw as f64 * ph as f64 * 3.0);
    eprintln!(
        "sanity check (all-zero mask): max per-channel diff={max_diff}, mean per-channel diff={mean_diff:.3}"
    );

    // Whole-page masks: for each detected text region, use `local_background_stroke_mask`'s own
    // precise glyph-shape masks when it finds real local contrast (matching what the real
    // `combined_erase_mask` fallback path does in production now), falling back to the region's
    // whole rectangle (for both masks alike) only when it doesn't (a flat crop with nothing to
    // separate). `model_mask` (wide dilation, fed to LaMa) and `paste_mask` (narrow dilation, the
    // pixels actually overwritten in the output) are tracked separately — see
    // `Inpainter::erase_page`'s own doc comment for why the split matters.
    let mut model_mask = vec![false; (pw * ph) as usize];
    let mut paste_mask = vec![false; (pw * ph) as usize];
    let mut precise_regions = 0usize;
    for &(x0, y0, w, h) in PAGE_102_REGIONS {
        let (rw, rh) = (w.min(pw.saturating_sub(x0)), h.min(ph.saturating_sub(y0)));
        if rw == 0 || rh == 0 {
            continue;
        }
        let crop = image::imageops::crop_imm(&page, x0, y0, rw, rh).to_image();
        // No real OCR pipeline here to source a `coarse_prior` from — this tool works straight off
        // a raw page image, not a `DetectedTextRegion` with its own `raw_text_mask` attached.
        let region_mask = local_background_stroke_mask(&crop, None);
        if region_mask.is_some() {
            precise_regions += 1;
        }
        for ry in 0..rh {
            for rx in 0..rw {
                let idx = (ry * rw + rx) as usize;
                let (erase_model, erase_paste) = match &region_mask {
                    Some(m) => (m.model[idx], m.paste[idx]),
                    None => (true, true),
                };
                let page_idx = ((y0 + ry) * pw + (x0 + rx)) as usize;
                if erase_model {
                    model_mask[page_idx] = true;
                }
                if erase_paste {
                    paste_mask[page_idx] = true;
                }
            }
        }
    }
    let model_true: usize = model_mask.iter().filter(|&&m| m).count();
    let paste_true: usize = paste_mask.iter().filter(|&&m| m).count();
    eprintln!(
        "model mask true count: {model_true}, paste mask true count: {paste_true}, across {} regions ({precise_regions} used a precise local_background_stroke_mask, {} fell back to the full rectangle)",
        PAGE_102_REGIONS.len(),
        PAGE_102_REGIONS.len() - precise_regions
    );

    let result = inpainter
        .erase_page(&page, &model_mask, &paste_mask)
        .expect("erase_page failed");
    result.save(&out_path).expect("failed to save output");
    eprintln!("saved to {out_path}");

    // Sample the centre pixel of every region to spot-check reconstruction quality.
    for &(x0, y0, w, h) in PAGE_102_REGIONS {
        let (cx, cy) = ((x0 + w / 2).min(pw - 1), (y0 + h / 2).min(ph - 1));
        let px = result.get_pixel(cx, cy);
        eprintln!("region ({x0},{y0},{w},{h}) centre ({cx},{cy}) = {:?}", px.0);
    }
}
