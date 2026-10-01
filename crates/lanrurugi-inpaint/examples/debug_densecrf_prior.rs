//! Diagnostic: compares DenseCRF's refined mask under the detector's own coarse prior vs. a
//! prior augmented with a from-scratch local-background stroke estimate. Real-page evidence
//! (2026-09-18, issue #101) showed the DB detector's `raw_text_mask` covers only ~25-35% of a
//! region crop and can miss whole columns/lines; anything >8px from a prior pixel is outside
//! `densecrf_stroke_mask`'s own per-component search window and can never be recovered. This tool
//! renders both variants against the real crop so the union-seed experiment can be judged by eye
//! before it is considered for production.
//!
//! usage: debug_densecrf_prior <crop.png> <prior.raw> <out_prefix>
//!   prior.raw: width u32 LE, height u32 LE, then width*height bytes (0/1)

use image::{Rgb, RgbImage};
use lanrurugi_inpaint::stroke_mask::{densecrf_stroke_mask, local_background_stroke_mask};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: debug_densecrf_prior <crop.png> <prior.raw> <out_prefix>");
        std::process::exit(2);
    }
    let crop = image::open(&args[1]).expect("open crop").to_rgb8();
    let (w, h) = crop.dimensions();
    let blob = std::fs::read(&args[2]).expect("read prior");
    let pw = u32::from_le_bytes(blob[0..4].try_into().unwrap());
    let ph = u32::from_le_bytes(blob[4..8].try_into().unwrap());
    let prior: Vec<bool> = blob[8..].iter().map(|&b| b != 0).collect();
    assert_eq!((pw, ph), (w, h), "prior dims must match crop");
    assert_eq!(prior.len(), (w * h) as usize);
    let prefix = &args[3];

    let prior_px = prior.iter().filter(|&&b| b).count();
    eprintln!(
        "crop {w}x{h}, detector prior {prior_px}px ({:.1}%)",
        100.0 * prior_px as f64 / prior.len() as f64
    );

    // Local-background from-scratch estimate (its own `raw`, not the dilated masks).
    let local = local_background_stroke_mask(&crop, Some(&prior)).map(|m| m.raw);
    if let Some(local) = &local {
        let n = local.iter().filter(|&&b| b).count();
        eprintln!(
            "local-background raw {n}px ({:.1}%)",
            100.0 * n as f64 / local.len() as f64
        );
    } else {
        eprintln!("local-background returned None");
    }

    let current = densecrf_stroke_mask(&crop, &prior).map(|m| m.raw);
    if let Some(m) = &current {
        eprintln!(
            "densecrf(detector prior) raw {}px",
            m.iter().filter(|&&b| b).count()
        );
    } else {
        eprintln!("densecrf(detector prior) returned None");
    }

    if let Some(local) = &local {
        let union: Vec<bool> = prior.iter().zip(local).map(|(&a, &b)| a || b).collect();
        let n = union.iter().filter(|&&b| b).count();
        eprintln!(
            "union prior {n}px ({:.1}%)",
            100.0 * n as f64 / union.len() as f64
        );
        match densecrf_stroke_mask(&crop, &union) {
            Some(m) => {
                eprintln!(
                    "densecrf(union prior) raw {}px",
                    m.raw.iter().filter(|&&b| b).count()
                );
                save(&crop, &m.raw, &format!("{prefix}_densecrf_union.png"));
            }
            None => eprintln!("densecrf(union prior) returned None"),
        }
        save(&crop, local, &format!("{prefix}_local_bg.png"));
        save(&crop, &union, &format!("{prefix}_union_prior.png"));
    }
    save(&crop, &prior, &format!("{prefix}_detector_prior.png"));
    if let Some(m) = &current {
        save(&crop, m, &format!("{prefix}_densecrf_current.png"));
    }
}

fn save(crop: &RgbImage, mask: &[bool], path: &str) {
    let (w, h) = crop.dimensions();
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let px = *crop.get_pixel(x, y);
            out.put_pixel(x, y, if mask[i] { Rgb([255, 0, 255]) } else { px });
        }
    }
    out.save(path).expect("save overlay");
    eprintln!("saved {path}");
}
