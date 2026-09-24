//! Synthetic-library generator (T084): produces a reproducible test library at the SC-008 target
//! scale (~100,000 archives), or a smaller `--archive-count` for fast local runs, so US8's
//! benchmarks and the cross-system comparison harness have a realistic-enough, controllable
//! workload without needing an actual pre-existing 100k-archive manga collection on hand.
//!
//! Each archive's pages are a JPEG whose pixel content is derived from that archive's index (see
//! `page_jpeg_bytes`) rather than one shared image, plus a per-archive index marker entry — so
//! neither the leading bytes nor the total size are identical across archives, matching how a
//! real library (distinct works, distinct cover art, distinct page counts) actually looks to the
//! scanner.

use std::io::Write;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub output_dir: PathBuf,
    pub archive_count: usize,
    /// Base page count per archive; actual count varies slightly per archive (`base + i % 5`) so
    /// not every generated file is byte-identical in size.
    pub pages_per_archive: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GenerateSummary {
    pub archive_count: usize,
    pub total_size_bytes: u64,
}

/// 64-bit finalizer (splitmix64/MurmurHash3-style bit mixing) — full avalanche, no short cycles,
/// unlike a naive `index * odd_constant % 256` (which is a bijection mod 256 and so repeats with
/// period 256 — exactly the bug that motivated this comment; see the module's regression test).
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

/// Encodes a page image whose pixel content *and dimensions* are derived from `index`, so
/// distinct archives don't share identical leading bytes. This matters a great deal here: the
/// *legacy* hash algorithm this benchmark drives for comparison only samples a file's first
/// 512,000 bytes (research.md #1), so if every archive's first page were byte-identical — as a
/// single shared image across all archives would produce — legacy would false-merge nearly the
/// entire synthetic library into one archive record, the exact defect this rewrite fixes
/// (US2/US3). That would silently wreck the benchmark's premise (measuring wall-clock to
/// catalogue N *distinct* archives) rather than exercising it, since legacy would never report
/// more than a couple of distinct IDs no matter how long a scan runs. Real manga volumes each
/// have distinct cover art for the same reason.
fn page_jpeg_bytes(index: usize) -> Vec<u8> {
    let h = mix64(index as u64);
    let r = (h & 0xFF) as u8;
    let g = ((h >> 8) & 0xFF) as u8;
    let b = ((h >> 16) & 0xFF) as u8;
    let width = 800 + ((h >> 24) % 64) as u32;
    let height = 1200 + ((h >> 32) % 64) as u32;
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([r, g, b]));
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Jpeg,
    )
    .expect("encoding the sample page image must not fail");
    bytes
}

fn write_one_archive(path: &Path, index: usize, page_count: usize) -> std::io::Result<u64> {
    let jpeg = page_jpeg_bytes(index);
    let file = std::fs::File::create(path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for page in 0..page_count {
        writer
            .start_file(format!("page{page:04}.jpg"), options)
            .map_err(std::io::Error::other)?;
        writer.write_all(&jpeg)?;
    }
    writer
        .start_file("index.txt", options)
        .map_err(std::io::Error::other)?;
    write!(writer, "{index}")?;
    writer.finish().map_err(std::io::Error::other)?;

    Ok(std::fs::metadata(path)?.len())
}

/// Generates `config.archive_count` archives under `config.output_dir`, in parallel across
/// available cores (rayon — the same bridging pattern the real scanner uses for bulk hashing,
/// constitution Principle III) since generating ~100k zip files serially would itself become the
/// bottleneck for a "fast local run".
pub fn generate(config: &GenerateConfig) -> anyhow::Result<GenerateSummary> {
    std::fs::create_dir_all(&config.output_dir)?;

    let total_size_bytes: u64 = (0..config.archive_count)
        .into_par_iter()
        .map(|i| -> anyhow::Result<u64> {
            let page_count = config.pages_per_archive + (i % 5);
            let path = config
                .output_dir
                .join(format!("Synthetic Archive {i:06}.zip"));
            Ok(write_one_archive(&path, i, page_count)?)
        })
        .try_reduce(|| 0, |a, b| Ok(a + b))?;

    Ok(GenerateSummary {
        archive_count: config.archive_count,
        total_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_the_requested_number_of_distinctly_sized_archives() {
        let dir = tempfile::tempdir().unwrap();
        let config = GenerateConfig {
            output_dir: dir.path().to_path_buf(),
            archive_count: 12,
            pages_per_archive: 3,
        };

        let summary = generate(&config).unwrap();
        assert_eq!(summary.archive_count, 12);
        assert!(summary.total_size_bytes > 0);

        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 12);

        // Page-count variation (base + i % 5) should produce at least two distinct file sizes
        // across 12 archives, confirming these aren't all byte-identical stamps of one template.
        let sizes: std::collections::HashSet<u64> = entries
            .into_iter()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .collect();
        assert!(sizes.len() > 1);
    }

    #[test]
    fn distinct_archives_have_distinct_leading_512000_bytes() {
        // Regression guard for the false-merge trap this generator fell into *twice*: first with
        // one shared image across every archive, then again with a naive
        // `index * odd_constant % 256` pixel derivation that turned out to be a bijection mod
        // 256 — i.e. it silently repeated with period 256. 2000 (not 20) is the point: this test
        // originally used a sample too small to span even one period, so it passed while the bug
        // was still live. Verified against the real difegue/lanraragi image: this exact
        // scale/config combination produced only 481 distinct legacy archive records out of 2000
        // files before the `mix64` fix.
        const COUNT: usize = 2000;
        let dir = tempfile::tempdir().unwrap();
        let config = GenerateConfig {
            output_dir: dir.path().to_path_buf(),
            archive_count: COUNT,
            pages_per_archive: 20,
        };
        generate(&config).unwrap();

        let mut leading_prefixes = std::collections::HashSet::new();
        for i in 0..COUNT {
            let path = dir.path().join(format!("Synthetic Archive {i:06}.zip"));
            let bytes = std::fs::read(&path).unwrap();
            let prefix_len = bytes.len().min(512_000);
            leading_prefixes.insert(bytes[..prefix_len].to_vec());
        }
        assert_eq!(leading_prefixes.len(), COUNT);
    }
}
