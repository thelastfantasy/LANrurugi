//! Captures the raw output of one `erase_page` call for a page that fits the boundary's limit, so the
//! *same* call can be run against a pre-change and a post-change build and the two outputs compared
//! byte for byte (quickstart scenario 6, SC-003).
//!
//! Deliberately uses **only** API that exists on both sides of feature 012's change — same client
//! type, same method signature, same wire types — so this file compiles unchanged against either
//! revision. That is the whole reason it is a separate file from `gpu_worker_boundary.rs`, which
//! asserts against feature-012-specific API and therefore cannot exist in the pre-change tree.
//!
//! Opt-in: `LANRURUGI_GPU_WORKER_ERASE_CAPTURE=<path>` writes the returned page's raw RGB bytes
//! there, and the checksum is printed either way. Skips (not fails) when unset or when the worker /
//! model is unavailable.

use std::time::Instant;

use lanrurugi_api::gpu_worker_client::{GpuWorkerClient, WorkerKind};
use lanrurugi_gpu_ipc::{RawMask, RawRgbImage};

/// Fits the boundary's limit, so both revisions take the full-resolution path — which is what makes
/// "the output did not change" the right thing to expect.
const PAGE: (u32, u32) = (2400, 1500);

fn e2e_enabled() -> Option<String> {
    std::env::var("LANRURUGI_GPU_WORKER_ERASE_CAPTURE")
        .ok()
        .filter(|p| !p.trim().is_empty())
}

fn model_dir_available() -> bool {
    if let Some(dir) = std::env::var_os(lanrurugi_inpaint::model_discovery::MODEL_DIR_ENV) {
        if std::path::Path::new(&dir).is_dir() {
            return true;
        }
    }
    lanrurugi_inpaint::model_discovery::find_model_dir().is_ok()
}

fn ensure_worker_binary() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(deps_dir) = exe.parent() else {
        return false;
    };
    let Some(binary) = deps_dir.parent().map(|p| p.join("lanrurugi-gpu-worker")) else {
        return false;
    };
    if !binary.is_file() {
        return false;
    }
    let expected = deps_dir.join("lanrurugi-gpu-worker");
    if !expected.exists() {
        return std::os::unix::fs::symlink(&binary, &expected).is_ok();
    }
    true
}

fn synthetic_page(width: u32, height: u32, bars: &[(u32, u32, u32, u32)]) -> RawRgbImage {
    let mut rgb = vec![0u8; (width as usize) * (height as usize) * 3];
    for y in 0..height {
        for x in 0..width {
            let i = ((y as usize) * (width as usize) + (x as usize)) * 3;
            let masked = bars
                .iter()
                .any(|&(bx, by, bw, bh)| x >= bx && x < bx + bw && y >= by && y < by + bh);
            let (r, g, b) = if masked {
                (0u8, 0u8, 0u8)
            } else {
                (
                    ((x * 7) % 256) as u8,
                    ((y * 11) % 256) as u8,
                    (((x + y) * 3) % 256) as u8,
                )
            };
            rgb[i] = r;
            rgb[i + 1] = g;
            rgb[i + 2] = b;
        }
    }
    RawRgbImage { width, height, rgb }
}

fn mask_from_bars(width: u32, height: u32, bars: &[(u32, u32, u32, u32)], dilate: u32) -> RawMask {
    let mut mask = vec![false; (width as usize) * (height as usize)];
    for &(bx, by, bw, bh) in bars {
        let x0 = bx.saturating_sub(dilate);
        let y0 = by.saturating_sub(dilate);
        let x1 = (bx + bw + dilate).min(width);
        let y1 = (by + bh + dilate).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                mask[(y as usize) * (width as usize) + (x as usize)] = true;
            }
        }
    }
    RawMask {
        width,
        height,
        mask,
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_erase_output_for_cross_revision_comparison() {
    let Some(output_path) = e2e_enabled() else {
        eprintln!("skipping: set LANRURUGI_GPU_WORKER_ERASE_CAPTURE=<path> to run this");
        return;
    };
    if !ensure_worker_binary() {
        eprintln!(
            "skipping: target/debug/lanrurugi-gpu-worker not built — run `mise run build` first"
        );
        return;
    }
    if !model_dir_available() {
        eprintln!("skipping: no inpainting model directory resolvable");
        return;
    }

    let (w, h) = PAGE;
    let unit = w / 16;
    let bars = vec![
        (unit, unit, unit * 3, unit * 2),
        (unit * 6, h / 2, unit * 4, unit * 2),
        (unit * 11, unit * 3, unit * 2, unit * 4),
    ];

    let input = synthetic_page(w, h, &bars);
    let input_checksum = fnv1a(&input.rgb);

    let client = GpuWorkerClient::new(WorkerKind::Image);
    let started = Instant::now();
    let erased = client
        .erase_page(
            input,
            mask_from_bars(w, h, &bars, 12),
            mask_from_bars(w, h, &bars, 0),
        )
        .await
        .expect("erase_page failed");
    let elapsed = started.elapsed();

    std::fs::write(&output_path, &erased.rgb).expect("writing the captured page");
    eprintln!(
        "CAPTURE page={w}x{h} input_fnv1a={input_checksum:#018x} output_fnv1a={:#018x} \
         bytes={} elapsed={elapsed:?} path={output_path}",
        fnv1a(&erased.rgb),
        erased.rgb.len()
    );
}
