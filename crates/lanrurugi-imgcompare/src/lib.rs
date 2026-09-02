//! Image-quality comparison between two archives believed to be different versions of the same
//! work (issue #77 — currently only reachable via the download-queue filename-conflict path, but
//! designed to also cover a local-upload conflict against an existing library archive per the
//! issue's own design comments).
//!
//! Three-stage pipeline, per the issue's own confirmed design:
//! 1. [`alignment::hash_page`] every page of both archives (perceptual hash — [`alignment`]'s own
//!    docs cover why this, not a heavier feature-matching approach, is the right tool here) and
//!    [`alignment::align_sequences`] them (banded DP — handles differing page counts without
//!    assuming 1:1 index correspondence).
//! 2. [`sharpness::laplacian_variance`] each aligned pair to compare *real* sharpness, not raw
//!    resolution/file size (issue's own motivating case: a large-resolution scan that's actually
//!    blurry/thick-lined and unreadable).
//! 3. Roll the per-pair sharpness comparisons up into an overall recommendation, deferring to the
//!    caller (not decided here — see [`Recommendation`]'s own docs) when confidence is too low to
//!    trust an automatic pick.

pub mod alignment;
pub mod crop_align;
pub mod sharpness;

use std::path::Path;

use image::DynamicImage;
use lanrurugi_scanner::archive_format;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Caps CPU-bound rayon work at ~30% of cores instead of the uncapped global pool — same figure
/// as `lanrurugi-api::recommend_precompute::precompute_worker_budget` (duplicated, not shared:
/// this crate can't depend on `lanrurugi-api`). No `LoadThrottle`-style live backoff here — that's
/// for multi-minute background jobs, not a single interactive comparison.
fn worker_budget() -> usize {
    (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 3
        / 10)
        .max(1)
}

fn with_capped_parallelism<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(worker_budget())
        .build()
        .expect("rayon pool build")
        .install(f)
}

#[derive(Debug, Error)]
pub enum ImgCompareError {
    #[error("archive read error: {0}")]
    Archive(#[from] archive_format::ArchiveFormatError),
    #[error("image decode error for {entry:?}: {source}")]
    Decode {
        entry: String,
        #[source]
        source: image::ImageError,
    },
    #[error("archive has no readable pages")]
    NoPages,
    #[error("comparison cancelled by caller")]
    Cancelled,
}

type Result<T> = std::result::Result<T, ImgCompareError>;

/// One aligned page pair's own sharpness comparison — the raw material both the automatic
/// recommendation and (when confidence is too low) the human-judgment fallback UI are built from.
/// Width/height come for free from the same decoded image the sharpness pass already reads
/// (no extra decode) — surfaced so the fallback UI can show each sample's real resolution
/// alongside sharpness, since a higher pixel count alone doesn't imply a sharper scan (the
/// motivating case this crate exists for: `sharpness.rs`'s own docs on a large-resolution-but-
/// blurry scan).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageComparison {
    pub a_page_index: usize,
    pub b_page_index: usize,
    /// This page's own filename (e.g. `"012.jpg"`) — not `ComparisonResult::a_filename`, which is
    /// the whole archive's filename.
    pub a_filename: String,
    pub b_filename: String,
    /// This page's own compressed byte size inside the archive, not the decoded pixel buffer size.
    pub a_file_size: u64,
    pub b_file_size: u64,
    pub a_sharpness: f64,
    pub b_sharpness: f64,
    pub a_width: u32,
    pub a_height: u32,
    pub b_width: u32,
    pub b_height: u32,
    /// Maps a point in A's pixel space to the corresponding point in B's, for cases where the two
    /// scans have a different crop margin and/or resolution (a real, common scenario — see
    /// `crop_align`'s own module docs). Computed only for sampled pairs (not every aligned pair —
    /// see `estimate_crop_alignment`'s own call site for why), so the frontend magnifier can sample
    /// the same underlying content on both sides instead of the same raw pixel coordinate.
    pub crop_alignment: crop_align::CropAlignment,
}

/// Which side an automatic or human judgment favors — deliberately symmetric (`A`/`B`, not
/// "old"/"new") since this crate doesn't know or care which side is the freshly-downloaded file
/// and which is the existing library archive; that framing is the caller's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    A,
    B,
}

/// The overall result of comparing two archives. `recommendation` is `None` when too few pages
/// could be confidently aligned to trust an automatic pick (per issue #77's own confirmed design:
/// low confidence must never produce a guess — it falls back to showing the caller a handful of
/// sample pairs for a human to judge, rather than silently picking one side).
///
/// `likely_different_language` is a SEPARATE signal from `recommendation` being `None` — the two
/// are different situations the caller needs to present differently: "same-language quality
/// conflict, but too close to call" (show the manual sample-comparison fallback) vs. "this isn't
/// really a quality conflict at all, it's probably two legitimate language editions of the same
/// work" (per issue #77's own confirmed design: don't offer a "keep A or B" pick in this case at
/// all, just surface that these might be different-language editions the user may want to keep
/// both of). Confirmed live against a real same-work cross-language pair (issue #77): near-total
/// page-count-preserving alignment (176 of 180 pages on both sides) is exactly what a translated
/// edition looks like — the art/panel layout is identical, only in-bubble text differs — which is
/// a fundamentally different shape from "two truly different scan quality levels of the same
/// print run" (which typically has SOME page-count drift from scan artifacts/front-matter
/// differences, not a near-perfect page-for-page match).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonResult {
    pub aligned_pairs: usize,
    pub a_total_pages: usize,
    pub b_total_pages: usize,
    /// Every entry (files *and* directories, including empty ones) inside each archive — a strict
    /// superset of the page list whenever the archive bundles non-image files alongside its pages
    /// (issue #77's own follow-on design: "有时候里面也可能有txt或torrent文件啥的", i.e. a readme
    /// or a stray `.torrent` a downloader left behind — confirmed design: surface the real archive
    /// structure as a tree the user can inspect via a popover next to each filename, not just a
    /// bare page-vs-file count). Each entry's own `is_page` flag is what the frontend uses to tell
    /// a real page apart from other bundled content within that same tree.
    pub a_entries: Vec<archive_format::ArchiveEntryInfo>,
    pub b_entries: Vec<archive_format::ArchiveEntryInfo>,
    pub likely_different_language: bool,
    pub recommendation: Option<Side>,
    /// Up to [`SAMPLE_SIZE`] aligned pairs — the material for either confirming the automatic
    /// recommendation to the user or, when `recommendation` is `None`, driving the manual
    /// pick-per-pair fallback UI.
    pub samples: Vec<PageComparison>,
    /// Display name + whole-file size for each side — this crate has no opinion on what "A"/"B"
    /// mean (see [`Side`]'s own docs), so the caller (`download_queue.rs`'s
    /// `compare_queue_item`) supplies these rather than this crate trying to derive a filename
    /// from a bare `Path`, which for the staged/uncatalogued A side wouldn't even be the
    /// user-meaningful original filename (the on-disk temp file is `temp_{crc32}_{filename}`).
    pub a_filename: String,
    pub b_filename: String,
    pub a_file_size: u64,
    pub b_file_size: u64,
    /// Pages in A that [`alignment::align_sequences`] found no counterpart for in B — every one of
    /// A's own pages when B is a different-language edition doesn't apply here (that alignment is
    /// near-total by definition, see `likely_different_language`'s own docs), so this is really
    /// "pages that might be genuinely unique to A" (a different print run's bonus content, extra
    /// panels, etc — issue #77's own follow-on design: "如果发现 A 版本比 B 版本多出一些页面...
    /// 用户可能想把 A 独有的这几页保留下来"). Empty whenever every A page found a match. The
    /// frontend reads each one's actual bytes via the existing `.../compare/page?side=a&index=N`
    /// endpoint (same `a_page_index` values `samples` already uses), and can package a selection
    /// of these into a `lanrurugi_scanner::patch` sidecar rather than this crate needing its own
    /// separate page-serving path.
    pub a_unmatched_pages: Vec<UnmatchedPage>,
    /// The `b`-side mirror of `a_unmatched_pages` — pages in B with no counterpart in A, for the
    /// symmetric "keep some of B's unique pages even when picking A overall" flow (issue #77's own
    /// follow-on design, confirmed to need both directions).
    pub b_unmatched_pages: Vec<UnmatchedPage>,
    /// `true` for a partial cache entry from a disconnected stream — only `samples` is real then,
    /// every other field is a placeholder. `#[serde(default)]` so old cache entries read as `false`.
    #[serde(default)]
    pub incomplete: bool,
}

/// One unmatched page plus a ready-made, no-AI-needed default for where a patch inserting it
/// should anchor — see [`alignment::default_insert_after_b_index`]'s own docs for why this is
/// derivable straight from the alignment DP's output. `default_insert_after` is the *other* side's
/// own page index (a `B` index for an `a_unmatched_pages` entry, an `A` index for a
/// `b_unmatched_pages` entry) to insert after; `None` means "insert at the very start" (no matched
/// page precedes this one at all). The frontend's drag-and-drop UI uses this as a starting
/// position a user can still drag elsewhere, not a constraint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UnmatchedPage {
    pub page_index: usize,
    pub default_insert_after: Option<usize>,
}

/// Alignment ratio (aligned pairs / total pages on the smaller side) above which two archives with
/// identical page counts are treated as likely different-language editions of the same work,
/// rather than a genuine quality-comparison candidate — see [`ComparisonResult`]'s own docs for
/// the real-data reasoning behind this threshold and why it's a separate signal from a low-
/// confidence `recommendation`.
const LANGUAGE_EDITION_ALIGNMENT_RATIO: f64 = 0.9;

/// Minimum aligned pairs before an automatic recommendation is even considered — with too few
/// data points, a majority could be a single lucky/unlucky pair rather than a real trend.
const MIN_PAIRS_FOR_RECOMMENDATION: usize = 3;
/// A recommendation is trusted once EITHER this fraction of the smaller side's total pages
/// aligned, OR [`MIN_ALIGNMENT_COUNT_FOR_RECOMMENDATION`] pairs aligned outright — an OR, not just
/// one absolute floor, because either check alone misses a real case: a short work (a handful of
/// pages) can legitimately clear a ratio bar while never reaching a large absolute count, and a
/// long work re-released as an updated/expanded edition (a scanlation group's own common practice
/// — re-uploading the same title with many new pages added) can legitimately clear a large
/// absolute count while its alignment ratio (computed against the smaller/older side's own total)
/// stays well above this ratio too, since every one of the older side's pages is still expected to
/// match — but a *shorter*, unrelated re-scan sharing only a handful of incidental blank/title
/// pages with a much longer, unrelated archive would clear neither bar, which is exactly the
/// coincidental-namesake case this guards against (`pairs.len() >= 3` with `pairs.len() /
/// total_pages` near zero — clearly not "these are the same work", but
/// `MIN_PAIRS_FOR_RECOMMENDATION` alone would wave it through).
const MIN_ALIGNMENT_RATIO_FOR_RECOMMENDATION: f64 = 0.5;
const MIN_ALIGNMENT_COUNT_FOR_RECOMMENDATION: usize = 20;
/// An automatic recommendation requires this fraction of compared pairs to agree, not just a bare
/// majority — a near-even split (e.g. 3 of 5) is exactly the low-confidence case that should defer
/// to the human-judgment fallback instead of asserting a pick.
const RECOMMENDATION_AGREEMENT_THRESHOLD: f64 = 0.7;
/// How many aligned pairs to sample for both the confidence check and the fallback UI, spread
/// evenly across the aligned range rather than always the first few pages. 6 (not 5) so the
/// frontend's 3-column sample grid fills evenly (3+3, not 3+2).
const SAMPLE_SIZE: usize = 6;

/// One update from a streaming [`compare_archives_streaming`] run — see that function's own docs
/// for the two-phase design. `on_event` may be called many times per run: once per sample in phase
/// 1 (`phase: Coarse`), then again for each sample whose coarse alignment indicated padding in
/// phase 2 (`phase: Precise`), then exactly once with [`CompareEvent::Done`] at the very end.
/// Deliberately an enum (not a bare `PageComparison`) so the transport layer (the SSE handler in
/// `lanrurugi-api`) has an explicit tag to serialize as the wire-level `phase`/`done` discriminator
/// the frontend switches on (issue #77 follow-on: "注意sse的数据里面要进行区分，用flag标明是粗结果
/// 还是精结果" / "并且sse要有结束标记") rather than the frontend needing to infer phase from
/// message order or a connection-close/timeout, which is fragile over a real network.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompareEvent {
    /// One sample pair is ready. `sample_index` is this sample's own position in the eventual
    /// `ComparisonResult.samples` array (stable across both phases — a `Precise` event for sample
    /// `i` always replaces the `Coarse` event's own sample `i`), not `a_page_index`/`b_page_index`
    /// (those can repeat across different samples only in principle, `sample_index` never does).
    #[serde(rename_all = "snake_case")]
    Sample {
        sample_index: usize,
        phase: ComparePhase,
        sample: PageComparison,
    },
    /// Emitted exactly once, after every `Sample` event (both phases) has already been sent —
    /// carries the same summary fields `ComparisonResult` itself has (everything except `samples`,
    /// which the frontend has already assembled incrementally from the `Sample` events above) so
    /// the caller never needs to wait for this event just to show the header/recommendation area.
    #[serde(rename_all = "snake_case")]
    Done {
        aligned_pairs: usize,
        a_total_pages: usize,
        b_total_pages: usize,
        a_entries: Vec<archive_format::ArchiveEntryInfo>,
        b_entries: Vec<archive_format::ArchiveEntryInfo>,
        likely_different_language: bool,
        recommendation: Option<Side>,
        a_filename: String,
        b_filename: String,
        a_file_size: u64,
        b_file_size: u64,
        a_unmatched_pages: Vec<UnmatchedPage>,
        b_unmatched_pages: Vec<UnmatchedPage>,
    },
}

/// Which pass produced a [`CompareEvent::Sample`] — see [`compare_archives_streaming`]'s own docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparePhase {
    /// The fast/coarse pipeline (perceptual-hash alignment + [`sharpness`] + the coarse
    /// [`crop_align::estimate_crop_alignment_with_confidence`]) — every sample gets exactly one of
    /// these, streamed as soon as it's ready so the caller can open its result view on the FIRST
    /// one rather than waiting for all [`SAMPLE_SIZE`].
    Coarse,
    /// A pixel-accurate replacement `crop_alignment` from
    /// [`crop_align::estimate_precise_crop_alignment_with_confidence`], for a sample whose coarse
    /// alignment already indicated real padding — every other field on the sample is identical to
    /// its own `Coarse` event (only `crop_alignment` differs), so the caller can treat this as an
    /// in-place field update rather than a whole new sample. Not every sample gets one of these —
    /// see the phase-2 loop's own doc comment for the "needs padding" test.
    Precise,
}

/// Compares two archive files page-by-page. Reads every page of both archives into memory to hash
/// them (see [`alignment::hash_page`]) — for typical manga-length archives (order of 200 pages)
/// this is the same cost class `thumbnail.rs`'s own per-page decode already pays elsewhere in this
/// codebase, not a new order of magnitude of work.
///
/// `a_filename`/`a_file_size`/`b_filename`/`b_file_size` are caller-supplied display metadata
/// (see [`ComparisonResult`]'s own docs for why this crate can't derive them itself) — passed
/// through into the result unchanged, not read from `a_path`/`b_path` again here.
///
/// This is the original, non-streaming entry point — still used by
/// `crates/lanrurugi-imgcompare/examples/compare_two.rs` and anywhere a single synchronous
/// `ComparisonResult` is all that's needed. Implemented as a thin wrapper over
/// [`compare_archives_streaming`] (collecting its events into one final result) so the two never
/// drift apart — see that function's own docs for the two-phase pipeline this now actually runs.
#[allow(clippy::too_many_arguments)]
pub fn compare_archives(
    a_path: &Path,
    b_path: &Path,
    a_filename: String,
    a_file_size: u64,
    b_filename: String,
    b_file_size: u64,
) -> Result<ComparisonResult> {
    let mut samples: Vec<Option<PageComparison>> = Vec::new();
    let mut done: Option<ComparisonResult> = None;
    compare_archives_streaming(
        a_path,
        b_path,
        a_filename,
        a_file_size,
        b_filename,
        b_file_size,
        |event| {
            match event {
                CompareEvent::Sample {
                    sample_index,
                    sample,
                    ..
                } => {
                    if samples.len() <= sample_index {
                        samples.resize(sample_index + 1, None);
                    }
                    samples[sample_index] = Some(sample);
                }
                CompareEvent::Done {
                    aligned_pairs,
                    a_total_pages,
                    b_total_pages,
                    a_entries,
                    b_entries,
                    likely_different_language,
                    recommendation,
                    a_filename,
                    b_filename,
                    a_file_size,
                    b_file_size,
                    a_unmatched_pages,
                    b_unmatched_pages,
                } => {
                    done = Some(ComparisonResult {
                        aligned_pairs,
                        a_total_pages,
                        b_total_pages,
                        a_entries,
                        b_entries,
                        likely_different_language,
                        recommendation,
                        samples: samples.drain(..).flatten().collect(),
                        a_filename,
                        b_filename,
                        a_file_size,
                        b_file_size,
                        a_unmatched_pages,
                        b_unmatched_pages,
                        incomplete: false,
                    });
                }
            }
            // No external listener to disconnect — this synchronous wrapper always continues.
            true
        },
    )?;
    Ok(done.expect(
        "compare_archives_streaming always emits exactly one Done event before returning Ok",
    ))
}

/// The streaming pipeline behind [`compare_archives`] — same three-stage algorithm (see this
/// module's own top-level docs) through alignment, but the crop-alignment stage is split into two
/// phases per issue #77's own confirmed design, calling `on_event` as each piece becomes ready
/// instead of only returning once, at the very end:
///
/// - **Phase 1 (coarse)**: for each of the (up to [`SAMPLE_SIZE`]) sampled pairs, compute its
///   `crop_alignment` with the fast [`crop_align::estimate_crop_alignment_with_confidence`] (the
///   SAME algorithm — and, critically, the same calibration — `alignment::align_sequences_with_rescue`'s
///   own rescue closure already used above to align the pages in the first place) and emit it
///   immediately as `CompareEvent::Sample { phase: Coarse, .. }`. This is what lets a caller (the
///   SSE handler) open its result view on the very FIRST sample rather than waiting for all of
///   them — confirmed live design: "假设有6对结果（粗加工），可以用sse分6次发，发第一对的时候就开始
///   显示modal,这样是一个很大的提速". A sample whose coarse alignment already indicates real
///   padding is rendered as a real (if slightly imprecise) synthetic border immediately — "粗结果
///   如果有需要也是要渲染canvas的" / "不是等精确结果到了之后才开始渲染" — not left blank until phase 2.
/// - **Phase 2 (precise)**: only after every phase-1 sample has been sent, for each one whose
///   coarse alignment is non-identity (`crop_alignment != CropAlignment::IDENTITY` — the same
///   signal the frontend's own `needsSyntheticPad` uses), re-run the slower, pixel-accurate
///   [`crop_align::estimate_precise_crop_alignment_with_confidence`] and emit a REPLACEMENT
///   `CompareEvent::Sample { phase: Precise, .. }` carrying the same `sample_index` — "也就是说需要
///   渲染两次". A sample that was already identity in phase 1 never gets a phase-2 event at all
///   (nothing to refine).
/// - Finally, exactly one `CompareEvent::Done` — see that variant's own docs.
///
/// Runs synchronously on the calling thread (the caller — `download_queue.rs`'s SSE handler — is
/// expected to run this inside `spawn_blocking`, same as the non-streaming `compare_archives`
/// already does) and returns only after every event (including `Done`) has been handed to
/// `on_event`.
///
/// `on_event` returns `bool` — `false` means the caller is no longer listening (the SSE handler's
/// own `tx.send` failed because the client disconnected) and this function stops sampling further
/// events, returning `Err(ImgCompareError::Cancelled)` instead of finishing the run to no one.
/// Checked only between phase-1/phase-2 sample emissions, not mid-stage inside `load_pages`/
/// hashing/alignment (those run once over the FULL page set regardless of sample count, so most of
/// the real CPU cost is already sunk before the first `on_event` call fires — this check targets
/// the actual per-sample cost that scales with how far into streaming a cancellation lands, not an
/// unrealistic goal of instant abort at arbitrary points in already-parallelized bulk stages).
#[allow(clippy::too_many_arguments)]
pub fn compare_archives_streaming(
    a_path: &Path,
    b_path: &Path,
    a_filename: String,
    a_file_size: u64,
    b_filename: String,
    b_file_size: u64,
    mut on_event: impl FnMut(CompareEvent) -> bool,
) -> Result<()> {
    // Stage-by-stage timing (reported live: "把比较耗时的操作都用日志记录下来，我要看看执行了
    // 多少次") — this is the whole pipeline `compare_queue_item` wraps in one `spawn_blocking`, so
    // any one of these stages dominating (or a stage's cost not matching its expected O(n) vs.
    // O(n²) shape) is otherwise invisible from outside a debugger.
    let t_list = std::time::Instant::now();
    let a_pages = archive_format::list_pages(a_path)?;
    let b_pages = archive_format::list_pages(b_path)?;
    if a_pages.is_empty() || b_pages.is_empty() {
        return Err(ImgCompareError::NoPages);
    }
    // Cheap header-only walks (no file bytes read) — see `list_all_entries`'s own docs for why
    // this is a separate, richer structure than the page list whenever the archive bundles a
    // readme/torrent/etc alongside its actual pages.
    let a_entries = archive_format::list_all_entries(a_path)?;
    let b_entries = archive_format::list_all_entries(b_path)?;
    tracing::info!(
        a_pages = a_pages.len(),
        b_pages = b_pages.len(),
        a_entries = a_entries.len(),
        b_entries = b_entries.len(),
        elapsed_ms = t_list.elapsed().as_millis(),
        "compare_archives: list_pages + list_all_entries (both sides) done"
    );

    let t_load = std::time::Instant::now();
    let a_images = load_pages(a_path, &a_pages)?;
    let b_images = load_pages(b_path, &b_pages)?;
    tracing::info!(
        a_pages = a_pages.len(),
        b_pages = b_pages.len(),
        elapsed_ms = t_load.elapsed().as_millis(),
        "compare_archives: load_pages (decode, both sides) done"
    );

    // Was the largest stage in the pipeline (3059ms of ~4000ms for 188+185 pages) running serial;
    // parallelized via the same capped pool `load_pages` uses.
    let t_hash = std::time::Instant::now();
    let (a_hashes, b_hashes) = with_capped_parallelism(|| {
        use rayon::prelude::*;
        rayon::join(
            || {
                a_images
                    .par_iter()
                    .map(|(image, _)| alignment::hash_page(image))
                    .collect::<Vec<_>>()
            },
            || {
                b_images
                    .par_iter()
                    .map(|(image, _)| alignment::hash_page(image))
                    .collect::<Vec<_>>()
            },
        )
    });
    tracing::info!(
        elapsed_ms = t_hash.elapsed().as_millis(),
        "compare_archives: perceptual hashing (both sides) done"
    );

    let t_align = std::time::Instant::now();
    let mut rescue_attempts = 0u32;
    let mut rescue_hits = 0u32;
    let pairs = alignment::align_sequences_with_rescue(&a_hashes, &b_hashes, |a_index, b_index| {
        rescue_attempts += 1;
        let (a_image, _) = &a_images[a_index];
        let (b_image, _) = &b_images[b_index];
        let (_, confidence) = crop_align::estimate_crop_alignment_with_confidence(a_image, b_image);
        if confidence >= alignment::RESCUE_CONFIDENCE_THRESHOLD {
            rescue_hits += 1;
            Some(alignment::rescued_match_distance())
        } else {
            None
        }
    });
    tracing::info!(
        aligned_pairs = pairs.len(),
        rescue_attempts,
        rescue_hits,
        elapsed_ms = t_align.elapsed().as_millis(),
        "compare_archives: banded-DP alignment (with crop_align rescue) done"
    );

    let comparisons: Vec<PageComparison> = pairs
        .iter()
        .map(|pair| {
            let (a_image, a_file_size) = &a_images[pair.a_index];
            let (b_image, b_file_size) = &b_images[pair.b_index];
            PageComparison {
                a_page_index: pair.a_index,
                b_page_index: pair.b_index,
                a_filename: a_pages[pair.a_index].clone(),
                b_filename: b_pages[pair.b_index].clone(),
                a_file_size: *a_file_size,
                b_file_size: *b_file_size,
                a_sharpness: sharpness::laplacian_variance(a_image),
                b_sharpness: sharpness::laplacian_variance(b_image),
                a_width: a_image.width(),
                a_height: a_image.height(),
                b_width: b_image.width(),
                b_height: b_image.height(),
                // Computed below, only for the subset `sample_evenly` actually picks — see that
                // call site's own docs for why running this for every aligned pair (which can be
                // hundreds) isn't worth the cost.
                crop_alignment: crop_align::CropAlignment::IDENTITY,
            }
        })
        .collect();

    let likely_different_language =
        is_likely_different_language(comparisons.len(), a_pages.len(), b_pages.len());
    // A likely different-language pair is never given a "keep A or B" pick — see
    // `ComparisonResult`'s own docs for why these are fundamentally different situations the
    // caller must present differently, not just two flavors of "low confidence".
    let recommendation = if likely_different_language {
        None
    } else {
        recommend(&comparisons, a_pages.len(), b_pages.len())
    };
    let mut samples = sample_evenly(&comparisons, SAMPLE_SIZE);

    // Phase 1 (coarse) — see `compare_archives_streaming`'s own docs. Each sample's own alignment
    // still computes on the capped rayon pool, but is emitted to `on_event` as soon as ITS OWN
    // worker finishes, via an `mpsc` channel `on_event`'s own caller thread drains — not after
    // waiting for every sample to finish first. An earlier version waited for the whole
    // `par_iter_mut` to complete before emitting any of the 6 sequentially; harmless at 6 samples
    // (a few ms apart) individually, but had the same "wait, then emit all at once" shape as
    // real caller-visible batching once `resume_samples`' own instantly-available seed events made
    // that batching more obvious (reported live: "打开modal后会一次性显示6组" instead of one at a
    // time) — this doesn't just fix that report, it makes phase 1 genuinely stream sample-by-sample
    // as intended, which the previous "wait for all, emit sequentially" shape only accidentally
    // approximated at small `SAMPLE_SIZE`. `sample_index` is carried on each message so out-of-order
    // arrival (whichever worker finishes first) is fine — the frontend already places each sample
    // by its own index, not by arrival order.
    let t_crop_align = std::time::Instant::now();
    let (coarse_tx, coarse_rx) = std::sync::mpsc::channel::<(usize, crop_align::CropAlignment)>();
    // `thread::scope` (not a bare `thread::spawn`, which requires `'static`) — the worker borrows
    // `a_images`/`b_images`/`samples`, all owned by this function's own stack frame, and the scope
    // guarantees the worker is joined (or the whole thing panics) before it returns, so those
    // borrows never outlive their owner even if `on_event` below returns early on cancellation.
    // Results land in `coarse_results` (not written back into `samples` directly) since the worker
    // already holds an immutable borrow of `samples` for the whole scope; applied to `samples`
    // itself only after the scope closes, below.
    let mut coarse_results: Vec<Option<crop_align::CropAlignment>> = vec![None; samples.len()];
    let mut cancelled = false;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            with_capped_parallelism(|| {
                use rayon::prelude::*;
                samples
                    .par_iter()
                    .enumerate()
                    .for_each(|(sample_index, sample)| {
                        let (a_image, _) = &a_images[sample.a_page_index];
                        let (b_image, _) = &b_images[sample.b_page_index];
                        let (alignment, _confidence) =
                            crop_align::estimate_crop_alignment_with_confidence(a_image, b_image);
                        let _ = coarse_tx.send((sample_index, alignment));
                    });
            });
        });
        let mut received = 0;
        while received < samples.len() {
            let Ok((sample_index, alignment)) = coarse_rx.recv() else {
                break;
            };
            received += 1;
            coarse_results[sample_index] = Some(alignment);
            let mut sample = samples[sample_index].clone();
            sample.crop_alignment = alignment;
            let listening = on_event(CompareEvent::Sample {
                sample_index,
                phase: ComparePhase::Coarse,
                sample,
            });
            if !listening {
                cancelled = true;
                break;
            }
        }
    });
    if cancelled {
        return Err(ImgCompareError::Cancelled);
    }
    for (sample_index, alignment) in coarse_results.into_iter().enumerate() {
        if let Some(alignment) = alignment {
            samples[sample_index].crop_alignment = alignment;
        }
    }
    tracing::info!(
        samples = samples.len(),
        elapsed_ms = t_crop_align.elapsed().as_millis(),
        "compare_archives: coarse crop-alignment estimation (sampled pairs only) done"
    );

    // Phase 2 (precise) — only for samples whose coarse alignment already indicates real padding;
    // an identity alignment has nothing to refine. Same per-worker streaming as phase 1 above, for
    // the same reason: a refined sample should update in the caller's UI the moment IT finishes,
    // not wait for every padded sample's own refinement to complete first (the magnifier may be
    // actively open on an already-precise sample while a later one is still computing).
    let t_precise_crop_align = std::time::Instant::now();
    let (precise_tx, precise_rx) = std::sync::mpsc::channel::<(usize, crop_align::CropAlignment)>();
    let padded_count = samples
        .iter()
        .filter(|s| s.crop_alignment != crop_align::CropAlignment::IDENTITY)
        .count();
    let mut cancelled = false;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            with_capped_parallelism(|| {
                use rayon::prelude::*;
                samples
                    .par_iter()
                    .enumerate()
                    .for_each(|(sample_index, sample)| {
                        if sample.crop_alignment == crop_align::CropAlignment::IDENTITY {
                            return;
                        }
                        let (a_image, _) = &a_images[sample.a_page_index];
                        let (b_image, _) = &b_images[sample.b_page_index];
                        let (alignment, _confidence) =
                            crop_align::estimate_precise_crop_alignment_with_confidence(
                                a_image, b_image,
                            );
                        let _ = precise_tx.send((sample_index, alignment));
                    });
            });
        });
        let mut received = 0;
        while received < padded_count {
            let Ok((sample_index, alignment)) = precise_rx.recv() else {
                break;
            };
            received += 1;
            let mut sample = samples[sample_index].clone();
            sample.crop_alignment = alignment;
            let listening = on_event(CompareEvent::Sample {
                sample_index,
                phase: ComparePhase::Precise,
                sample,
            });
            if !listening {
                cancelled = true;
                break;
            }
        }
    });
    if cancelled {
        return Err(ImgCompareError::Cancelled);
    }
    tracing::info!(
        samples = samples.len(),
        precise_count = padded_count,
        elapsed_ms = t_precise_crop_align.elapsed().as_millis(),
        "compare_archives: precise crop-alignment refinement (padded samples only) done"
    );

    let a_unmatched_pages = alignment::unmatched_a_indices(a_pages.len(), &pairs)
        .into_iter()
        .map(|page_index| UnmatchedPage {
            page_index,
            default_insert_after: alignment::default_insert_after_b_index(page_index, &pairs),
        })
        .collect();
    let b_unmatched_pages = alignment::unmatched_b_indices(b_pages.len(), &pairs)
        .into_iter()
        .map(|page_index| UnmatchedPage {
            page_index,
            default_insert_after: alignment::default_insert_after_a_index(page_index, &pairs),
        })
        .collect();

    // Return value ignored — this is the last event of the run, nothing left to skip either way.
    let _ = on_event(CompareEvent::Done {
        aligned_pairs: comparisons.len(),
        a_total_pages: a_pages.len(),
        b_total_pages: b_pages.len(),
        a_entries,
        b_entries,
        likely_different_language,
        recommendation,
        a_filename,
        b_filename,
        a_file_size,
        b_file_size,
        a_unmatched_pages,
        b_unmatched_pages,
    });

    Ok(())
}

/// See [`LANGUAGE_EDITION_ALIGNMENT_RATIO`]'s own docs for the threshold reasoning. Requires equal
/// page counts on both sides in addition to the high alignment ratio — a translated edition
/// preserves the print run's own page count exactly (only in-bubble text changes, not layout/page
/// breaks), so a page-count MISMATCH alongside high alignment is more consistent with "same work,
/// different scan/print run" (issue #77's own original motivating scenario) than a language
/// difference, even though both can produce a high alignment ratio.
fn is_likely_different_language(aligned_pairs: usize, a_total: usize, b_total: usize) -> bool {
    if a_total != b_total || a_total == 0 {
        return false;
    }
    (aligned_pairs as f64 / a_total as f64) >= LANGUAGE_EDITION_ALIGNMENT_RATIO
}

/// Reads every entry in one forward pass (`archive_format::read_entries`, not one `read_entry`
/// call per page) and decodes it in parallel via [`with_capped_parallelism`]. Also returns each
/// page's compressed byte size (for `PageComparison::a_file_size`/`b_file_size`), free since
/// `read_entries` already has the bytes in hand.
fn load_pages(path: &Path, entries: &[String]) -> Result<Vec<(DynamicImage, u64)>> {
    let t_read = std::time::Instant::now();
    let raw = archive_format::read_entries(path, entries)?;
    tracing::info!(
        path = %path.display(),
        entries = entries.len(),
        elapsed_ms = t_read.elapsed().as_millis(),
        "load_pages: read_entries (single forward-pass I/O) done"
    );

    let t_decode = std::time::Instant::now();
    let result: Result<Vec<(DynamicImage, u64)>> = with_capped_parallelism(|| {
        use rayon::prelude::*;
        raw.into_par_iter()
            .zip(entries)
            .map(|(bytes, entry)| {
                let bytes = bytes.ok_or_else(|| ImgCompareError::Decode {
                    entry: entry.clone(),
                    source: image::ImageError::IoError(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("entry {entry:?} not found while re-reading archive"),
                    )),
                })?;
                let file_size = bytes.len() as u64;
                let image =
                    image::load_from_memory(&bytes).map_err(|source| ImgCompareError::Decode {
                        entry: entry.clone(),
                        source,
                    })?;
                Ok((image, file_size))
            })
            .collect()
    });
    tracing::info!(
        path = %path.display(),
        entries = entries.len(),
        elapsed_ms = t_decode.elapsed().as_millis(),
        "load_pages: parallel decode (rayon) done"
    );
    result
}

/// Rolls per-pair sharpness comparisons into an overall pick, or `None` if confidence is too low
/// to trust one — see [`MIN_PAIRS_FOR_RECOMMENDATION`]/[`RECOMMENDATION_AGREEMENT_THRESHOLD`]/
/// [`MIN_ALIGNMENT_RATIO_FOR_RECOMMENDATION`]'s own docs for the specific bars. `a_total`/`b_total`
/// are each side's own full page count (not just the aligned subset `comparisons` covers) — needed
/// to catch two archives that only coincidentally share a handful of near-blank/title pages while
/// their real content doesn't correspond at all (see that constant's own docs).
fn recommend(comparisons: &[PageComparison], a_total: usize, b_total: usize) -> Option<Side> {
    if comparisons.len() < MIN_PAIRS_FOR_RECOMMENDATION {
        return None;
    }
    let smaller_total = a_total.min(b_total).max(1);
    let ratio_ok =
        (comparisons.len() as f64 / smaller_total as f64) >= MIN_ALIGNMENT_RATIO_FOR_RECOMMENDATION;
    let count_ok = comparisons.len() >= MIN_ALIGNMENT_COUNT_FOR_RECOMMENDATION;
    if !ratio_ok && !count_ok {
        return None;
    }
    let a_wins = comparisons
        .iter()
        .filter(|c| c.a_sharpness > c.b_sharpness)
        .count();
    let b_wins = comparisons
        .iter()
        .filter(|c| c.b_sharpness > c.a_sharpness)
        .count();
    let total = comparisons.len();
    let a_share = a_wins as f64 / total as f64;
    let b_share = b_wins as f64 / total as f64;
    if a_share >= RECOMMENDATION_AGREEMENT_THRESHOLD {
        Some(Side::A)
    } else if b_share >= RECOMMENDATION_AGREEMENT_THRESHOLD {
        Some(Side::B)
    } else {
        None
    }
}

/// Picks up to `count` comparisons spread evenly across the full aligned range (not just the
/// first `count`) — a fair cross-section of the work, not biased toward whichever pages happened
/// to align first. Per this crate's own confirmed design, the very first aligned pair (the
/// cover/color-page front matter — carries more signal than an arbitrary interior page) should
/// always be included: this falls out of the even-spacing formula itself (`i=0` always maps to
/// index `0`) rather than needing separate handling.
fn sample_evenly(comparisons: &[PageComparison], count: usize) -> Vec<PageComparison> {
    if comparisons.len() <= count {
        return comparisons.to_vec();
    }
    let step = comparisons.len() as f64 / count as f64;
    (0..count)
        .map(|i| comparisons[((i as f64 * step) as usize).min(comparisons.len() - 1)].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comparison(a: f64, b: f64) -> PageComparison {
        PageComparison {
            a_page_index: 0,
            b_page_index: 0,
            a_filename: String::new(),
            b_filename: String::new(),
            a_file_size: 0,
            b_file_size: 0,
            a_sharpness: a,
            b_sharpness: b,
            a_width: 0,
            a_height: 0,
            b_width: 0,
            b_height: 0,
            crop_alignment: crop_align::CropAlignment::IDENTITY,
        }
    }

    #[test]
    fn is_likely_different_language_true_for_the_real_confirmed_case() {
        // The real numbers from issue #77's own live verification: 176 of 180 pages aligned on
        // both sides (97.8%) for a confirmed same-work cross-language pair.
        assert!(is_likely_different_language(176, 180, 180));
    }

    #[test]
    fn is_likely_different_language_false_when_page_counts_differ() {
        // High alignment ratio alone isn't enough — a genuine "different scan/print run" case
        // (issue #77's own original motivating scenario) can also align well but typically has a
        // page-count difference (front matter, scan artifacts), which this must not misclassify
        // as a language edition.
        assert!(!is_likely_different_language(176, 180, 198));
    }

    #[test]
    fn is_likely_different_language_false_below_the_alignment_ratio_threshold() {
        assert!(!is_likely_different_language(100, 180, 180));
    }

    #[test]
    fn is_likely_different_language_false_for_zero_pages() {
        assert!(!is_likely_different_language(0, 0, 0));
    }

    #[test]
    fn recommend_is_none_below_minimum_pair_count() {
        let comparisons = vec![comparison(10.0, 1.0), comparison(10.0, 1.0)];
        assert_eq!(recommend(&comparisons, 2, 2), None);
    }

    #[test]
    fn recommend_picks_the_side_that_wins_a_strong_majority() {
        let comparisons = vec![
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(1.0, 10.0),
        ];
        // 3 of 4 = 75% >= 70% threshold, and 4 aligned of 4 total pages each side clears the
        // alignment-ratio guard easily.
        assert_eq!(recommend(&comparisons, 4, 4), Some(Side::A));
    }

    #[test]
    fn recommend_is_none_for_a_near_even_split() {
        let comparisons = vec![
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(1.0, 10.0),
            comparison(1.0, 10.0),
        ];
        // 3 of 5 = 60% < 70% threshold — must defer to human judgment, not assert a pick.
        assert_eq!(recommend(&comparisons, 5, 5), None);
    }

    #[test]
    fn recommend_is_none_when_aligned_pairs_are_a_small_fraction_of_total_pages() {
        // A real confirmed scenario: two archives that only coincidentally share a title/filename
        // — a handful of near-blank/title pages happen to align (and, by pure chance, all agree A
        // is sharper), but they're a tiny fraction of either side's real page count. The absolute
        // pair count (4) clears `MIN_PAIRS_FOR_RECOMMENDATION` and the agreement (100%) clears
        // `RECOMMENDATION_AGREEMENT_THRESHOLD`, but this must still refuse to recommend — these
        // two archives are not actually the same work.
        let comparisons = vec![
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
            comparison(10.0, 1.0),
        ];
        assert_eq!(recommend(&comparisons, 200, 180), None);
    }

    #[test]
    fn recommend_trusts_a_large_absolute_count_even_at_a_low_ratio() {
        // The scanlation-re-release case: B is A's later, expanded edition — every one of A's
        // pages is expected to still be in B, but B itself has grown so large (many new pages)
        // that `pairs.len() / smaller_total` alone reads as a modest ratio. 25 aligned pairs (>=
        // `MIN_ALIGNMENT_COUNT_FOR_RECOMMENDATION`) must still be trusted via the OR, not rejected
        // just because the ratio bar wasn't independently cleared.
        let comparisons: Vec<PageComparison> = (0..25).map(|_| comparison(10.0, 1.0)).collect();
        // 25 aligned of a 30-page smaller side = 83% ratio anyway here, so use a bigger smaller
        // side to isolate the count-only path: 25 aligned of 60 total = ~42%, below the 50% ratio
        // bar, but the absolute count (25 >= 20) must still carry it.
        assert_eq!(recommend(&comparisons, 60, 500), Some(Side::A));
    }

    #[test]
    fn sample_evenly_returns_everything_when_below_the_sample_size() {
        let comparisons = vec![comparison(1.0, 2.0), comparison(3.0, 4.0)];
        assert_eq!(sample_evenly(&comparisons, 5).len(), 2);
    }

    #[test]
    fn sample_evenly_spreads_across_the_full_range_not_just_the_start() {
        let comparisons: Vec<_> = (0..20).map(|i| comparison(i as f64, 0.0)).collect();
        let sampled = sample_evenly(&comparisons, 5);
        assert_eq!(sampled.len(), 5);
        // The last sample must come from near the end of the range, not be clustered at the start
        // — a_sharpness == original index here, so this also checks spread numerically.
        assert!(
            sampled.last().unwrap().a_sharpness >= 15.0,
            "got {sampled:?}"
        );
    }

    #[test]
    fn sample_evenly_always_includes_the_cover_page() {
        // Confirmed design requirement: the cover/front-matter page carries more signal than an
        // arbitrary interior page, so it must always be among the samples, not left to chance.
        let comparisons: Vec<_> = (0..37).map(|i| comparison(i as f64, 0.0)).collect();
        let sampled = sample_evenly(&comparisons, 5);
        assert_eq!(sampled.first().unwrap().a_sharpness, 0.0, "got {sampled:?}");
    }
}
