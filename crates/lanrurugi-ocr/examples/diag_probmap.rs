//! Temporary diagnostic: dump the raw DB probability map over specific page regions to decide
//! whether a detection miss is "the model never saw it" or "post-processing dropped it".
//! Delete after the investigation it was written for.

use lanrurugi_ocr::detect::TextDetector;
use std::path::PathBuf;

struct Roi {
    name: &'static str,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber_init();

    let mut args = std::env::args().skip(1);
    let model_path = PathBuf::from(
        args.next()
            .expect("usage: diag_probmap <model.onnx> <page.png> [box_threshold]"),
    );
    let page_path = PathBuf::from(
        args.next()
            .expect("usage: diag_probmap <model.onnx> <page.png> [box_threshold]"),
    );
    let box_threshold: Option<f32> = args
        .next()
        .map(|s| s.parse().expect("box_threshold must be a float"));

    let img = image::open(&page_path)?.to_rgb8();
    let (pw, ph) = (img.width(), img.height());
    println!("page: {}x{}", pw, ph);

    let detector = match box_threshold {
        Some(t) => {
            println!("using overridden box_threshold={}", t);
            TextDetector::load_with_box_threshold(&model_path, t)?
        }
        None => TextDetector::load(&model_path)?,
    };
    let mut out = detector.detect_batch_with_raw_mask(vec![img])?;
    let (boxes, mask) = out.remove(0);

    println!(
        "\n=== detected boxes (after guards), {} total ===",
        boxes.len()
    );
    let mut sorted: Vec<_> = boxes.iter().collect();
    sorted.sort_by_key(|b| (b.y, b.x));
    for b in &sorted {
        println!("  x={} y={} w={} h={}", b.x, b.y, b.w, b.h);
    }

    let rois = [
        Roi {
            name: "A-HIT  皇族ガーディアン (line1, detected)",
            x: 877,
            y: 846,
            w: 214,
            h: 31,
        },
        Roi {
            name: "A-MISS トウカ (line2, MISSING)",
            x: 880,
            y: 887,
            w: 137,
            h: 36,
        },
        Roi {
            name: "B-HIT  皇族ガーディアンスズナ (detected)",
            x: 640,
            y: 996,
            w: 215,
            h: 89,
        },
        Roi {
            name: "C-HIT  見知らぬ (right column, detected)",
            x: 928,
            y: 1174,
            w: 49,
            h: 141,
        },
        Roi {
            name: "C-MISS 美女3人 (left column, MISSING)",
            x: 893,
            y: 1183,
            w: 37,
            h: 190,
        },
    ];

    println!("\n=== ROI probability stats ===");
    for r in &rois {
        let mut vals = Vec::new();
        for yy in r.y..(r.y + r.h).min(ph) {
            for xx in r.x..(r.x + r.w).min(pw) {
                vals.push(mask.get(xx, yy));
            }
        }
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = vals.len();
        let mean = vals.iter().sum::<f32>() / n as f32;
        let pct = |p: f64| vals[((n as f64 - 1.0) * p) as usize];
        let frac_over = |t: f32| vals.iter().filter(|v| **v >= t).count() as f64 / n as f64;
        println!(
            "{}\n   box=({},{},{}x{}) n={} max={:.3} mean={:.3} p50={:.3} p90={:.3} p99={:.3}\n   frac>=0.3: {:.3}  >=0.5: {:.3}  >=0.6: {:.3}  >=0.7: {:.3}",
            r.name, r.x, r.y, r.w, r.h, n,
            vals[n - 1], mean, pct(0.5), pct(0.9), pct(0.99),
            frac_over(0.3), frac_over(0.5), frac_over(0.6), frac_over(0.7),
        );
    }

    // --- Simulate `densecrf_stroke_mask`'s own pre-filter per ROI: binarize at
    // `RawTextMask`'s threshold (>=0.3), dilate by `PRIOR_DILATION_RADIUS` (4), split into
    // connected components, and drop any component covering > `MAX_COMPONENT_AREA_FRACTION`
    // (50%) of the crop. If a *legitimate* dense prior trips this, the whole region gets no
    // precise mask and is skipped (no erase, no redraw) — which looks exactly like a "miss".
    println!("\n=== densecrf pre-filter simulation (binarize>=0.3, dilate r=4, cap 50%) ===");
    for r in &rois {
        let (rw, rh) = (r.w, r.h);
        let n = (rw * rh) as usize;
        let mut m = vec![false; n];
        for yy in 0..rh {
            for xx in 0..rw {
                m[(yy * rw + xx) as usize] = mask.get(r.x + xx, r.y + yy) >= 0.3;
            }
        }
        let raw = m.iter().filter(|b| **b).count();
        let mut d = m.clone();
        for yy in 0..rh as i32 {
            for xx in 0..rw as i32 {
                if m[(yy as u32 * rw + xx as u32) as usize] {
                    continue;
                }
                's: for dy in -4..=4i32 {
                    let ny = yy + dy;
                    if ny < 0 || ny >= rh as i32 {
                        continue;
                    }
                    for dx in -4..=4i32 {
                        let nx = xx + dx;
                        if nx < 0 || nx >= rw as i32 {
                            continue;
                        }
                        if m[(ny as u32 * rw + nx as u32) as usize] {
                            d[(yy as u32 * rw + xx as u32) as usize] = true;
                            break 's;
                        }
                    }
                }
            }
        }
        let dilated = d.iter().filter(|b| **b).count();
        let mut seen = vec![false; n];
        let mut max_comp = 0usize;
        for i in 0..n {
            if !d[i] || seen[i] {
                continue;
            }
            let mut stack = vec![i];
            seen[i] = true;
            let mut c = 0usize;
            while let Some(j) = stack.pop() {
                c += 1;
                let (jx, jy) = ((j as u32 % rw) as i32, (j as u32 / rw) as i32);
                for dy in -1..=1i32 {
                    for dx in -1..=1i32 {
                        let (nx, ny) = (jx + dx, jy + dy);
                        if nx < 0 || ny < 0 || nx >= rw as i32 || ny >= rh as i32 {
                            continue;
                        }
                        let k = (ny as u32 * rw + nx as u32) as usize;
                        if d[k] && !seen[k] {
                            seen[k] = true;
                            stack.push(k);
                        }
                    }
                }
            }
            max_comp = max_comp.max(c);
        }
        let pct = |v: usize| 100.0 * v as f32 / n as f32;
        println!(
            "{}: raw>=0.3={} ({:.1}%) dilate4={} ({:.1}%) max_component={} ({:.1}%) -> {}",
            r.name,
            raw,
            pct(raw),
            dilated,
            pct(dilated),
            max_comp,
            pct(max_comp),
            if max_comp as f32 / n as f32 > 0.5 {
                "DROPPED by MAX_COMPONENT_AREA_FRACTION -> densecrf_stroke_mask returns None"
            } else {
                "kept"
            }
        );
    }

    // --- Binary mask + contours: replicate `oar-ocr-core`'s own DB post-process *input* chain so
    // a miss can be classified as "no contour existed at all" vs "contour existed but its own
    // `box_score` failed `box_thresh`". `oar-ocr`'s `threshold_to_mask` binarizes with `val >
    // thresh`; `find_contours` here is the exact same `imageproc` function it calls on that buffer;
    // for axis-aligned manga text boxes the AABB mean is a faithful proxy for `box_score_fast`
    // (its min-area rect equals the AABB). This is the measurement the earlier ROI-window stats
    // could not give: those used a fixed *sampling window*, not the contour's own box.
    let thresh: f32 = std::env::var("DIAG_THRESH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.3);
    let box_thresh: f32 = std::env::var("DIAG_BOX_THRESH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.6);
    println!(
        "\n=== contours of prob > {} (oar-ocr's own bitmap input; box_thresh={}) ===",
        thresh, box_thresh
    );
    let mut bin = image::GrayImage::new(pw, ph);
    for yy in 0..ph {
        for xx in 0..pw {
            bin.put_pixel(
                xx,
                yy,
                image::Luma([if mask.get(xx, yy) > thresh { 255 } else { 0 }]),
            );
        }
    }
    let contours = imageproc::contours::find_contours::<u32>(&bin);
    let (mut pass, mut drop_min_size, mut drop_score) = (0usize, 0usize, 0usize);
    let mut rows: Vec<(u32, u32, u32, u32, f32, bool)> = Vec::new();
    for c in contours.iter() {
        if c.points.len() < 4 {
            drop_min_size += 1;
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for p in &c.points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        let min_side = w.min(h);
        let mut sum = 0f32;
        let mut n = 0u32;
        for yy in y0..=y1 {
            for xx in x0..=x1 {
                sum += mask.get(xx, yy);
                n += 1;
            }
        }
        let mean = if n > 0 { sum / n as f32 } else { 0.0 };
        let passes = min_side >= 3 && mean >= box_thresh;
        if min_side < 3 {
            drop_min_size += 1;
        } else if mean < box_thresh {
            drop_score += 1;
        } else {
            pass += 1;
        }
        rows.push((x0, y0, w, h, mean, passes));
    }
    println!(
        "contours total={} pass={} dropped_by_min_size={} dropped_by_box_score={}",
        contours.len(),
        pass,
        drop_min_size,
        drop_score
    );
    println!("contours intersecting a named ROI:");
    for (x0, y0, w, h, mean, passes) in &rows {
        let which = rois
            .iter()
            .find(|r| *x0 < r.x + r.w && r.x < x0 + w && *y0 < r.y + r.h && r.y < y0 + h);
        if let Some(r) = which {
            println!(
                "  {} <- contour bbox=({},{},{}x{}) aabb_mean={:.3} {}",
                r.name,
                x0,
                y0,
                w,
                h,
                mean,
                if *passes {
                    "PASSES box_thresh"
                } else {
                    "DROPPED by box_score"
                }
            );
        }
    }

    // Row-by-row profile across the two labels so the boundary between a detected line and a
    // missed one is visible directly in the probabilities.
    println!("\n=== per-row max/mean probability, label A column x=877..1091, y=835..935 ===");
    for yy in 835..935u32 {
        let (mut mx, mut sum) = (0.0f32, 0.0f32);
        for xx in 877..1092u32 {
            let v = mask.get(xx, yy);
            mx = mx.max(v);
            sum += v;
        }
        let mean = sum / (1092 - 877) as f32;
        println!(
            "  y={:4}  max={:.3}  mean={:.3}  {}",
            yy,
            mx,
            mean,
            bar(mean)
        );
    }

    println!("\n=== per-column max/mean, region C, x=880..1000, y=1174..1380 ===");
    for xx in 880..1000u32 {
        let (mut mx, mut sum) = (0.0f32, 0.0f32);
        for yy in 1174..1380u32 {
            let v = mask.get(xx, yy);
            mx = mx.max(v);
            sum += v;
        }
        let mean = sum / (1380 - 1174) as f32;
        println!(
            "  x={:4}  max={:.3}  mean={:.3}  {}",
            xx,
            mx,
            mean,
            bar(mean)
        );
    }

    // Dump the probability map as a greyscale PNG for visual inspection.
    let mut gray = image::GrayImage::new(pw, ph);
    for yy in 0..ph {
        for xx in 0..pw {
            gray.put_pixel(xx, yy, image::Luma([(mask.get(xx, yy) * 255.0) as u8]));
        }
    }
    let dump = std::env::var("DIAG_PROBMAP_OUT")
        .unwrap_or_else(|_| ".cargo-target/diag/probmap.png".into());
    gray.save(&dump)?;
    println!("\nprobability map written to {}", dump);

    Ok(())
}

fn bar(v: f32) -> String {
    "#".repeat((v * 60.0) as usize)
}

/// Minimal stderr logger so `detect`'s own `discarding an anomalously ...` warning is visible.
fn tracing_subscriber_init() {
    let _ = tracing::subscriber::set_global_default(SimpleSub);
}

struct SimpleSub;
impl tracing::Subscriber for SimpleSub {
    fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _s: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _s: &tracing::Id, _v: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _s: &tracing::Id, _f: &tracing::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct V;
        impl tracing::field::Visit for V {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                eprint!(" {}={:?}", f.name(), v);
            }
        }
        eprint!("[{}]", event.metadata().level());
        event.record(&mut V);
        eprintln!();
    }
    fn enter(&self, _s: &tracing::Id) {}
    fn exit(&self, _s: &tracing::Id) {}
}
