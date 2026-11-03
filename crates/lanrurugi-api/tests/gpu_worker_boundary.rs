//! End-to-end verification of the GPU-worker boundary against a **real** worker process, real model
//! weights, and the real Unix-socket transport.
//!
//! Opt-in, and skipped rather than failed when its inputs are missing:
//! - `LANRURUGI_GPU_WORKER_BOUNDARY_E2E=1` must be set (a full worker spawn plus a real ONNX
//!   inference is not something every `cargo test` run should pay for),
//! - the inpainting model directory must resolve (`LANRURUGI_INPAINT_MODEL_DIR`, or the crate's own
//!   discovery),
//! - `target/debug/lanrurugi-gpu-worker` must exist. Note that `cargo test -p lanrurugi-gpu-worker`
//!   does **not** produce it: `--bins` under `cargo test` builds a hashed test-harness binary in
//!   `target/debug/deps/`, not the plain executable this client spawns. Run `mise run build` first.
//!
//! This is the only place the feature's central claim is checked against the real machinery rather
//! than against a pure function: that a page whose full-resolution request cannot fit the boundary's
//! message limit still erases successfully, because the client reduces it *before* sending.

use std::path::PathBuf;
use std::time::Instant;

use lanrurugi_api::gpu_worker_client::{GpuWorkerClient, WorkerKind};
use lanrurugi_gpu_ipc::{RawMask, RawRgbImage};

/// 4000 x 3600 = 14.4 megapixels, comfortably past the ~13.4 MP the full-resolution erase request
/// may occupy. Real archives in this state were the reason the feature exists.
const OVER_LIMIT: (u32, u32) = (4000, 3600);

/// A page that fits, used to show the reduced path did not replace the existing one.
const FITTING: (u32, u32) = (2400, 1500);

fn e2e_enabled() -> bool {
    std::env::var("LANRURUGI_GPU_WORKER_BOUNDARY_E2E").is_ok_and(|v| v.trim() == "1")
}

/// Installs a `tracing` subscriber for the test process. `GpuWorkerClient` forwards the worker's own
/// stdout/stderr through `tracing`, so without this a worker refusal arrives as a one-line
/// `WorkerError` with its actual cause discarded — which is exactly how the first run of this test
/// failed to explain itself.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();
}

/// Prints the environment the worker inherits, so a model-*resolution* failure can be told apart
/// from a model-*load* failure or a missing ONNX Runtime without guessing.
fn dump_environment() {
    for key in [
        lanrurugi_inpaint::model_discovery::MODEL_DIR_ENV,
        "ORT_DYLIB_PATH",
        "LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH",
        "LANRURUGI_GPU_WORKER_MODELS",
        "LANRURUGI_GPU_WORKER_SOCKET",
    ] {
        eprintln!("  env {key} = {:?}", std::env::var(key).ok());
    }
    let ort_cpu = "/usr/local/lib/ort-cpu/libonnxruntime.so";
    eprintln!(
        "  {ort_cpu} exists = {}",
        std::path::Path::new(ort_cpu).is_file()
    );
}

/// Whether the inpainting model can be found the same way the worker itself finds it.
fn model_dir_available() -> bool {
    if let Some(dir) = std::env::var_os(lanrurugi_inpaint::model_discovery::MODEL_DIR_ENV) {
        if std::path::Path::new(&dir).is_dir() {
            return true;
        }
    }
    lanrurugi_inpaint::model_discovery::find_model_dir().is_ok()
}

/// `GpuWorkerClient::worker_binary_path` looks for the worker next to the *currently running*
/// executable. For a test binary that directory is `target/debug/deps`, while cargo puts binaries in
/// `target/debug` — so a symlink is planted there, which is a test-harness detail rather than a
/// change to how the client finds its worker. Idempotent, and returns the binary's real path.
fn ensure_worker_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps_dir = exe.parent()?.to_path_buf();
    let binary = deps_dir.parent()?.join("lanrurugi-gpu-worker");
    if !binary.is_file() {
        return None;
    }
    let expected = deps_dir.join("lanrurugi-gpu-worker");
    if !expected.exists() {
        std::os::unix::fs::symlink(&binary, &expected).ok()?;
    }
    Some(binary)
}

/// A page with high-contrast marks exactly where it will be masked, so a real model has something to
/// reconstruct: solid black bars under the mask, and a busy gradient elsewhere.
fn synthetic_page(width: u32, height: u32, black_bars: &[(u32, u32, u32, u32)]) -> RawRgbImage {
    let mut rgb = vec![0u8; (width as usize) * (height as usize) * 3];
    for y in 0..height {
        for x in 0..width {
            let i = ((y as usize) * (width as usize) + (x as usize)) * 3;
            let masked = black_bars
                .iter()
                .any(|&(bx, by, bw, bh)| x >= bx && x < bx + bw && y >= by && y < by + bh);
            if masked {
                rgb[i] = 0;
                rgb[i + 1] = 0;
                rgb[i + 2] = 0;
            } else {
                rgb[i] = ((x * 7) % 256) as u8;
                rgb[i + 1] = ((y * 11) % 256) as u8;
                rgb[i + 2] = (((x + y) * 3) % 256) as u8;
            }
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

/// Three separated bars, so the erased page has more than one region to reconstruct.
fn bars_for(width: u32, height: u32) -> Vec<(u32, u32, u32, u32)> {
    let unit = width / 16;
    vec![
        (unit, unit, unit * 3, unit * 2),
        (unit * 6, height / 2, unit * 4, unit * 2),
        (unit * 11, unit * 3, unit * 2, unit * 4),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn erase_succeeds_for_both_an_over_limit_page_and_a_fitting_one() {
    if !e2e_enabled() {
        eprintln!("skipping: set LANRURUGI_GPU_WORKER_BOUNDARY_E2E=1 to run this");
        return;
    }
    if ensure_worker_binary().is_none() {
        eprintln!(
            "skipping: target/debug/lanrurugi-gpu-worker not built — run `mise run build` first \
             (a `cargo test -p lanrurugi-gpu-worker` run produces only a hashed test-harness binary)"
        );
        return;
    }
    if !model_dir_available() {
        eprintln!("skipping: no inpainting model directory resolvable");
        return;
    }

    init_tracing();
    eprintln!("environment the worker will inherit:");
    dump_environment();

    // One client for both cases: two `GpuWorkerClient`s would race for the same socket path and each
    // spawn its own worker process.
    let client = GpuWorkerClient::new(WorkerKind::Image);

    // --- The case the feature exists for -----------------------------------------------------
    let (w, h) = OVER_LIMIT;
    assert!(
        !lanrurugi_gpu_ipc::erase_request_provably_fits(w, h),
        "this test proves nothing unless {w}x{h} is past the boundary's limit at full resolution"
    );
    let (ww, wh) = lanrurugi_inpaint::work_dims(w, h);
    let reduced = lanrurugi_gpu_ipc::erase_request_bytes(ww, wh);
    assert!(
        lanrurugi_gpu_ipc::erase_request_provably_fits(ww, wh),
        "the reduced request must fit"
    );
    eprintln!(
        "over-limit {w}x{h}: full-resolution request {} bytes (refused) -> reduced to {ww}x{wh}, \
         {reduced} bytes",
        lanrurugi_gpu_ipc::erase_request_bytes(w, h)
    );

    let bars = bars_for(w, h);
    let page = synthetic_page(w, h, &bars);
    let original = page.rgb.clone();
    let model_mask = mask_from_bars(w, h, &bars, 12);
    // The paste mask is the bars themselves, deliberately *narrower* than the model mask — the same
    // model-mask-wide / paste-mask-tight split production uses.
    let paste_mask = mask_from_bars(w, h, &bars, 0);

    let started = Instant::now();
    let erased = client
        .erase_page(page, model_mask, paste_mask.clone())
        .await
        .expect(
            "erase_page must succeed for a page the reduced path can carry — before this feature \
             the same call was refused by the codec and the page was silently left untranslated",
        );
    eprintln!("  erase_page({w}x{h}) succeeded in {:?}", started.elapsed());

    assert_eq!(
        (erased.width, erased.height),
        (w, h),
        "the returned page must be at the original dimensions"
    );
    assert_eq!(erased.rgb.len(), (w as usize) * (h as usize) * 3);

    let mut masked_changed = 0usize;
    for i in 0..(w as usize) * (h as usize) {
        let changed = erased.rgb[i * 3..i * 3 + 3] != original[i * 3..i * 3 + 3];
        if paste_mask.mask[i] {
            if changed {
                masked_changed += 1;
            }
        } else if changed {
            panic!(
                "pixel {i} is outside the paste mask but changed — the composite must leave \
                 everything the mask does not cover as the original page's own pixels"
            );
        }
    }
    assert!(
        masked_changed > 0,
        "nothing under the paste mask changed, so the check above proved nothing about the erased \
         region"
    );
    eprintln!("  {masked_changed} pixels under the paste mask were rewritten by the model");

    // --- The case that must keep working unchanged -------------------------------------------
    let (w, h) = FITTING;
    assert!(
        lanrurugi_gpu_ipc::erase_request_provably_fits(w, h),
        "{w}x{h} is meant to be a fitting page"
    );
    let bars = bars_for(w, h);
    let page = synthetic_page(w, h, &bars);
    let original = page.rgb.clone();
    let paste_mask = mask_from_bars(w, h, &bars, 0);

    let started = Instant::now();
    let erased = client
        .erase_page(page, mask_from_bars(w, h, &bars, 12), paste_mask.clone())
        .await
        .expect("a fitting page must still erase, through the pre-existing full-resolution path");
    eprintln!("  erase_page({w}x{h}) succeeded in {:?}", started.elapsed());

    assert_eq!((erased.width, erased.height), (w, h));
    for i in 0..(w as usize) * (h as usize) {
        if !paste_mask.mask[i] {
            assert_eq!(
                erased.rgb[i * 3..i * 3 + 3],
                original[i * 3..i * 3 + 3],
                "pixel {i} outside the paste mask changed on the full-resolution path too"
            );
        }
    }
}
