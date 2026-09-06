//! In-process microbenchmarks (T085, research.md #11): isolated hashing throughput and thumbnail
//! decode/resize costs, the two CPU-bound operations FR-022 requires to scale with available
//! cores. These run fast and standalone (no legacy instance, no server boot needed), unlike the
//! `compare` cross-system harness, so they're suited to running continuously during development.

use std::io::Write;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lanrurugi_storage::id::size_aware_id;

fn make_temp_file(size: usize) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let chunk = vec![0xABu8; 8192];
    let mut written = 0;
    while written < size {
        let n = chunk.len().min(size - written);
        file.write_all(&chunk[..n]).unwrap();
        written += n;
    }
    file.flush().unwrap();
    file
}

fn bench_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("size_aware_id");
    // 512_000 is the exact sample size the algorithm reads (research.md #1) — below and above it
    // exercise, respectively, "whole file fits in the sample" and "file size read separately from
    // a truncated sample" code paths.
    for size in [64 * 1024, 512_000, 5_000_000] {
        let file = make_temp_file(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| size_aware_id(file.path()).unwrap());
        });
    }
    group.finish();
}

fn make_test_archive_with_image() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
    let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    writer.start_file("page1.jpg", options).unwrap();

    let img = image::RgbImage::from_pixel(3000, 4000, image::Rgb([120, 60, 200]));
    let mut jpeg_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut jpeg_bytes),
        image::ImageFormat::Jpeg,
    )
    .unwrap();
    writer.write_all(&jpeg_bytes).unwrap();
    writer.finish().unwrap();
    file
}

fn bench_thumbnail(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let archive = make_test_archive_with_image();
    let out_dir = tempfile::tempdir().unwrap();

    c.bench_function("thumbnail_decode_resize", |b| {
        b.to_async(&runtime).iter(|| {
            let archive_path = archive.path().to_path_buf();
            let output = out_dir.path().join("thumb.jpg");
            async move {
                lanrurugi_scanner::thumbnail::generate(
                    archive_path,
                    1,
                    output,
                    lanrurugi_scanner::thumbnail::ThumbFormat::Jpeg,
                    lanrurugi_scanner::thumbnail::JPEG_QUALITY_NORMAL,
                )
                .await
                .unwrap();
            }
        });
    });
}

criterion_group!(benches, bench_hashing, bench_thumbnail);
criterion_main!(benches);
