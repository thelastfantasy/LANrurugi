//! Byte- and record-accurate progress for a backup or an export, shared by the job runners.
//!
//! Two very different things are being measured, so the sinks are fed from two places: [`copy_rdb`]
//! and the `BGSAVE` sampler report **bytes** (the RDB's size is knowable, or at least estimable),
//! while [`write_document`] reports **records serialized** (a JSON document's final size is not
//! knowable before it is written, but the record count is). Both end at exactly 100.
//!
//! [`copy_rdb`]: super::copy_rdb

use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde::Serialize;

use lanrurugi_backup::build::BackupDocument;

/// How often a sink is told about a change. The UI polls the stream at 250 ms and shows one decimal,
/// so anything faster is wasted work; anything slower makes the bar look stuck.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(150);

/// Where a running backup reports how far it has got.
///
/// Implementations must keep `percent` monotonic themselves — the sources can legitimately report a
/// smaller number (a re-estimate of the expected total), and a bar that goes backwards is worse than
/// one that pauses.
pub trait BackupProgress: Send + Sync {
    /// `written` / `expected` are bytes (`expected` is `0` while unknown), `percent` is 0–100.
    fn update(&self, written: u64, expected: u64, percent: f64);
}

/// The sink for work nobody is watching: the schedule's own firings and the `pre-*` exports.
pub struct NoProgress;

impl BackupProgress for NoProgress {
    fn update(&self, _written: u64, _expected: u64, _percent: f64) {}
}

/// A writer that counts what it has written. The document serializer wraps the real file writer in
/// this so every record it emits advances one counter.
struct Counted<'a, W: Write> {
    inner: &'a mut W,
    bytes: u64,
}

impl<W: Write> Write for Counted<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct ExportState {
    total: u64,
    done: u64,
    last_sample: Instant,
}

/// Serializes `doc` as the exact JSON `serde_json::to_writer(doc)` would produce, one record at a
/// time, reporting progress per record.
///
/// Hand-rolled because `serde_json` serializes the whole struct in one call with no hook between
/// records — which is exactly the granularity the UI needs for a large library. The field order and
/// separators deliberately match what the derive emits, so the streamed bytes are byte-identical to
/// the single-call form (a test asserts it).
pub fn write_document<W: Write>(
    out: &mut W,
    doc: &BackupDocument,
    progress: &dyn BackupProgress,
) -> io::Result<()> {
    let total = (doc.archives.len()
        + doc.categories.len()
        + doc.tankoubons.len()
        + doc.stamps.len()
        + doc.bookmarks.len()
        + doc.terminology_glossaries.len()
        + doc.volume_font_patterns.len()
        + doc.text_regions.len()) as u64;
    let mut counted = Counted {
        inner: out,
        bytes: 0,
    };
    let mut state = ExportState {
        total,
        done: 0,
        last_sample: Instant::now(),
    };
    let mut first = true;

    write_section(
        &mut counted,
        "archives",
        &doc.archives,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "categories",
        &doc.categories,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "tankoubons",
        &doc.tankoubons,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "stamps",
        &doc.stamps,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "bookmarks",
        &doc.bookmarks,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "terminology_glossaries",
        &doc.terminology_glossaries,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "volume_font_patterns",
        &doc.volume_font_patterns,
        &mut first,
        &mut state,
        progress,
    )?;
    write_section(
        &mut counted,
        "text_regions",
        &doc.text_regions,
        &mut first,
        &mut state,
        progress,
    )?;
    // `share_node_secret` is `skip_serializing_if = "Option::is_none"` in the derived form.
    if let Some(secret) = &doc.share_node_secret {
        counted.write_all(b",\"share_node_secret\":")?;
        serde_json::to_writer(&mut counted, secret).map_err(io::Error::other)?;
    }
    counted.write_all(b"}")?;

    // The whole document is on disk (well, in the caller's buffer): the byte total is now exact.
    progress.update(counted.bytes, counted.bytes, 100.0);
    Ok(())
}

fn write_section<W, T>(
    out: &mut Counted<'_, W>,
    name: &str,
    items: &[T],
    first: &mut bool,
    state: &mut ExportState,
    progress: &dyn BackupProgress,
) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    if *first {
        out.write_all(b"{\"")?;
        *first = false;
    } else {
        out.write_all(b",\"")?;
    }
    out.write_all(name.as_bytes())?;
    out.write_all(b"\":[")?;
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(&mut *out, item).map_err(io::Error::other)?;
        state.done += 1;
        if state.last_sample.elapsed() >= SAMPLE_INTERVAL || state.done == state.total {
            // The total size is unknowable up front, so `expected` is this writer's own running
            // estimate (bytes so far scaled to the whole record count). It converges exactly at the
            // end; `percent` — the number the UI actually shows — is the real one.
            let expected = out
                .bytes
                .saturating_mul(state.total)
                .checked_div(state.done)
                .unwrap_or(0);
            let percent = if state.total == 0 {
                100.0
            } else {
                state.done as f64 / state.total as f64 * 100.0
            };
            progress.update(out.bytes, expected, percent.min(99.9));
            state.last_sample = Instant::now();
        }
    }
    out.write_all(b"]")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    struct Recorder {
        samples: Mutex<Vec<(u64, u64, f64)>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                samples: Mutex::new(Vec::new()),
            }
        }
    }

    impl BackupProgress for Recorder {
        fn update(&self, written: u64, expected: u64, percent: f64) {
            self.samples
                .lock()
                .expect("test sink lock")
                .push((written, expected, percent));
        }
    }

    fn document(archives: usize) -> BackupDocument {
        BackupDocument {
            archives: (0..archives)
                .map(|i| lanrurugi_backup::build::BackupArchive {
                    arcid: format!("a{i:04}"),
                    title: format!("title {i}"),
                    tags: "artist:x".to_string(),
                    summary: None,
                    thumbhash: None,
                    filename: format!("a{i:04}.zip"),
                })
                .collect(),
            categories: vec![],
            tankoubons: vec![],
            stamps: vec![],
            bookmarks: vec![],
            terminology_glossaries: vec![],
            volume_font_patterns: vec![],
            text_regions: vec![],
            share_node_secret: None,
        }
    }

    #[test]
    fn the_streamed_document_is_byte_identical_to_the_single_call_form() {
        let doc = document(3);
        let expected = serde_json::to_vec(&doc).unwrap();
        let mut written = Vec::new();
        write_document(&mut written, &doc, &NoProgress).unwrap();
        assert_eq!(written, expected);
    }

    #[test]
    fn the_streamed_document_matches_with_a_share_secret_too() {
        let mut doc = document(1);
        doc.share_node_secret = Some("secret".to_string());
        let expected = serde_json::to_vec(&doc).unwrap();
        let mut written = Vec::new();
        write_document(&mut written, &doc, &NoProgress).unwrap();
        assert_eq!(written, expected);

        let empty = document(0);
        let expected = serde_json::to_vec(&empty).unwrap();
        let mut written = Vec::new();
        write_document(&mut written, &empty, &NoProgress).unwrap();
        assert_eq!(written, expected);
    }

    /// The bar only goes one way, and it ends at 100 even for a big document.
    #[test]
    fn progress_is_monotonic_and_ends_at_one_hundred() {
        let recorder = Recorder::new();
        let mut written = Vec::new();
        write_document(&mut written, &document(500), &recorder).unwrap();
        let samples = recorder.samples.lock().unwrap();
        assert!(
            samples.len() > 1,
            "a 500-record document samples more than once"
        );
        let mut previous = -1.0f64;
        for (_, _, percent) in samples.iter() {
            assert!(
                *percent >= previous,
                "progress went backwards: {previous} -> {percent}"
            );
            previous = *percent;
        }
        assert_eq!(samples.last().unwrap().2, 100.0, "the last sample is 100%");
        let (bytes, expected, _) = samples.last().unwrap();
        assert_eq!(bytes, expected, "the byte estimate is exact at the end");
    }
}
