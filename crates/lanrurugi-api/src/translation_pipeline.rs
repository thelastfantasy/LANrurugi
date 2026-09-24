//! The server-side translation orchestration the reader's page requests actually drive
//! (T028/T029/T034/T039).
//!
//! This module is what connects the individually-built pieces into one real path:
//!
//! ```text
//! GET .../page/{n}/translation  (cache miss)
//!   → schedule()                       ← records "the reader is here", opens the look-ahead window
//!       → PrefetchScheduler            ← decides which pages still need work (T039)
//!           → translate_page()         ← one page's full chain:
//!               run_batch              ← OCR detect + recognize + style estimate (T009)
//!               classify_batch/route   ← font voting or golden-set routing (T012/T013/T014, T034)
//!               save_detected          ← regions are the authoritative record (research.md §16)
//!               translate_batch        ← context-assembled, batched LLM call (T019b/T015)
//!               save_translated        ← translated regions persisted before rendering
//!               composite_page         ← draw over the page (T029)
//!               TranslationImageCache::put  ← so the next poll is a cache hit
//!               BudgetRepository::record    ← usage accounting (FR-014)
//! ```
//!
//! **Why work happens here and not inline in the handler**: an OCR + LLM round trip takes tens of
//! seconds. FR-012 requires the reader to show the original page immediately with a non-blocking
//! indicator, and `contracts/translation-api.md` specifies the `202` not-ready response as a normal
//! outcome, not an error. So the handler triggers this and returns not-ready at once; the reader
//! polls, and a later poll hits the cache this fills. Holding the connection open for the full
//! cycle would violate both.
//!
//! **Failure isolation (FR-020)**: every page is translated in its own task and records its own
//! outcome. One page's OCR or provider failure never affects another's.

use std::sync::Arc;

use image::RgbImage;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_fontcache::classify::Classification;
use lanrurugi_fontcache::entities::VolumeFontPattern;
use lanrurugi_fontcache::voting::VoteCandidate;
use lanrurugi_fontcache::{routing, FontId, FontPatternRepository, RouteDecision};
use lanrurugi_ocr::batch::{run_batch, OcrEngine, PageInput};
use lanrurugi_ocr::entities::{BoundingBox, DetectedTextRegion, PageNumber, VolumeId};
use lanrurugi_translate::adapter::TranslationError;
use lanrurugi_translate::budget::BudgetRepository;
use lanrurugi_translate::cache::{TranslationCacheKey, TranslationImageCache};
use lanrurugi_translate::composite::{encode_webp, finish_composite_page, prepare_page_erase};
use lanrurugi_translate::fonts::FontLibrary;
use lanrurugi_translate::glossary::GlossaryRepository;
use lanrurugi_translate::pipeline::{translate_batch, PageWork};
use lanrurugi_translate::prefetch::{PageState, PrefetchScheduler};
use lanrurugi_translate::regions::RegionRepository;
use lanrurugi_translate::settings::{CloudProvider, TranslationSettings};
use lanrurugi_translate::{
    AnthropicAdapter, CredentialStore, DeepSeekAdapter, OpenAiCompatAdapter,
};

use crate::AppState;

/// WebP quality for composited pages — matches the reader's own default so a translated page and
/// an untranslated one look consistent and cost comparable disk space.
const COMPOSITE_QUALITY: f32 = 90.0;

/// Upper bound on one LLM translation HTTP call (connect + send + receive), across every cloud
/// provider — see `translate_with_configured_provider`'s own doc comment on the real silent-hang
/// incident this fixes. 90s, not the GPU worker's 60s `CALL_TIMEOUT`: a translation batch can cover
/// up to `lanrurugi_translate::settings::MAX_BATCH_PAGES` (4) pages' worth of text blocks in one
/// request, a meaningfully larger payload than any single GPU inference call, so a somewhat longer
/// ceiling is warranted — but still bounded, not the "wait forever" default `reqwest::Client::new()`
/// otherwise leaves in place.
const LLM_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Everything one page's translation needs, resolved once per scheduling round rather than per
/// page: settings, repositories, models, and fonts are all shared across the whole window.
#[derive(Clone)]
pub struct TranslationContextHandles {
    pub settings: TranslationSettings,
    pub provider: CloudProvider,
    pub target_language: String,
    pub regions: RegionRepository,
    pub glossary: GlossaryRepository,
    pub fonts_repo: FontPatternRepository,
    pub budget: BudgetRepository,
    pub cache: TranslationImageCache,
    pub engine: Arc<OcrEngine>,
    pub font_library: Arc<FontLibrary>,
    /// GPU EP integration plan v9 (issue #103): whether the inpainting/bubble-segmentation models
    /// are actually installed is no longer known ahead of time (the worker that would tell us
    /// starts lazily) — `composite_and_cache` finds out by calling `image_worker.erase_page`/
    /// `.segment_bubbles` and treating `GpuWorkerError::Worker("no ... model loaded")` the same
    /// way the old `None` case degraded (flat-fill/outline-stroke, plain rectangular erasure).
    /// `Image`-kind only (bubble segmenter + inpainter) — text recognition uses a separate
    /// `Recognize`-kind client owned by `engine` instead; see
    /// `crate::gpu_worker_client::WorkerKind`'s own doc comment for why they're split.
    pub image_worker: Arc<crate::gpu_worker_client::GpuWorkerClient>,
    /// SC-003/SC-004 counters, shared process-wide via [`crate::AppState`].
    pub telemetry: Arc<lanrurugi_translate::TranslationTelemetry>,
}

/// Why a page couldn't be translated. Kept separate from [`TranslationError`] because OCR/IO
/// failures aren't provider failures, and FR-019 surfaces both the same way to the reader.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("page image unavailable: {0}")]
    PageUnavailable(String),
    #[error("OCR failed: {0}")]
    Ocr(String),
    #[error("storage failed: {0}")]
    Storage(String),
    #[error(transparent)]
    Translation(#[from] TranslationError),
    #[error("compositing failed: {0}")]
    Composite(String),
}

/// Builds the configured cloud adapter and runs `translate_batch` through it.
///
/// Dispatching here (rather than storing a boxed adapter) keeps [`TranslationAdapter`]'s
/// `async fn` in trait shape usable without `dyn` compatibility gymnastics, and resolves the
/// credential immediately before the call so it never lives longer than one request (FR-006).
async fn translate_with_configured_provider(
    state: &AppState,
    ctx: &TranslationContextHandles,
    glossary: &mut lanrurugi_translate::TerminologyGlossary,
    pages: Vec<PageWork>,
) -> Result<lanrurugi_translate::pipeline::BatchOutcome, TranslationError> {
    let credentials = CredentialStore::new(state.redis.config.clone());
    // `reqwest::Client::new()` has no request timeout at all by default — a real incident (2026-09-
    // 14) found a translation batch silently hang forever with no log output anywhere (not even the
    // OCR/GPU-worker side, which does have its own `CALL_TIMEOUT`): the LLM HTTP call itself never
    // returned and nothing was ever going to time it out. `LLM_REQUEST_TIMEOUT` bounds the whole
    // request (connect + send + receive body), not just the connect phase — a provider that accepts
    // the connection but then never responds (or streams so slowly it may as well not have) is
    // exactly the failure mode this project actually hit, and `.timeout()` covers both.
    let client = reqwest::Client::builder()
        .timeout(LLM_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let model = ctx.settings.model.clone().unwrap_or_default();
    let endpoint = ctx.settings.endpoint.clone();

    match ctx.provider {
        CloudProvider::Anthropic => {
            let key = credentials
                .resolve(&ctx.provider.credential_ref())
                .await
                .map_err(|_| TranslationError::AuthFailed)?;
            let adapter = AnthropicAdapter::new(
                client,
                endpoint.unwrap_or_else(|| "https://api.anthropic.com".into()),
                model,
                key.expose(),
            );
            translate_batch(&adapter, glossary, pages, &ctx.target_language).await
        }
        CloudProvider::DeepSeek => {
            // The project-wide DeepSeek key (`LRR_CONFIG.llm_api_key`, shared with the recommend
            // system) takes precedence unconditionally — one account, one key. The
            // translation-specific credential is only a fallback for installs that configured it
            // before the global key existed.
            let key = match crate::translation_settings::global_deepseek_key(state).await {
                Some(key) => key,
                None => credentials
                    .resolve(&ctx.provider.credential_ref())
                    .await
                    .map_err(|_| TranslationError::AuthFailed)?
                    .expose()
                    .to_string(),
            };
            let adapter = DeepSeekAdapter::new(client, key);
            translate_batch(&adapter, glossary, pages, &ctx.target_language).await
        }
        CloudProvider::OpenAiCompatible => {
            // An OpenAI-compatible endpoint may be a keyless local gateway, so a missing
            // credential is not fatal here (unlike the two metered providers above).
            let key = credentials
                .resolve(&ctx.provider.credential_ref())
                .await
                .ok()
                .map(|s| s.expose().to_string());
            let adapter = OpenAiCompatAdapter::new(
                client,
                endpoint.unwrap_or_else(|| "https://api.openai.com/v1".into()),
                model,
                key,
            );
            translate_batch(&adapter, glossary, pages, &ctx.target_language).await
        }
    }
}

/// Loads one page's decoded pixels from the archive.
///
/// Both the archive read and the image decode are blocking CPU/IO work, so both go through
/// `run_blocking` rather than running on an async worker thread (constitution Principle III).
///
/// `pub(crate)`: also used directly by `translation::trigger_local_path_detection` to load the
/// image for its own up-front bubble-segmentation call (same reasoning as `translate_page`'s own
/// — see that function's doc comment on why bubble segmentation must run exactly once per request).
pub(crate) async fn load_page_image(
    state: &AppState,
    archive_id: &ArchiveId,
    page: PageNumber,
) -> Result<(RgbImage, bool, u32), PipelineError> {
    let archive = state
        .repos
        .archives
        .get(archive_id)
        .await
        .map_err(|e| PipelineError::Storage(e.to_string()))?
        .ok_or_else(|| PipelineError::PageUnavailable("no such archive".into()))?;

    let file = archive.file.clone();
    let entries = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::archive_format::list_pages(std::path::Path::new(&file))
    })
    .await
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?;

    let total_pages = entries.len() as u32;
    // Page numbers are 1-based throughout this feature (`PageNumber(1)` is the cover).
    let index = page.get().saturating_sub(1) as usize;
    let entry = entries
        .get(index)
        .ok_or_else(|| PipelineError::PageUnavailable(format!("page {page} out of range")))?
        .clone();

    let file = archive.file.clone();
    let raw = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::archive_format::read_entry(std::path::Path::new(&file), &entry)
    })
    .await
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?;

    let image = lanrurugi_core::concurrency::run_blocking(move || {
        image::load_from_memory(&raw).map(|img| img.to_rgb8())
    })
    .await
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?
    .map_err(|e| PipelineError::PageUnavailable(e.to_string()))?;

    // The cover is page 1 — `is_cover` comes from page position (Phase 1's own page ordering),
    // never inferred from OCR output (FR-008).
    Ok((image, page.get() == 1, total_pages))
}

/// Runs OCR for a page and persists the result, unless regions are already stored.
///
/// This is the shared "detection must exist before anything else can happen" step: the cloud path
/// (below) and the local-backend path (`translation::get_page_text_regions`) both need it, and
/// neither should re-run a detection that already happened.
///
/// A cached result of zero regions is deliberately never trusted as a cache hit — only ever
/// persisted below when `regions` is non-empty, and `get_detected` returning `Some(vec![])`
/// (a pre-existing empty result from before this fix, or a race with another in-flight
/// detection) falls through to re-running detection rather than short-circuiting. Real reported
/// incident (2026-09-16): a page whose true content is dense with real dialogue permanently
/// cached zero regions — almost certainly from a transient failure during a real host-level
/// memory-pressure incident this same session hit repeatedly (GPU worker `DeadlineExceeded`
/// kills, `whole-page inpainting failed` degradations) silently dropping every candidate region
/// downstream of detection without `run_batch` itself returning an `Err` — and every subsequent
/// request for that page kept returning the same empty, permanently-stuck result forever, with
/// no retry path at all short of an operator manually deleting the Redis key. `run_batch`
/// succeeding (`Ok`) with an empty `Vec` is inherently ambiguous at this layer (a genuinely
/// textless page vs. every real candidate silently filtered out downstream, e.g. by
/// `looks_like_japanese()` or a swallowed per-region recognition error) — there's no reliable way
/// to tell those apart here, so the safe choice is to never let an empty result become permanent:
/// re-running detection for an actually-blank page only costs one extra (cheap) inference call,
/// while a falsely-cached empty result costs that page's translation forever.
///
/// The same rule extends to *partial* results (issue #101): when `run_batch` reports
/// `degraded_by_infrastructure_failure`, at least one line box was dropped because the GPU worker
/// RPC failed rather than because the model rejected that crop's content, so the region set is
/// incomplete for a transient reason and is returned to the caller but never persisted. A
/// content-level rejection (a low-confidence decode on unreadable SFX lettering, say) is the
/// opposite case — reproducible, so it caches normally.
pub async fn ensure_detected(
    state: &AppState,
    ctx: &TranslationContextHandles,
    archive_id: &ArchiveId,
    page: PageNumber,
    bubbles: Option<&[lanrurugi_ocr::bubble_segment::DetectedBubble]>,
) -> Result<Vec<DetectedTextRegion>, PipelineError> {
    detect_with_degradation(state, ctx, archive_id, page, bubbles)
        .await
        .map(|(regions, _)| regions)
}

/// [`ensure_detected`], but also reporting whether this detection was degraded by an
/// infrastructure failure.
///
/// Split out because the degradation flag has to survive past detection: the *rendered* page
/// cache (`TranslationImageCache`, on disk) freezes a degraded result just as permanently as
/// the Redis region cache does, so `translate_page` needs the flag to decide whether the
/// composite it produces is safe to keep (issue #101). Callers that only ever read regions
/// (`translation::detect_for_local_backend`) keep using the simpler wrapper above.
async fn detect_with_degradation(
    state: &AppState,
    ctx: &TranslationContextHandles,
    archive_id: &ArchiveId,
    page: PageNumber,
    bubbles: Option<&[lanrurugi_ocr::bubble_segment::DetectedBubble]>,
) -> Result<(Vec<DetectedTextRegion>, bool), PipelineError> {
    // `Some(vec![])` — an empty-but-present cached result — deliberately does NOT short-circuit
    // here, only `Some(non_empty)` does; see this function's own doc comment for why. A stale
    // empty entry from before this fix (or a genuinely blank page that gets re-detected as empty
    // again below) simply falls through to a fresh detection call rather than being trusted.
    // A cache hit is by definition not degraded: only a non-degraded detection was ever allowed
    // to be persisted in the first place.
    if let Ok(Some(existing)) = ctx.regions.get_detected(archive_id, page).await {
        if !existing.is_empty() {
            return Ok((existing, false));
        }
    }

    let (image, is_cover, _total) = load_page_image(state, archive_id, page).await?;

    // One page per `run_batch` call here, deliberately: pages arrive as the reader (or look-ahead)
    // reaches them, and each is scheduled as its own independent task for FR-020 isolation. The
    // batching that matters for cost — the LLM request — happens in `translate_batch`, which does
    // group several pages' blocks into one call. `run_batch` itself still dispatches all of a
    // page's regions through a single rayon `parallel_map`, which is where OCR's parallelism
    // actually comes from (constitution Principle III).
    //
    // `bubbles` (already computed by `translate_page`, strictly *before* this call — see that
    // function's own doc comment on why bubble segmentation runs up front rather than inside this
    // function) is threaded straight into `PageInput` so `merge_lines` can consider bubble
    // membership and geometry together in one pass (`lanrurugi_ocr::merge::merge_lines`'s own doc
    // comment covers why a single unified pass, not two sequential ones, is what actually fixed a
    // real 2026-09-15 bug).
    let results = run_batch(
        Arc::clone(&ctx.engine),
        vec![PageInput {
            archive_id: archive_id.clone(),
            page_number: page,
            is_cover,
            image: image.clone(),
            bubbles: bubbles.map(|b| b.to_vec()),
        }],
    )
    .await
    .map_err(|e| PipelineError::Ocr(e.to_string()))?;

    let (mut regions, mut degraded) = results
        .into_iter()
        .next()
        .map(|r| (r.regions, r.degraded_by_infrastructure_failure))
        .unwrap_or_default();

    // --- Bubble-guided + white-blob fallbacks ------------------------------------------------
    // Two independent sources of "a real text block the detector never boxed":
    //  - a bubble the segmentation model *did* detect, but no surviving region covers;
    //  - a near-white connected component (bubble interior) found directly in the page image,
    //    independent of the bubble model — for handwritten/coloured SFX bubbles the model misses
    //    entirely (real page-12 incident: the purple "つけたね。" bubble was in neither the
    //    detector's boxes nor the bubble model's output).
    let mut candidates: Vec<BoundingBox> = Vec::new();
    if let Some(bubbles) = bubbles {
        for bubble in bubbles {
            let (w, h) = (bubble.bbox.w, bubble.bbox.h);
            let area = u64::from(w) * u64::from(h);
            if w < 24 || h < 24 || area > 200_000 {
                continue;
            }
            if regions.iter().any(|region| {
                region_area_fraction_inside_bubble(&region.bounding_box, bubble) >= 0.5
            }) {
                continue;
            }
            candidates.push(bubble.bbox);
        }
    }
    candidates.extend(white_blob_candidates(&image, &regions));

    // Drop candidates that merely nest inside a larger one (bubble bbox + its own white blob would
    // otherwise be recognized twice).
    candidates.sort_by_key(|b| std::cmp::Reverse(u64::from(b.w) * u64::from(b.h)));
    let mut deduped: Vec<BoundingBox> = Vec::new();
    for candidate in candidates {
        let nested = deduped.iter().any(|kept| {
            kept.x <= candidate.x
                && kept.y <= candidate.y
                && kept.x + kept.w >= candidate.x + candidate.w
                && kept.y + kept.h >= candidate.y + candidate.h
        });
        if !nested {
            deduped.push(candidate);
        }
    }
    if !deduped.is_empty() {
        tracing::info!(
            count = deduped.len(),
            "translation fallback: candidate text boxes with no covering region"
        );
        let recognizer = ctx.engine.recognizer();
        let crops: Vec<(usize, RgbImage)> = deduped
            .iter()
            .enumerate()
            .filter_map(|(index, bbox)| {
                lanrurugi_ocr::style_estimate::crop_region(&image, bbox).map(|crop| (index, crop))
            })
            .collect();
        let outcomes = lanrurugi_core::concurrency::run_blocking(move || {
            crops
                .into_iter()
                .map(|(index, crop)| (index, recognizer.recognize(&crop)))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| PipelineError::Ocr(e.to_string()))?;
        for (index, outcome) in outcomes {
            match outcome {
                Ok(text) => {
                    let text = text.trim();
                    if !text.is_empty() && lanrurugi_ocr::recognize::looks_like_japanese(text) {
                        let bbox = deduped[index];
                        tracing::info!(
                            ?bbox,
                            "translation fallback recovered text the detector missed"
                        );
                        regions.push(DetectedTextRegion::new(
                            archive_id.clone(),
                            page,
                            bbox,
                            text.to_string(),
                            is_cover,
                        ));
                    }
                }
                Err(lanrurugi_ocr::batch::RecognizeHandleError::Infrastructure(error)) => {
                    tracing::warn!(
                        %error,
                        "translation fallback hit an infrastructure failure; page stays degraded"
                    );
                    degraded = true;
                }
                Err(_) => {}
            }
        }
    }

    // Drop overlapping duplicate regions: a large bubble-level region and a narrow column region
    // can both survive detection/fallbacks while covering the same source text, which renders the
    // translation twice on top of itself ("double lettering"). Real reported incident (2026-09-20,
    // page 13 left-middle bubble: `そしてこの任へ` at x=166 and `そしてこの任へ就くにあたって....`
    // at x=28 overlapped 74%, both translated and drawn).
    deduplicate_overlapping_regions(&mut regions);

    // Re-estimate fg/bg colour over each region's own *final* (possibly bubble-merged) bounding
    // box, rather than trusting whatever a pre-merge line's own narrower crop happened to estimate
    // (issue found 2026-09-14 via a reader screenshot): `merge_lines` can fold several visually-
    // separated columns — with real background gaps between them — into one wide box, and a colour
    // pair estimated from only the first (reading-order) column doesn't describe that gap-including
    // box at all. `combined_erase_mask` (composite.rs) then builds its stroke mask from exactly
    // this `fg_color`/`bg_color` pair over the *merged* box, so a stale narrow-column estimate
    // there produced a mask so wrong the erase pass silently skipped the region entirely
    // (`merge_region_mask_into_page`'s own no-precise-mask-means-skip rule) — the original Japanese
    // lettering stayed on the page with the translation drawn directly over it. Re-estimating here
    // costs one cheap crop + `style_estimate::estimate` per region, same cost `run_batch` already
    // pays once per region pre-merge.
    for region in &mut regions {
        if let Some(crop) = lanrurugi_ocr::style_estimate::crop_region(&image, &region.bounding_box)
        {
            let style = lanrurugi_ocr::style_estimate::estimate(&crop, 1);
            region.fg_color = style.fg_color;
            region.bg_color = style.bg_color;
            region.is_bold = style.is_bold;
        }
    }

    // Font resolution (T034) happens here, at detection time, so the resolved `font` is persisted
    // onto the authoritative record rather than recomputed on every render (research.md §16).
    let volume_id = resolve_volume_id(state, archive_id).await;
    resolve_fonts(ctx, &volume_id, &image, &mut regions).await;

    // Only a non-empty result gets persisted — see this function's own doc comment on why an
    // empty result is never trusted as permanent. A genuinely blank page just re-runs detection
    // (and finds nothing again) on its next request too, which costs one cheap inference call;
    // that's a strictly better failure mode than a real page's content getting permanently stuck
    // behind a falsely-cached empty result with no retry path at all.
    if degraded {
        // Issue #101: at least one line box was dropped because the GPU worker RPC failed
        // (timeout / not ready / connection lost / session died mid-call), not because the model
        // judged that crop unreadable. `regions` is therefore a silently incomplete view of this
        // page, and persisting it would freeze that specific incomplete translation in Redis
        // forever — the same permanent-staleness failure mode the empty-result rule above already
        // guards against, just with a partial result instead of an empty one. Returning it
        // unpersisted still serves this request as well as we can while leaving the next request
        // free to re-detect and (under less resource pressure) get the full page.
        tracing::warn!(
            %archive_id,
            %page,
            region_count = regions.len(),
            "recognition hit an infrastructure failure; returning this page's regions without caching them so a later request can retry"
        );
    }

    if should_persist_detection(&regions, degraded) {
        ctx.regions
            .save_detected(archive_id, page, &regions)
            .await
            .map_err(|e| PipelineError::Storage(e.to_string()))?;
    }

    Ok((regions, degraded))
}

/// Removes regions that substantially overlap a larger region *and* whose source text is
/// contained in (or contains) the larger one's — the double-lettering failure mode where a
/// bubble-level region and a narrow column region both survive for the same Japanese text.
fn deduplicate_overlapping_regions(regions: &mut Vec<DetectedTextRegion>) {
    let mut keep = vec![true; regions.len()];
    for i in 0..regions.len() {
        for j in (i + 1)..regions.len() {
            if !keep[i] || !keep[j] {
                continue;
            }
            let (a, b) = (&regions[i], &regions[j]);
            let inter = bbox_intersection_area(&a.bounding_box, &b.bounding_box);
            let area_a = u64::from(a.bounding_box.w) * u64::from(a.bounding_box.h);
            let area_b = u64::from(b.bounding_box.w) * u64::from(b.bounding_box.h);
            let smaller = area_a.min(area_b);
            // 50%, not the previous 70%: a detector-fragmented block can leave one column as a
            // separate region whose bbox only half-overlaps the merged region (real page-15
            // `ーッ` fragment), yet the fragment still belongs to that block. The text-overlap
            // check below prevents dropping genuinely unrelated nearby regions.
            if smaller == 0 || inter * 10 < smaller * 5 {
                continue;
            }
            if !region_texts_overlap(a, b) {
                continue;
            }
            // Keep the larger region (its box already covers the shared text area); drop the other.
            let drop_i = area_a < area_b;
            if drop_i {
                keep[i] = false;
                break;
            } else {
                keep[j] = false;
            }
        }
    }
    let mut index = 0;
    regions.retain(|_| {
        let keep_this = keep[index];
        index += 1;
        keep_this
    });
}

/// Whether one text's own visible characters are contained in the other's (after stripping
/// whitespace/punctuation) — the signal that two overlapping regions describe the same lettering.
fn regions_text_overlap(a: &str, b: &str) -> bool {
    fn normalise(text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && !"，。、！？…「」『』（）,.!?()".contains(*c))
            // OCR routinely confuses small kana with their full-size variants (`ッ` vs `ツ` is
            // exactly the page-15 fragment/canonical pair this overlap check exists for). Fold
            // them together for *overlap detection only*; the actual region text is untouched.
            .map(|c| match c {
                'ァ' => 'ア',
                'ィ' => 'イ',
                'ゥ' => 'ウ',
                'ェ' => 'エ',
                'ォ' => 'オ',
                'ッ' => 'ツ',
                'ャ' => 'ヤ',
                'ュ' => 'ユ',
                'ョ' => 'ヨ',
                'ヮ' => 'ワ',
                'ヵ' => 'カ',
                'ヶ' => 'ケ',
                'ぁ' => 'あ',
                'ぃ' => 'い',
                'ぅ' => 'う',
                'ぇ' => 'え',
                'ぉ' => 'お',
                'っ' => 'つ',
                'ゃ' => 'や',
                'ゅ' => 'ゆ',
                'ょ' => 'よ',
                'ゎ' => 'わ',
                other => other,
            })
            .collect()
    }

    /// Whether every character in `small` occurs at least as many times in `big` (multiset
    /// containment). Catches OCR-fragment pairs like `ーッ` / `スーツ母娘` where the characters
    /// are a scrambled subset rather than a contiguous substring.
    fn multiset_contains(big: &str, small: &str) -> bool {
        let mut counts: Vec<(char, usize)> = Vec::new();
        for ch in big.chars() {
            match counts.iter_mut().find(|(seen, _)| *seen == ch) {
                Some((_, count)) => *count += 1,
                None => counts.push((ch, 1)),
            }
        }
        for ch in small.chars() {
            match counts.iter_mut().find(|(seen, _)| *seen == ch) {
                Some((_, count)) if *count > 0 => *count -= 1,
                _ => return false,
            }
        }
        true
    }

    let (na, nb) = (normalise(a), normalise(b));
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    na.contains(&nb)
        || nb.contains(&na)
        || multiset_contains(&na, &nb)
        || multiset_contains(&nb, &na)
}

/// Whether two regions describe overlapping lettering, allowing either side's alternate OCR
/// candidate to be the one that contains the other's visible characters.
fn region_texts_overlap(a: &DetectedTextRegion, b: &DetectedTextRegion) -> bool {
    let a_texts: Vec<&str> = std::iter::once(a.source_text.as_str())
        .chain(a.alternate_source_text.as_deref())
        .collect();
    let b_texts: Vec<&str> = std::iter::once(b.source_text.as_str())
        .chain(b.alternate_source_text.as_deref())
        .collect();
    a_texts.iter().any(|a_text| {
        b_texts
            .iter()
            .any(|b_text| regions_text_overlap(a_text, b_text))
    })
}

/// Intersection area of two axis-aligned boxes in pixels (`0` when they don't overlap).
fn bbox_intersection_area(a: &BoundingBox, b: &BoundingBox) -> u64 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 || y1 <= y0 {
        0
    } else {
        u64::from(x1 - x0) * u64::from(y1 - y0)
    }
}

/// Near-white connected components (bubble interiors) with no existing text region covering them,
/// largest first — an image-only fallback for bubbles the segmentation model missed entirely.
fn white_blob_candidates(page: &RgbImage, regions: &[DetectedTextRegion]) -> Vec<BoundingBox> {
    let (pw, ph) = page.dimensions();
    let mut visited = vec![false; (pw * ph) as usize];
    let is_light = |x: u32, y: u32| -> bool {
        let p = page.get_pixel(x, y).0;
        let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
        r.min(g).min(b) >= 200 && (r.max(g).max(b) - r.min(g).min(b)) <= 28
    };
    let mut out = Vec::new();
    for y in 0..ph {
        for x in 0..pw {
            let start = (y * pw + x) as usize;
            if visited[start] || !is_light(x, y) {
                continue;
            }
            let mut stack = vec![(x, y)];
            visited[start] = true;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x, y, x, y);
            let mut count = 0u64;
            while let Some((cx, cy)) = stack.pop() {
                count += 1;
                min_x = min_x.min(cx);
                min_y = min_y.min(cy);
                max_x = max_x.max(cx);
                max_y = max_y.max(cy);
                for (nx, ny) in [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ] {
                    if nx >= pw || ny >= ph {
                        continue;
                    }
                    let ni = (ny * pw + nx) as usize;
                    if !visited[ni] && is_light(nx, ny) {
                        visited[ni] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
            if !(600..=120_000).contains(&count)
                || bw < 24
                || bh < 24
                || (bw.max(bh) as f32 / bw.min(bh) as f32) > 8.0
            {
                continue;
            }
            let bbox = BoundingBox::new(min_x, min_y, bw, bh);
            if !blob_covered(&bbox, regions) {
                out.push(bbox);
            }
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(u64::from(b.w) * u64::from(b.h)));
    out.truncate(8);
    out
}

/// Whether an existing text region already sits (almost entirely) inside `bbox` — the signal that
/// this white blob is a bubble whose text was detected/translated already. Deliberately measured on
/// the *region's* own area (>=80% of the region inside the blob bbox) rather than the blob's area:
/// a tight text-column region covers only a fraction of its bubble's white interior, so a
/// blob-area threshold would wrongly keep the bubble as a fallback candidate and add a duplicate
/// second translation for it (real regression caught on the 2026-09-19 page-12 run: the top-right
/// bubble's existing region is much smaller than the bubble's white blob, and the blob fallback
/// produced a duplicate region covering the whole bubble).
fn blob_covered(bbox: &BoundingBox, regions: &[DetectedTextRegion]) -> bool {
    regions.iter().any(|region| {
        let r = &region.bounding_box;
        let region_area = u64::from(r.w) * u64::from(r.h);
        if region_area == 0 {
            return false;
        }
        let x0 = bbox.x.max(r.x);
        let y0 = bbox.y.max(r.y);
        let x1 = (bbox.x + bbox.w).min(r.x + r.w);
        let y1 = (bbox.y + bbox.h).min(r.y + r.h);
        if x1 <= x0 || y1 <= y0 {
            return false;
        }
        let inside = u64::from(x1 - x0) * u64::from(y1 - y0);
        // 80%, not 90: a merged multi-column label's bbox can sit slightly off-centre inside its
        // white interior (real page-15 `エルフ母娘`), and re-recognizing that interior would only
        // pay for a duplicate the later region-dedup pass immediately throws away.
        inside * 10 >= region_area * 8
    })
}

/// Fraction of `region`'s own bbox area whose pixels fall inside `bubble`'s detected *mask* — the
/// same measurement `merge_lines`'s own bubble membership uses, reused here so the bubble-guided
/// fallback is not fooled by a neighbouring bubble's region merely overlapping this bubble's bbox.
fn region_area_fraction_inside_bubble(
    region: &lanrurugi_ocr::entities::BoundingBox,
    bubble: &lanrurugi_ocr::bubble_segment::DetectedBubble,
) -> f32 {
    let mut inside = 0u64;
    let mut total = 0u64;
    for y in region.y..region.y.saturating_add(region.h) {
        for x in region.x..region.x.saturating_add(region.w) {
            total += 1;
            if x < bubble.bbox.x || y < bubble.bbox.y {
                continue;
            }
            let (bx, by) = (x - bubble.bbox.x, y - bubble.bbox.y);
            if bx >= bubble.bbox.w || by >= bubble.bbox.h {
                continue;
            }
            if bubble.mask[(by * bubble.bbox.w + bx) as usize] {
                inside += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        inside as f32 / total as f32
    }
}

/// Whether a freshly-detected region set is safe to persist as this page's authoritative record.
///
/// Two independent reasons not to, both about the same hazard — a result that is wrong for a
/// *transient* reason becoming permanent, with no retry path short of an operator deleting the
/// Redis key by hand:
///
/// - `degraded`: recognition hit an infrastructure failure, so regions are silently incomplete
///   (issue #101).
/// - empty: indistinguishable at this layer from "everything got dropped downstream", so it is
///   never trusted — see [`ensure_detected`]'s own doc comment.
fn should_persist_detection(regions: &[DetectedTextRegion], degraded: bool) -> bool {
    !degraded && !regions.is_empty()
}

/// Assigns each region a golden-set font, running the voting or routing stage as appropriate
/// (T012/T013/T014, wired for T034).
///
/// Two sequential checks, matching `routing`'s own design: lock state first, then per-block
/// routing. An unlocked volume votes (heavy classifier, batched); a locked one routes cheaply and
/// only sends genuine outliers back through the classifier.
/// `volume_id` is passed in rather than derived here: it must be the *Tankoubon-aware* scope
/// (`resolve_volume_id`), the same one compositing reads the golden set back under. Deriving it
/// locally as `VolumeId::from_archive` would write a grouped volume's pattern under a key the
/// renderer never looks at, silently defeating font matching for exactly the multi-archive volumes
/// it matters most for.
async fn resolve_fonts(
    ctx: &TranslationContextHandles,
    volume_id: &VolumeId,
    image: &RgbImage,
    regions: &mut [DetectedTextRegion],
) {
    if regions.is_empty() {
        return;
    }
    let mut pattern = match ctx.fonts_repo.get(volume_id).await {
        Ok(p) => p,
        Err(e) => {
            // A font-pattern read failure must not block translation itself — regions simply keep
            // no resolved font and compositing falls back (FR-008a's independent-fallback rule).
            tracing::warn!(error = %e, "font pattern unavailable; compositing will use the fallback face");
            return;
        }
    };

    if routing::is_locked(&pattern) {
        route_locked_volume(ctx, image, regions, &mut pattern).await;
    } else {
        accumulate_and_maybe_lock(image, regions, &mut pattern).await;
    }

    if let Err(e) = ctx.fonts_repo.save(&pattern).await {
        tracing::warn!(error = %e, "failed to persist the volume font pattern");
    }
}

/// Crops every region out of the page for the classifier.
fn candidates_for(image: &RgbImage, regions: &[DetectedTextRegion]) -> Vec<VoteCandidate> {
    regions
        .iter()
        .map(|r| VoteCandidate {
            crop: lanrurugi_ocr::style_estimate::crop_region(image, &r.bounding_box)
                .unwrap_or_else(|| RgbImage::new(1, 1)),
            text: r.source_text.clone(),
            is_bold: r.is_bold,
            is_cover: r.is_cover,
        })
        .collect()
}

/// Voting stage: classify this page's blocks and add them to the pool, locking if there's enough
/// evidence now.
async fn accumulate_and_maybe_lock(
    image: &RgbImage,
    regions: &mut [DetectedTextRegion],
    pattern: &mut VolumeFontPattern,
) {
    let candidates = candidates_for(image, regions);
    let is_cover: Vec<bool> = candidates.iter().map(|c| c.is_cover).collect();

    let classifications = match lanrurugi_fontcache::voting::classify_batch(candidates).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "font classification failed; leaving fonts unresolved");
            return;
        }
    };

    let paired: Vec<(Classification, bool)> = classifications
        .iter()
        .cloned()
        .zip(is_cover.iter().copied())
        .collect();
    let locked = lanrurugi_fontcache::voting::accumulate_votes(pattern, &paired);
    if locked {
        tracing::info!(volume = %pattern.volume_id, golden_set = ?pattern.golden_set, "volume font pattern locked");
    }

    // Even before locking, a block's own classification is the best available answer for how to
    // render it — better than dropping to the fallback face for the whole pre-lock stretch.
    for (region, classification) in regions.iter_mut().zip(classifications) {
        region.font = Some(classification.font.to_string());
    }
}

/// Locked-volume fast path: route among the golden set, sending only outliers to meltdown.
async fn route_locked_volume(
    ctx: &TranslationContextHandles,
    image: &RgbImage,
    regions: &mut [DetectedTextRegion],
    pattern: &mut VolumeFontPattern,
) {
    let mut outliers: Vec<usize> = Vec::new();

    for (index, region) in regions.iter_mut().enumerate() {
        match routing::route_block(
            pattern,
            region.bounding_box.w,
            region.bounding_box.h,
            &region.source_text,
        ) {
            RouteDecision::Matched(font) => {
                region.font = Some(font.to_string());
                ctx.telemetry.record_font_match(true);
            }
            RouteDecision::Outlier => {
                outliers.push(index);
                ctx.telemetry.record_font_match(false);
            }
            // Can't happen — `is_locked` was checked by the caller — but treated as "no opinion"
            // rather than unreachable!(), so a future refactor can't turn it into a panic.
            RouteDecision::NotLocked => {}
        }
    }

    if outliers.is_empty() {
        return;
    }

    // Meltdown re-classification: one batched dispatch for every outlier on the page, never one
    // per block (constitution Principle III).
    let outlier_regions: Vec<DetectedTextRegion> =
        outliers.iter().map(|&i| regions[i].clone()).collect();
    let candidates = candidates_for(image, &outlier_regions);

    let classifications = match lanrurugi_fontcache::meltdown::reclassify_batch(candidates).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "meltdown re-classification failed; outliers keep the fallback face");
            return;
        }
    };

    for (&index, classification) in outliers.iter().zip(classifications.iter()) {
        if let Some(font) = lanrurugi_fontcache::meltdown::record_meltdown(pattern, classification)
        {
            regions[index].font = Some(font.to_string());
        }
    }

    let promoted = lanrurugi_fontcache::meltdown::promote_recurring_meltdowns(pattern);
    if !promoted.is_empty() {
        tracing::info!(volume = %pattern.volume_id, ?promoted, "promoted recurring meltdown fonts into the golden set");
    }
}

/// Translates and composites one page, filling the rendered-image cache.
///
/// This is the function that makes a translated page actually exist. Every step it runs is
/// persisted before the next begins, so an interruption anywhere costs only the steps after it:
/// detection survives a translation failure, and translation survives a compositing failure
/// (research.md §16).
pub async fn translate_page(
    state: &AppState,
    ctx: &TranslationContextHandles,
    archive_id: &ArchiveId,
    page: PageNumber,
) -> Result<(), PipelineError> {
    let volume_id = resolve_volume_id(state, archive_id).await;

    // --- 0. Bubble segmentation, once, up front --------------------------------------------------
    // A real 2026-09-14 incident: this used to run once inside `ensure_detected` (for bubble-aware
    // region merging) and again inside `composite_and_cache` (for shape-accurate page erasure) —
    // two separate `Image`-worker RPC calls per page. On a cache-miss page those two calls landed
    // close enough together that the `Recognize` worker (still finishing `run_batch`'s own
    // recognition pass) and the `Image` worker ended up contending for the same card's VRAM at
    // once, producing `Available memory of 0` CUDA allocation failures and garbled/near-empty OCR
    // output — the exact cross-worker VRAM race issue #103's `Recognize`/`Image` process split
    // (`crate::gpu_worker_client::WorkerKind`) was built to prevent, just triggered by two calls
    // within one request instead of two long-lived processes. Running it here, exactly once, and
    // threading the same result into both `ensure_detected` (merging) and `composite_and_cache`
    // (erasure) removes the second call entirely rather than just moving where the race happens.
    let image_worker_for_bubbles = Arc::clone(&ctx.image_worker);
    let bubble_page_image = load_page_image(state, archive_id, page).await?.0;
    let bubbles = lanrurugi_core::concurrency::run_blocking(move || {
        use lanrurugi_ocr::bubble_segment::BubbleSegmenterHandle;
        BubbleSegmenterHandle::detect(&*image_worker_for_bubbles, &bubble_page_image)
            .inspect_err(|e| {
                tracing::warn!(error = %e, "bubble segmentation failed for this page; regions stay unmerged and erasure falls back to rectangular boxes")
            })
            .ok()
    })
    .await
    .map_err(|e| PipelineError::Ocr(e.to_string()))?;

    // --- 1. Detection (persisted) --------------------------------------------------------------
    let (detected, degraded) =
        detect_with_degradation(state, ctx, archive_id, page, bubbles.as_deref()).await?;

    if detected.is_empty() {
        if degraded {
            // Every candidate region was lost to the same infrastructure failure, so "this page
            // has no text" is a conclusion about the GPU worker, not about the page. Caching the
            // untouched original here would make that conclusion permanent on disk — exactly the
            // hazard `should_persist_detection` already blocks one layer up. Not an `Err`
            // either: that marks the page `Failed`, which the scheduler never auto-retries, so
            // it would trade a stuck disk cache for an equally stuck in-memory state. Leaving it
            // un-cached and not-ready lets the next poll re-run the whole pipeline.
            tracing::warn!(
                %archive_id,
                %page,
                "recognition degraded by an infrastructure failure and left no regions; not \
                 caching this page so a later request can retry"
            );
            return Ok(());
        }
        // A page with no text is fully "translated" the moment it's detected — there is nothing to
        // draw, so the original page is already the correct rendering. Cache the original bytes so
        // the API can answer "ready" instead of staying in the 202/poll loop forever; the byte
        // cost is bounded by the normal reader image-cache quota.
        cache_original_page(state, ctx, archive_id, page).await?;
        tracing::debug!(%archive_id, %page, "no text regions; cached original page");
        return Ok(());
    }

    // --- 2. Reuse any translation already stored for this exact language/provider ---------------
    // The persisted translated record is authoritative (research.md §16): a page whose regions are
    // already translated must never be re-sent to a billed provider, even if the rendered image
    // was swept out of the cache.
    let stored = ctx
        .regions
        .get_translated(
            archive_id,
            page,
            &ctx.target_language,
            ctx.provider.as_str(),
        )
        .await
        .ok()
        .flatten();

    let regions = match stored {
        Some(stored) if stored.iter().all(|r| r.translated_text.is_some()) => {
            // Everything already translated — skip straight to compositing (the §16 rebuild path).
            stored
        }
        stored => {
            // Carry over whatever partial translations exist so `translate_batch` only pays for
            // the blocks still missing one.
            let mut regions = detected;
            if let Some(stored) = stored {
                for region in &mut regions {
                    if let Some(prior) = stored.iter().find(|s| {
                        s.source_text == region.source_text && s.translated_text.is_some()
                    }) {
                        region.translated_text = prior.translated_text.clone();
                    }
                }
            }
            translate_and_persist(state, ctx, &volume_id, archive_id, page, regions).await?
        }
    };

    // If the backend returned no usable translations at all, a page that does contain text is a
    // failure (FR-019), not a success with an empty page. Letting it fall through to compositing
    // would cache the untouched original as if it were a translated result and mask the error.
    if !regions.iter().any(|r| r.translated_text.is_some()) {
        return Err(PipelineError::Translation(
            TranslationError::MalformedResponse("provider returned no usable translations".into()),
        ));
    }

    // A *partial* miss (some regions translated, some not) doesn't fail the page — FR-019 only
    // covers the all-or-nothing case above — but it must not stay invisible either: this is exactly
    // the "one speech bubble, several regions, only some came back translated" bug (issue found
    // 2026-09-14 via a reader screenshot with no corresponding log line anywhere). Compositing still
    // proceeds; the untranslated regions render as original text, same as `FR-008a`'s existing
    // per-region fallback.
    let untranslated = regions
        .iter()
        .filter(|r| r.translated_text.is_none())
        .count();
    if untranslated > 0 {
        tracing::warn!(
            %archive_id,
            %page,
            untranslated,
            total = regions.len(),
            "page partially translated; some regions will render as original text"
        );
    }

    // --- 5. Composite and cache ----------------------------------------------------------------
    composite_and_cache(
        state,
        ctx,
        archive_id,
        page,
        &regions,
        bubbles.as_deref(),
        degraded,
    )
    .await
}

/// The billed half: context-assembled batched translation, then persistence of both the glossary
/// and the translated regions.
async fn translate_and_persist(
    state: &AppState,
    ctx: &TranslationContextHandles,
    volume_id: &VolumeId,
    archive_id: &ArchiveId,
    page: PageNumber,
    regions: Vec<DetectedTextRegion>,
) -> Result<Vec<DetectedTextRegion>, PipelineError> {
    // The glossary is both the context source and a write target — `translate_batch` assembles
    // context from it (T019b) and captures newly-seen terms into it (FR-007a).
    let mut glossary = ctx
        .glossary
        .get(volume_id)
        .await
        .unwrap_or_else(|_| lanrurugi_translate::TerminologyGlossary::new(volume_id));

    let chapter_name = resolve_chapter_name(state, archive_id, page.get()).await;

    let outcome = translate_with_configured_provider(
        state,
        ctx,
        &mut glossary,
        vec![PageWork {
            archive_id: archive_id.clone(),
            page_number: page,
            regions,
            chapter_name: chapter_name.clone(),
        }],
    )
    .await?;

    let translated = outcome
        .pages
        .into_iter()
        .next()
        .map(|p| p.regions)
        .unwrap_or_default();

    // --- 3. Persist the authoritative record BEFORE rendering ---------------------------------
    ctx.regions
        .save_translated(
            archive_id,
            page,
            &ctx.target_language,
            ctx.provider.as_str(),
            &translated,
        )
        .await
        .map_err(|e| PipelineError::Storage(e.to_string()))?;

    if !outcome.captured_terms.is_empty() {
        if let Err(e) = ctx.glossary.save(&glossary).await {
            tracing::warn!(error = %e, "failed to persist captured glossary terms");
        } else {
            for term in &outcome.captured_terms {
                // The candidate this call itself just captured — matched by (archive, chapter) so
                // an ambiguous term (multiple sources) still reports the one actually written here,
                // not some other archive's unrelated translation for the same term string.
                let translation = glossary.entries.get(term).and_then(|candidates| {
                    candidates
                        .iter()
                        .find(|e| {
                            e.archive_id == archive_id.as_str() && e.chapter_name == chapter_name
                        })
                        .map(|e| e.translation.as_str())
                });
                crate::activity::record_glossary_change(
                    state,
                    crate::activity::GlossaryActor::Automatic("translation"),
                    lanrurugi_storage::activity::action_types::TRANSLATION_GLOSSARY_CAPTURE,
                    volume_id.as_str(),
                    term,
                    translation,
                )
                .await;
            }
        }
    }

    // --- 4. Usage accounting (FR-014) ----------------------------------------------------------
    if let Some(tokens) = outcome.total_tokens {
        if let Err(e) = ctx
            .budget
            .record(ctx.provider.as_str(), archive_id, page, tokens)
            .await
        {
            // Losing a usage counter must not lose the translation the user already paid for.
            tracing::warn!(error = %e, "failed to record translation usage");
        }
    }

    // --- 5. LLM-call activity telemetry (issue #100) -------------------------------------------
    // `outcome.usage` is `None` only when `translate_batch` never actually called the provider at
    // all (everything in this batch was already translated) — genuinely nothing to record then,
    // not a failure.
    if let Some(usage) = &outcome.usage {
        crate::activity::record_llm_call(
            state,
            archive_id.as_str(),
            page.get(),
            &ctx.target_language,
            usage,
        )
        .await;
    }

    Ok(translated)
}

/// Draws the translated regions over the page and stores the rendering (T029/T034).
///
/// `bubbles` comes from `translate_page`'s own single up-front segmentation call — see that
/// function's doc comment on why this no longer runs its own segmentation RPC (issue found
/// 2026-09-14: a second in-request `Image`-worker call here raced the `Recognize` worker over VRAM
/// on a cache-miss page).
///
/// `degraded` carries detection's own infrastructure-failure verdict all the way down to the
/// disk cache (issue #101, second half): the rendered page is only ever as complete as the
/// regions it was drawn from, so a composite built on a degraded region set is still served to
/// this request but never written to `TranslationImageCache`. Without that, the Redis-side fix
/// alone just moved the permanent staleness one layer down — confirmed live by a user
/// screenshot of a page cached on disk as the completely untranslated original after a real GPU
/// worker failure, with no retry path short of deleting the `.webp` by hand.
#[allow(clippy::too_many_arguments)]
async fn composite_and_cache(
    state: &AppState,
    ctx: &TranslationContextHandles,
    archive_id: &ArchiveId,
    page: PageNumber,
    regions: &[DetectedTextRegion],
    bubbles: Option<&[lanrurugi_ocr::bubble_segment::DetectedBubble]>,
    degraded: bool,
) -> Result<(), PipelineError> {
    if !regions.iter().any(|r| r.translated_text.is_some()) {
        return Ok(());
    }

    let (mut image, _is_cover, _total) = load_page_image(state, archive_id, page).await?;

    let volume_id = resolve_volume_id(state, archive_id).await;
    let golden_set: Vec<FontId> = ctx
        .fonts_repo
        .get(&volume_id)
        .await
        .map(|p| p.golden_set)
        .unwrap_or_default();

    let font_library = Arc::clone(&ctx.font_library);
    let image_worker = Arc::clone(&ctx.image_worker);
    let region_count = regions.len();
    let regions = regions.to_vec();
    let bubbles = bubbles.map(|b| b.to_vec());

    // Rasterising glyphs over a full-size page is CPU work — off the reactor, like every other
    // image operation in this codebase (constitution Principle III). Page erasure (GPU EP
    // integration plan v9: an RPC call to `lanrurugi-gpu-worker`, bridged back to a synchronous
    // call via `GpuWorkerClient`'s own `Handle::block_on` — see that type's trait impls) runs in
    // the same blocking closure for the same reason; `spawn_blocking`'s own thread is exactly the
    // kind of thread `block_on` is safe to call from (never the async reactor itself).
    let bytes = lanrurugi_core::concurrency::run_blocking(move || {
        use lanrurugi_inpaint::InpainterHandle;

        let font_set = font_library
            .font_set(&golden_set)
            .ok_or_else(|| PipelineError::Composite("no usable font available".into()))?;
        let plan = prepare_page_erase(&image, &regions, bubbles.as_deref());
        let erased = if plan.nothing_to_erase() {
            None
        } else {
            let (page_ref, model_mask, paste_mask) = plan.erase_request();
            // Fully-qualified: `GpuWorkerClient` also has its own inherent `async fn erase_page`
            // (the raw RPC call `InpainterHandle::erase_page` bridges to via `block_on`) with a
            // different signature — plain method-call syntax would otherwise be ambiguous here.
            InpainterHandle::erase_page(&*image_worker, page_ref, model_mask, paste_mask)
                .inspect_err(|e| {
                    // Wording matters here: this used to say "falling back to flat-fill
                    // backdrops only", which stopped being true when that rectangle fill was
                    // removed — a failed erase now leaves the page untouched and skips drawing
                    // the translation entirely, so the original lettering stays readable.
                    tracing::warn!(
                        error = %e,
                        "whole-page inpainting failed; leaving this page untranslated rather than \
                         drawing over un-erased lettering"
                    )
                })
                .ok()
        };
        finish_composite_page(&mut image, plan, &font_set, erased)
            .map_err(|e| PipelineError::Composite(e.to_string()))?;
        encode_webp(&image, COMPOSITE_QUALITY).map_err(|e| PipelineError::Composite(e.to_string()))
    })
    .await
    .map_err(|e| PipelineError::Composite(e.to_string()))??;

    if !should_persist_composite(degraded) {
        tracing::warn!(
            %archive_id,
            %page,
            region_count,
            "recognition hit an infrastructure failure; serving this composite without caching it \
             so a later request can re-render the full page"
        );
        return Ok(());
    }

    let key = TranslationCacheKey::new(
        archive_id.clone(),
        page,
        ctx.target_language.clone(),
        ctx.provider.as_str(),
    );
    ctx.cache
        .put(&key, &bytes)
        .await
        .map_err(|e| PipelineError::Storage(e.to_string()))?;

    tracing::info!(%archive_id, %page, "translated page composited and cached");
    Ok(())
}

/// Whether a freshly-rendered page image is safe to keep in the on-disk cache.
///
/// The disk-cache counterpart of [`should_persist_detection`], and deliberately the same rule:
/// a degraded detection produces a composite that may be missing some — or, as really happened,
/// all — of its translations, and `translation::get_page_translation` short-circuits on a disk
/// hit without re-running the pipeline, so caching one freezes that partial rendering forever.
fn should_persist_composite(degraded: bool) -> bool {
    !degraded
}

/// Encodes and caches the untouched original page image.
///
/// Used for pages where OCR found no text at all. The page *is* the correct translated rendering
/// in that case, but the API still needs a cache entry so it can return `ready` rather than
/// reporting not-ready forever (the scheduler's `Ready` state alone is not observable by the
/// HTTP layer).
async fn cache_original_page(
    state: &AppState,
    ctx: &TranslationContextHandles,
    archive_id: &ArchiveId,
    page: PageNumber,
) -> Result<(), PipelineError> {
    let (image, _is_cover, _total) = load_page_image(state, archive_id, page).await?;

    let bytes = lanrurugi_core::concurrency::run_blocking(move || {
        encode_webp(&image, COMPOSITE_QUALITY).map_err(|e| PipelineError::Composite(e.to_string()))
    })
    .await
    .map_err(|e| PipelineError::Composite(e.to_string()))??;

    let key = TranslationCacheKey::new(
        archive_id.clone(),
        page,
        ctx.target_language.clone(),
        ctx.provider.as_str(),
    );
    ctx.cache
        .put(&key, &bytes)
        .await
        .map_err(|e| PipelineError::Storage(e.to_string()))?;

    tracing::debug!(%archive_id, %page, "cached untouched original page for translation");
    Ok(())
}

/// The volume scope for an archive: its Tankoubon grouping if it has one, else the archive itself.
///
/// Mirrors `translation::resolve_volume_id` so the glossary and font pattern agree on scope no
/// matter which path reached them.
pub async fn resolve_volume_id(state: &AppState, archive_id: &ArchiveId) -> VolumeId {
    match state.repos.groupings.list_all().await {
        Ok(groupings) => groupings
            .into_iter()
            .find(|g| g.archives.contains(archive_id))
            .map(|g| VolumeId::from(g.tankid.as_str()))
            .unwrap_or_else(|| VolumeId::from_archive(archive_id)),
        Err(_) => VolumeId::from_archive(archive_id),
    }
}

// ---------------------------------------------------------------------------------------------
// Look-ahead scheduling (T039)
// ---------------------------------------------------------------------------------------------

/// The live look-ahead state, one scheduler per reading session (archive).
///
/// [`PrefetchScheduler`] is a pure state machine with no driver of its own — it answers "which
/// pages still need work" but never runs any. This type is that missing driver: it holds the
/// scheduler, receives the reader's position from the translation endpoint, and spawns the actual
/// per-page work.
pub struct TranslationScheduler {
    /// Per-archive scheduling state. Keyed by archive because a look-ahead window is scoped to one
    /// reading session (`PrefetchScheduler`'s own doc comment), and abandoning one archive's
    /// window on navigate-away must not disturb another's.
    sessions: tokio::sync::Mutex<std::collections::HashMap<String, PrefetchScheduler>>,
    /// Caps how many *look-ahead* pages translate concurrently. Split out from
    /// `current_page_permits` (rather than one shared pool) so a burst of prefetch work can never
    /// make the reader wait behind it for the one page actually on screen — `tokio::sync::Semaphore`
    /// has no priority/preemption, it's strict FIFO, so the only way to guarantee the current page
    /// isn't queued behind prefetch is to give it a pool prefetch never touches. Total budget
    /// (`precompute_worker_budget()`) is unchanged — this only repartitions it.
    lookahead_permits: tokio::sync::Semaphore,
    /// Small reserved pool for the page the reader is actually looking at right now — see
    /// `lookahead_permits`'s doc comment. Sized `CURRENT_PAGE_RESERVED_PERMITS`, not the whole
    /// budget: reserving more would starve look-ahead of its own purpose (translating pages before
    /// the reader gets there).
    current_page_permits: tokio::sync::Semaphore,
}

/// How many of the total translation concurrency budget are reserved exclusively for the page the
/// reader is currently on (never touched by look-ahead) — see `TranslationScheduler::new`.
const CURRENT_PAGE_RESERVED_PERMITS: usize = 2;

/// Which permit pool a spawned page draws from — see `TranslationScheduler::lookahead_permits`'s
/// doc comment for why there are two pools instead of one shared queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagePriority {
    /// The page the reader is actually looking at right now.
    Current,
    /// A page being translated ahead of the reader reaching it.
    LookAhead,
}

impl std::fmt::Debug for TranslationScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslationScheduler")
            .field(
                "available_lookahead_permits",
                &self.lookahead_permits.available_permits(),
            )
            .field(
                "available_current_page_permits",
                &self.current_page_permits.available_permits(),
            )
            .finish_non_exhaustive()
    }
}

/// Hand-written rather than derived: a derived `Default` would build the semaphore with **zero**
/// permits, so every spawned page would wait forever on `acquire()`. `Default` must mean the same
/// thing as [`TranslationScheduler::new`] here.
impl Default for TranslationScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslationScheduler {
    pub fn new() -> Self {
        // Each in-flight page holds an OCR session lock plus an LLM round trip, so this budget is
        // about bounding memory and CPU contention, not maximising throughput — same total either
        // way, just split between the two pools (see `lookahead_permits`'s doc comment for why).
        //
        // On a small enough host (`precompute_worker_budget()` itself floors at 1, e.g. a
        // 4-core box under the test container's own `--cpus` cap yields budget 1) reserving
        // `CURRENT_PAGE_RESERVED_PERMITS` outright would leave `lookahead_permits` at zero —
        // permanently starving look-ahead, not just deprioritising it. Both pools get at least 1
        // permit; the total handed out may then exceed `total_budget` by at most 1 on such hosts,
        // which is an acceptable trade against a look-ahead pool that can never make progress.
        let total_budget = crate::recommend_precompute::precompute_worker_budget();
        let current_page_budget = CURRENT_PAGE_RESERVED_PERMITS.min(total_budget);
        let lookahead_budget = (total_budget - current_page_budget).max(1);
        Self {
            sessions: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            lookahead_permits: tokio::sync::Semaphore::new(lookahead_budget),
            current_page_permits: tokio::sync::Semaphore::new(current_page_budget),
        }
    }

    /// Drops a session's window (FR-015) — nothing keeps accruing cost for pages nobody will see.
    pub async fn abandon(&self, archive_id: &ArchiveId) {
        let mut sessions = self.sessions.lock().await;
        if let Some(scheduler) = sessions.get_mut(archive_id.as_str()) {
            scheduler.abandon_all();
        }
    }

    /// Returns a page's current look-ahead state, if this archive has a live scheduling session.
    ///
    /// Used by `get_page_translation` to turn a page that has already failed into the FR-019
    /// "unavailable" response instead of reporting not-ready forever: without this, a provider
    /// error would be silently hidden behind an endless 202/poll cycle.
    pub async fn state(&self, archive_id: &ArchiveId, page: PageNumber) -> Option<PageState> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(archive_id.as_str())
            .and_then(|scheduler| scheduler.state(archive_id, page).cloned())
    }

    /// Records that the reader is on `page` of `archive_id` and starts whatever work that implies.
    ///
    /// Returns immediately: the requested page and its look-ahead window are translated in
    /// background tasks, and the caller reports not-ready (FR-012). Called from
    /// `translation::get_page_translation` on every cache miss, which is what gives the scheduler
    /// its input — it has no other way to learn where the reader is.
    pub async fn on_reader_at(
        &self,
        state: &AppState,
        ctx: &TranslationContextHandles,
        archive_id: &ArchiveId,
        page: PageNumber,
        total_pages: u32,
    ) {
        // Which pool each spawned page draws its concurrency permit from — see
        // `lookahead_permits`'s doc comment for why the current page gets its own reserved pool
        // rather than sharing one queue with look-ahead.
        let mut wanted: Vec<(PageNumber, PagePriority)> = Vec::new();

        {
            let mut sessions = self.sessions.lock().await;
            let scheduler = sessions
                .entry(archive_id.as_str().to_string())
                .or_insert_with(PrefetchScheduler::new);

            // The page the reader is actually looking at is an explicit action, not background
            // spend — it is scheduled regardless of the look-ahead budget gate below (FR-013), and
            // draws from `current_page_permits` rather than `lookahead_permits` so it never queues
            // behind a burst of prefetch work for pages the reader hasn't reached yet.
            //
            // A page already `Queued`/`InFlight` is left alone (work is underway; the client is
            // polling for it). Anything else — never seen, or a previous `Failed`/`BudgetExhausted`
            // — is (re)scheduled. `PrefetchScheduler` deliberately never *auto*-retries a failure,
            // since that would re-bill for something known broken; but a request for the page the
            // user is looking at right now is not automatic retry, and without this a single
            // transient provider error would leave that page permanently untranslatable for the
            // rest of the session with no way to recover short of a restart.
            if should_schedule_current_page(scheduler.state(archive_id, page)) {
                scheduler.mark_queued(archive_id, page);
                wanted.push((page, PagePriority::Current));
            }

            // Look-ahead proper is budget-gated: a metered provider at its cap prefetches nothing.
            let usage = ctx
                .budget
                .snapshot(ctx.provider.as_str(), archive_id, page)
                .await
                .unwrap_or_default();

            let ahead = scheduler.pages_to_schedule(
                archive_id,
                page,
                ctx.settings.lookahead_pages,
                total_pages,
            );

            if PrefetchScheduler::may_prefetch(true, &usage) {
                for ahead_page in ahead {
                    scheduler.mark_queued(archive_id, ahead_page);
                    wanted.push((ahead_page, PagePriority::LookAhead));
                }
            } else {
                for ahead_page in ahead {
                    scheduler.mark_budget_exhausted(archive_id, ahead_page);
                }
                tracing::debug!(%archive_id, "look-ahead paused: usage budget exhausted");
            }
        }

        for (target, priority) in wanted {
            self.spawn_page(state, ctx, archive_id, target, priority);
        }
    }

    /// Spawns one page's translation as an independent task (FR-020: one page's failure is its
    /// own). `priority` picks which permit pool it draws from — see `lookahead_permits`'s doc
    /// comment.
    fn spawn_page(
        &self,
        state: &AppState,
        ctx: &TranslationContextHandles,
        archive_id: &ArchiveId,
        page: PageNumber,
        priority: PagePriority,
    ) {
        let state = state.clone();
        let ctx = ctx.clone();
        let archive_id = archive_id.clone();
        let scheduler = Arc::clone(&state.translation_scheduler);

        tokio::spawn(async move {
            // Bounds concurrent translations process-wide. Acquired inside the task so scheduling
            // never blocks the HTTP handler that triggered it.
            let permits = match priority {
                PagePriority::Current => &scheduler.current_page_permits,
                PagePriority::LookAhead => &scheduler.lookahead_permits,
            };
            let _permit = match permits.acquire().await {
                Ok(permit) => permit,
                Err(_) => return,
            };

            {
                let mut sessions = scheduler.sessions.lock().await;
                if let Some(s) = sessions.get_mut(archive_id.as_str()) {
                    s.mark_in_flight(&archive_id, page);
                }
            }

            let outcome = translate_page(&state, &ctx, &archive_id, page).await;

            let mut sessions = scheduler.sessions.lock().await;
            let Some(s) = sessions.get_mut(archive_id.as_str()) else {
                return;
            };
            match outcome {
                Ok(()) => {
                    s.mark_ready(&archive_id, page);
                    ctx.telemetry.record_lookahead(true);
                }
                Err(PipelineError::Translation(e)) => {
                    tracing::warn!(%archive_id, %page, error = %e, "page translation failed");
                    s.mark_failed(&archive_id, page, &e);
                    ctx.telemetry.record_lookahead(false);
                }
                Err(e) => {
                    tracing::warn!(%archive_id, %page, error = %e, "page translation failed");
                    // Non-provider failures (OCR, IO, compositing) reach the reader the same way
                    // (FR-019), so they're recorded through the same per-page failure state.
                    s.mark_failed(
                        &archive_id,
                        page,
                        &TranslationError::Unreachable(e.to_string()),
                    );
                    ctx.telemetry.record_lookahead(false);
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Lazily-initialised shared runtime (models + fonts)
// ---------------------------------------------------------------------------------------------

/// The OCR models and font faces, loaded at most once per process.
///
/// Lazy rather than eager at startup because translation is off by default (FR-007): a user who
/// never enables it must never pay the model-load cost, and a deployment with no model files
/// installed must still start normally (`model_discovery`'s own "a missing model is recoverable"
/// rule).
///
/// `OnceCell`, not a plain `Mutex<Option<_>>`: a mutex held across the entire (multi-second, worse
/// with a large `Inpainter` session pool — see that type's own incident note) load future would
/// serialize *every* concurrent translation request behind whichever one happens to be first,
/// even ones for a completely different archive/page that have nothing to do with each other.
/// Confirmed live, 2026-09-08: loading `Inpainter`'s 4-session pool (since reduced to 2) took
/// ~24s, and every other in-flight request — including some unrelated to translation entirely —
/// failed with 500/503 for that whole window, most likely a downstream resource (the Redis
/// connection pool) exhausted by requests piling up *holding a connection* while queued on this
/// lock, not the lock itself directly. `OnceCell::get_or_try_init` still only ever runs the
/// init future once (first caller wins, everyone else awaits its result cheaply), but never holds
/// a lock across unrelated work the way a `Mutex` guard spanning an `.await` does.
#[derive(Default)]
pub struct TranslationRuntime {
    inner: tokio::sync::OnceCell<Arc<LoadedRuntime>>,
}

impl std::fmt::Debug for TranslationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslationRuntime").finish_non_exhaustive()
    }
}

pub struct LoadedRuntime {
    pub engine: Arc<OcrEngine>,
    pub fonts: Arc<FontLibrary>,
    /// GPU EP integration plan v9 (issue #103): the real `Inpainter`/`BubbleSegmenter` (and their
    /// CUDA sessions) now live in a separate `lanrurugi-gpu-worker` subprocess this client talks
    /// to — see `crate::gpu_worker_client`'s own module doc. Whether a model is actually installed
    /// is no longer knowable at this `load()`'s own time (the worker hasn't necessarily started
    /// yet, since it's spawned lazily on first real use) — see `TranslationContextHandles::
    /// inpainter`/`bubble_segmenter`'s own doc comments for how that's handled instead.
    ///
    /// Two separate clients/subprocesses (added 2026-09-13, same issue #103), not one — see
    /// `crate::gpu_worker_client::WorkerKind`'s own doc comment for why all three models resident
    /// in one worker process was a real crash-loop incident.
    pub image_worker: Arc<crate::gpu_worker_client::GpuWorkerClient>,
}

/// Why the translation runtime couldn't be prepared. Both variants are configuration problems the
/// user can fix, so they're reported distinctly rather than as one generic failure (FR-021).
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("OCR model unavailable: {0}")]
    Model(String),
    #[error("no usable font found for compositing translated text")]
    NoFont,
}

impl TranslationRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the loaded runtime, loading it on first use. Concurrent callers during that first
    /// load all await the same in-progress attempt (`OnceCell`'s own contract) rather than each
    /// queueing behind a lock held across the whole thing — see this type's own doc comment.
    pub async fn get(&self) -> Result<Arc<LoadedRuntime>, RuntimeError> {
        self.inner
            .get_or_try_init(|| self.load())
            .await
            .map(Arc::clone)
    }

    async fn load(&self) -> Result<Arc<LoadedRuntime>, RuntimeError> {
        // GPU EP integration plan v9 (issue #103): `TextRecognizer`/`Inpainter`/`BubbleSegmenter`
        // (every model with a CUDA session) moved out of this process entirely, into
        // `lanrurugi-gpu-worker` — see `crate::gpu_worker_client`'s own module doc for why. This
        // `load()` now only needs the CPU-only `TextDetector` and the font library directly; both
        // GPU worker clients are created here but neither spawns its actual subprocess yet (that
        // happens lazily on first real RPC call — see `GpuWorkerClient::ensure_session`).
        //
        // Two clients, not one — see `gpu_worker_client::WorkerKind`'s own doc comment for the real
        // crash-loop incident this split fixes (all three models resident in one worker process
        // left an 8GB card with no VRAM free for the next inference).
        use crate::gpu_worker_client::{GpuWorkerClient, WorkerKind};
        let recognize_worker = Arc::new(GpuWorkerClient::new(WorkerKind::Recognize));
        tokio::spawn(Arc::clone(&recognize_worker).run_idle_reaper());
        let image_worker = Arc::new(GpuWorkerClient::new(WorkerKind::Image));
        tokio::spawn(Arc::clone(&image_worker).run_idle_reaper());

        // TextDetector construction and font parsing are both blocking; neither belongs on the
        // async reactor (constitution Principle III).
        let (detector, fonts) = lanrurugi_core::concurrency::run_blocking(|| {
            let paths = lanrurugi_ocr::model_discovery::ModelPaths::discover()
                .map_err(|e| RuntimeError::Model(e.to_string()))?;
            let detector = lanrurugi_ocr::detect::TextDetector::load(&paths.detection)
                .map_err(|e| RuntimeError::Model(e.to_string()))?;
            let fonts = FontLibrary::discover().ok_or(RuntimeError::NoFont)?;
            Ok::<_, RuntimeError>((detector, fonts))
        })
        .await
        .map_err(|e| RuntimeError::Model(e.to_string()))??;

        tracing::info!("translation runtime loaded (text detector + fonts; GPU workers start lazily on first use)");

        let engine = Arc::new(OcrEngine::new(
            detector,
            recognize_worker as Arc<dyn lanrurugi_ocr::batch::TextRecognizerHandle>,
        ));

        Ok(Arc::new(LoadedRuntime {
            engine,
            fonts: Arc::new(fonts),
            image_worker,
        }))
    }
}

/// Assembles everything a translation needs for one archive, loading models on first use.
///
/// Returns `Err` when the feature can't run at all (no model, no font) — the caller turns that into
/// FR-019's "translation unavailable" rather than leaving the reader polling forever.
pub async fn build_handles(
    state: &AppState,
    settings: TranslationSettings,
    provider: CloudProvider,
    target_language: String,
) -> Result<TranslationContextHandles, RuntimeError> {
    let runtime = state.translation_runtime.get().await?;

    Ok(TranslationContextHandles {
        settings,
        provider,
        target_language,
        regions: RegionRepository::new(state.redis.config.clone()),
        glossary: GlossaryRepository::new(state.redis.config.clone()),
        fonts_repo: FontPatternRepository::new(state.redis.config.clone()),
        budget: BudgetRepository::new(state.redis.config.clone()),
        cache: TranslationImageCache::new(&state.library.temp_dir),
        engine: Arc::clone(&runtime.engine),
        font_library: Arc::clone(&runtime.fonts),
        image_worker: Arc::clone(&runtime.image_worker),
        telemetry: Arc::clone(&state.translation_telemetry),
    })
}

/// The `toc` chapter name active at `page`, if `archive_id` has a table of contents and `page`
/// falls within one of its entries (issue #105) — the chapter this page belongs to is whichever
/// `toc` entry has the largest `page` not greater than the page being translated (a `toc` marks
/// where each chapter *starts*, so the active chapter is the most recent start at or before this
/// page). `None` for an archive with no `toc` at all, or a page before its first `toc` entry.
pub async fn resolve_chapter_name(
    state: &AppState,
    archive_id: &ArchiveId,
    page: u32,
) -> Option<String> {
    let archive = state.repos.archives.get(archive_id).await.ok()??;
    archive
        .toc
        .iter()
        .filter(|entry| entry.page <= page)
        .max_by_key(|entry| entry.page)
        .map(|entry| entry.name.clone())
}

/// Total page count for an archive — the look-ahead window's upper bound.
pub async fn total_pages(state: &AppState, archive_id: &ArchiveId) -> Option<u32> {
    let archive = state.repos.archives.get(archive_id).await.ok()??;
    let file = archive.file.clone();
    lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::archive_format::list_pages(std::path::Path::new(&file))
    })
    .await
    .ok()?
    .ok()
    .map(|pages| pages.len() as u32)
}

/// Drops look-ahead sessions that have nothing left in flight (FR-015).
///
/// Without this, `sessions` grows one entry per archive ever opened and never shrinks — a slow leak
/// over a long uptime. A session with no queued or in-flight page is one the reader has left (or
/// finished), and re-reading that archive simply builds a fresh window.
///
/// Registered as a periodic task in `lanrurugi-server::main`, alongside the other sweeps.
pub async fn sweep_finished_translation_sessions(state: &AppState) {
    let scheduler = &state.translation_scheduler;
    let mut sessions = scheduler.sessions.lock().await;

    let before = sessions.len();
    sessions.retain(|_, s| s.in_flight_count() > 0);
    let dropped = before - sessions.len();

    if dropped > 0 {
        tracing::debug!(dropped, "dropped finished translation look-ahead sessions");
    }
}

/// Whether an explicit request for the page the reader is looking at should (re)schedule work.
///
/// Split out from [`TranslationScheduler::on_reader_at`] so the rule is directly testable without a
/// Redis connection, an OCR model, or a provider — the orchestration around it needs all three.
fn should_schedule_current_page(state: Option<&PageState>) -> bool {
    match state {
        // Work is already underway and the client is polling for it.
        Some(PageState::Queued | PageState::InFlight) => false,
        // Never seen before.
        None => true,
        // `Ready` but reached here anyway means the rendered image was swept out of the cache
        // (`on_reader_at` is only called on a cache miss). Re-running rebuilds it from the
        // persisted regions with no provider call at all (research.md §16).
        Some(PageState::Ready) => true,
        // A previous failure or budget stop. Retrying on an explicit request is not the automatic
        // retry `PrefetchScheduler` refuses to do — without this a single transient error would
        // make the page permanently untranslatable for the rest of the session.
        Some(PageState::Failed(_) | PageState::BudgetExhausted) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_region() -> DetectedTextRegion {
        DetectedTextRegion::new(
            ArchiveId("0".repeat(40)),
            PageNumber(10),
            lanrurugi_ocr::entities::BoundingBox::new(100, 100, 200, 60),
            "こんにちは".into(),
            false,
        )
    }

    /// The issue #101 fix itself: a page whose recognition hit a transient GPU-worker failure is
    /// returned to the caller but never written to Redis, so the next request re-detects instead
    /// of replaying the same incomplete translation forever.
    #[test]
    fn a_degraded_result_is_never_persisted() {
        assert!(
            !should_persist_detection(&[a_region()], true),
            "an infrastructure failure must keep even a non-empty result out of the cache"
        );
    }

    /// The no-regression case: a clean, non-empty detection caches normally, so later requests hit
    /// that cache rather than paying for detection again.
    #[test]
    fn a_clean_non_empty_result_is_persisted() {
        assert!(should_persist_detection(&[a_region()], false));
    }

    /// Pre-existing rule, kept intact: an empty result is never trusted as permanent either, since
    /// a genuinely blank page and a page whose regions all got dropped downstream look identical
    /// here.
    #[test]
    fn an_empty_result_is_never_persisted() {
        assert!(!should_persist_detection(&[], false));
        assert!(!should_persist_detection(&[], true));
    }

    /// The second half of issue #101, found by a user's on-device screenshot after the Redis-side
    /// fix above was already in: the rendered page goes to a *disk* cache that
    /// `get_page_translation` reads before anything else, so a composite built from a degraded
    /// region set froze there just as permanently. The real incident cached a page as the
    /// completely untranslated original.
    #[test]
    fn a_degraded_composite_is_never_cached_on_disk() {
        assert!(
            !should_persist_composite(true),
            "a composite drawn from an infrastructure-degraded region set must not reach the \
             on-disk cache, or the next request hits it instead of re-rendering"
        );
    }

    /// The no-regression case, and the reason this is a flag rather than a blanket rule: a clean
    /// page must still cache, otherwise every single view re-runs the full inference pipeline.
    #[test]
    fn a_clean_composite_is_cached_on_disk() {
        assert!(should_persist_composite(false));
    }

    /// Both cache layers answer the same question the same way, which is the whole point of the
    /// follow-up fix — the disk cache was the one layer that did not.
    #[test]
    fn both_cache_layers_agree_on_a_degraded_result() {
        for degraded in [true, false] {
            assert_eq!(
                should_persist_detection(&[a_region()], degraded),
                should_persist_composite(degraded),
                "detection and composite caching must not disagree about a degraded page"
            );
        }
    }

    #[test]
    fn a_page_already_being_worked_on_is_not_rescheduled() {
        assert!(!should_schedule_current_page(Some(&PageState::Queued)));
        assert!(!should_schedule_current_page(Some(&PageState::InFlight)));
    }

    #[test]
    fn an_unseen_page_is_scheduled() {
        assert!(should_schedule_current_page(None));
    }

    #[test]
    fn an_explicit_request_retries_a_failed_page() {
        // The regression this guards: without it, one transient provider error left the page
        // permanently stuck at "not ready" for the rest of the session.
        assert!(should_schedule_current_page(Some(&PageState::Failed(
            "rate_limited".into()
        ))));
        assert!(should_schedule_current_page(Some(
            &PageState::BudgetExhausted
        )));
    }

    #[test]
    fn a_ready_page_whose_cache_was_swept_is_rebuilt() {
        // Reaching the scheduler at all means the rendered image is gone; the persisted regions
        // make rebuilding it free.
        assert!(should_schedule_current_page(Some(&PageState::Ready)));
    }

    #[test]
    fn the_scheduler_default_has_real_permits() {
        // A derived `Default` would produce zero permits and deadlock every spawned page.
        let scheduler = TranslationScheduler::default();
        assert!(
            scheduler.lookahead_permits.available_permits() > 0,
            "Default must match new(), not leave the look-ahead semaphore empty"
        );
        assert!(
            scheduler.current_page_permits.available_permits() > 0,
            "Default must match new(), not leave the current-page semaphore empty"
        );
    }

    #[test]
    fn current_page_permits_are_reserved_out_of_the_same_total_budget() {
        let scheduler = TranslationScheduler::default();
        let total = crate::recommend_precompute::precompute_worker_budget();
        // On a large enough host, splitting must not change the total budget. On a small enough
        // one (`total` itself near the floor of 1), `TranslationScheduler::new`'s own doc comment
        // explains why look-ahead still gets a minimum of 1 permit even if that pushes the grand
        // total one above `total` — so this only asserts the split doesn't shrink the total below
        // what was budgeted, not that it's exactly equal.
        assert!(
            scheduler.lookahead_permits.available_permits()
                + scheduler.current_page_permits.available_permits()
                >= total,
            "splitting into two pools must not shrink the total concurrency budget"
        );
        assert_eq!(
            scheduler.current_page_permits.available_permits(),
            CURRENT_PAGE_RESERVED_PERMITS.min(total),
        );
        assert!(
            scheduler.lookahead_permits.available_permits() >= 1,
            "look-ahead must always get at least one permit, even on a tiny host"
        );
    }
    #[test]
    fn a_fragment_region_is_dropped_when_the_alternate_candidate_contains_it() {
        let mut merged = a_region();
        merged.bounding_box = lanrurugi_ocr::entities::BoundingBox::new(955, 1036, 99, 86);
        merged.source_text = "娘スー母猫".into();
        merged.alternate_source_text = Some("スーツ母娘".into());

        let mut fragment = a_region();
        fragment.bounding_box = lanrurugi_ocr::entities::BoundingBox::new(1000, 1079, 50, 86);
        fragment.source_text = "ーッ".into();

        let mut regions = vec![merged, fragment];
        deduplicate_overlapping_regions(&mut regions);
        assert_eq!(regions.len(), 1, "the overlapping fragment must be dropped");
        assert_eq!(regions[0].source_text, "娘スー母猫");
        assert_eq!(
            regions[0].alternate_source_text.as_deref(),
            Some("スーツ母娘")
        );
    }

    #[test]
    fn overlapping_regions_with_unrelated_text_are_both_kept() {
        let mut first = a_region();
        first.bounding_box = lanrurugi_ocr::entities::BoundingBox::new(0, 0, 100, 100);
        first.source_text = "こんにちは".into();
        let mut second = a_region();
        second.bounding_box = lanrurugi_ocr::entities::BoundingBox::new(20, 20, 100, 100);
        second.source_text = "さようなら".into();

        let mut regions = vec![first, second];
        deduplicate_overlapping_regions(&mut regions);
        assert_eq!(regions.len(), 2);
    }
}
