//! Runs detection + recognition on one page image and prints exactly what survived, without a
//! server, a database or a redeploy.
//!
//! Written because every OCR-quality question ("why did this bubble come back empty?") otherwise
//! needed a full 20-minute `dev-rebuild` before its answer could be observed at all. This runs the
//! same `lanrurugi_ocr::batch::run_batch` the server calls, so the crop/padding/axis decisions under
//! test are the real ones.
//!
//! Usage: `mise run debug-ocr-page -- <page.jpg|page.png> [expected-text]`
//!
//! Exit code is 0 regardless of what OCR finds — this is an inspection tool, not a gate.

use std::path::PathBuf;

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::PageNumber;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(true)
        .without_time()
        .init();

    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: debug_ocr_page <page image> [expected text]");
        std::process::exit(2);
    };
    let expected = args.next();

    let image = match load_page(&PathBuf::from(&path)) {
        Ok(image) => image,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            std::process::exit(2);
        }
    };
    println!("page: {path} ({}x{})", image.width(), image.height());

    // The same two components `translation_pipeline::build_handles` builds: a CPU text detector and
    // the recognize worker (a subprocess, exactly as in production).
    let paths = match lanrurugi_ocr::model_discovery::ModelPaths::discover() {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("model discovery failed: {e} (set LANRURUGI_MANGA_OCR_MODEL_DIR)");
            std::process::exit(2);
        }
    };
    let detector = match lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_ocr::detect::TextDetector::load(&paths.detection)
    })
    .await
    {
        Ok(Ok(detector)) => detector,
        Ok(Err(e)) => {
            eprintln!("text detector failed to load: {e}");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("blocking task failed: {e}");
            std::process::exit(2);
        }
    };

    use lanrurugi_api::gpu_worker_client::{GpuWorkerClient, WorkerKind};
    let recognize_worker = std::sync::Arc::new(GpuWorkerClient::new(WorkerKind::Recognize));
    tokio::spawn(std::sync::Arc::clone(&recognize_worker).run_idle_reaper());

    let engine = std::sync::Arc::new(lanrurugi_ocr::batch::OcrEngine::new(
        detector,
        recognize_worker as std::sync::Arc<dyn lanrurugi_ocr::batch::TextRecognizerHandle>,
    ));

    let page = lanrurugi_ocr::batch::PageInput {
        archive_id: ArchiveId("0".repeat(40)),
        page_number: PageNumber(1),
        is_cover: false,
        image,
        bubbles: None,
    };

    let started = std::time::Instant::now();
    let results = match lanrurugi_ocr::batch::run_batch(engine, vec![page]).await {
        Ok(results) => results,
        Err(e) => {
            eprintln!("run_batch failed: {e}");
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed();

    let Some(result) = results.into_iter().next() else {
        println!("no result");
        return;
    };

    println!(
        "\n{} region(s) kept in {:.1}s{}",
        result.regions.len(),
        elapsed.as_secs_f32(),
        if result.degraded_by_infrastructure_failure {
            " (degraded: an infrastructure failure dropped boxes)"
        } else {
            ""
        }
    );
    for region in &result.regions {
        println!(
            "  {:<40} {:?}",
            region.source_text.replace('\n', "⏎"),
            region.bounding_box
        );
    }

    if let Some(expected) = expected {
        let found = result
            .regions
            .iter()
            .any(|region| region.source_text.contains(&expected));
        println!(
            "\nexpected text {:?}: {}",
            expected,
            if found { "FOUND" } else { "MISSING" }
        );
        if !found {
            std::process::exit(3);
        }
    }
}

fn load_page(path: &PathBuf) -> Result<image::RgbImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let decoded = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    Ok(decoded.to_rgb8())
}
