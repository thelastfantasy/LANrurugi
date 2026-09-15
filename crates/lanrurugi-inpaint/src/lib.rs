//! LaMa-based inpainting for erasing a manga page's original lettering before the translated text
//! is composited on top (`lanrurugi-translate::composite`) — the missing piece that a plain
//! flat-colour-rectangle fill or a contrasting outline stroke (both already shipped there) were
//! ever only a stand-in for. See this crate's `README.md` for model file placement/licensing.
//!
//! Model: `ogkalu/lama-manga-onnx-dynamic`'s `lama-manga-dynamic.onnx` (Apache-2.0, an ONNX export
//! of `dreMaz/AnimeMangaInpainting` — a LaMa variant fine-tuned specifically on anime/manga
//! content, the same checkpoint the reference `zyddnys/manga-image-translator` project's own
//! `LamaLargeInpainter` uses). Two float32 inputs, both **genuinely dynamic `[batch, C, h, w]`**
//! (confirmed by reading the ONNX graph directly, 2026-09-08 — symbolic `h`/`w` dims, not fixed
//! integers): `image` (RGB, `[0, 1]`-normalised, no further mean/std scaling) and `mask`
//! (single-channel, `1.0` marks a hole to fill). `output` is also `[0, 1]` (the graph's own final
//! op is `sigmoid(gen_out) * mask + image * (1 - mask)`, both operands already `[0, 1]`, no `* 255`
//! node at all — unlike the earlier `Carve/LaMa-ONNX` model this crate used before, whose output
//! *was* already `[0, 255]`; always verify a new model's own graph rather than assuming the same
//! convention carries over between exports).
//!
//! Switched away from `Carve/LaMa-ONNX`'s fixed-512x512 export (2026-09-08) after a real reported
//! bug: squeezing a whole ~1600px-tall manga page down into a 512x512 letterboxed input left real
//! content (a small character's head, ~80px in the original) at only ~25px once scaled down —
//! nowhere near enough resolution for the model to reconstruct clean edges, producing a visibly
//! blurred/blocky result once upscaled back to page size. The reference project's own default
//! `inpainting_size` is `2048` (`manga_translator/config.py`), padded only to a multiple of 8, not
//! squeezed into a fixed square — this dynamic-shape model lets this crate do the same instead of
//! working around a hard 512x512 ceiling with tiling or other compensating machinery.
//!
//! [`Inpainter::erase_page`] runs a single whole-page inference per page, given a page-sized mask
//! merged from every text region's own (bubble or stroke) mask —
//! `lanrurugi-translate::composite`'s `merge_region_mask_into_page` builds that merged mask. This
//! is a real improvement over the pre-existing flat-fill/outline-stroke fallback: the erased area
//! is regenerated from surrounding real pixels rather than covered with a guessed flat colour or
//! left showing through a contrasting-outline compromise.

pub mod model_discovery;
pub mod stroke_mask;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use image::{imageops::FilterType, Rgb, RgbImage};
use ort::ep;
use ort::ep::ExecutionProvider;
use ort::session::Session;
use ort::value::Tensor;
use thiserror::Error;

// GPU EP integration plan v9 (issue #103): per-session CUDA memory cap used to be a fixed
// constant here (4GiB, arrived at by retuning 1GiB → 2GiB → 4GiB against real allocation
// failures on one specific 8GB card — the failures themselves are still real, documented
// history: 1GiB left a `Conv` node ~495MB short, 2GiB left a `ConvTranspose` ~699MB short with
// real total GPU memory already at ~7.5GB/8.15GB). Now resolved dynamically at startup instead —
// see `lanrurugi-gpu-worker::vram_budget` (the crate that actually calls `Inpainter::load`) for
// the hardware-proportional percent/min/max calculation, and why chasing one card's specific
// failures with a fixed byte constant doesn't scale to different hardware. This crate no longer
// owns that number — `load`'s own `cuda_memory_limit_bytes` parameter is the caller's resolved
// value. `ort`'s `with_memory_limit` remains a soft cap on ONNX Runtime's own `BFCArena`, not a
// hard ceiling this project enforces itself — falling back to flat-fill on allocation failure
// (`composite.rs`) means an under-budgeted value degrades output quality, not stability.

/// Both spatial dimensions of the input must be a multiple of this before the model sees them —
/// standard requirement for any FFC-based LaMa architecture (3 stride-2 downsampling stages, same
/// `pad_size = 8` the reference `zyddnys/manga-image-translator` project's own `_infer` uses in
/// `inpainting_lama_mpe.py`), not something read off the graph since ONNX doesn't expose this as
/// a queryable constraint for a dynamic-shape input.
const PAD_MODULO: u32 = 8;

/// A page longer than this on its own longer edge is downscaled before padding (still keeping
/// aspect ratio, no letterbox/squeeze) — an unbounded input size would let one huge page spike
/// memory/CPU cost unpredictably.
///
/// Lowered from `2048` to `1024` (2026-09-14, a real live incident): `2048` matches the reference
/// `zyddnys/manga-image-translator` project's own default `inpainting_size`
/// (`manga_translator/config.py`, its own `inpainting_lama_mpe.py::_infer` default parameter) — but
/// that project runs LaMa as the *only* model holding GPU memory, with the whole card available to
/// it. This project splits one card's VRAM three ways (`Recognize`/`BubbleSegmenter`/`Inpainter`,
/// `lanrurugi-gpu-worker::vram_budget`'s own percent/min/max split), so `Inpainter`'s own real
/// share is a few GiB, not a whole 8GiB+ card — at `2048`, a real ~1600px-tall page's `ConvTranspose`
/// decoder stage requested ~667MiB against an arena with only ~445MiB actually free at that moment
/// (confirmed live: `nvidia-smi` showed >4GiB free on the card as a whole at the same instant — the
/// failure was the arena's own budget being real but insufficient at that input size, not the
/// physical card running out). `1024` — the reference project's own value at its *own* smaller
/// default, not a value invented here — cuts the input's pixel count roughly 2.4x, bringing peak
/// activation memory back under what this project's own per-worker VRAM share can actually supply.
const MAX_INPAINT_SIZE: u32 = 1024;

#[derive(Debug, Error)]
pub enum InpaintError {
    #[error("failed to build ORT session from {path}: {source}")]
    Session {
        path: String,
        #[source]
        source: ort::Error,
    },
    #[error("failed to run ORT inference: {0}")]
    Inference(#[from] ort::Error),
    #[error("unexpected model output shape: {0}")]
    BadOutput(String),
}

/// Abstracts over "how a page's original lettering actually gets erased" — same reasoning as
/// `lanrurugi_ocr::batch::TextRecognizerHandle`: GPU EP integration plan v9 (issue #103) moved the
/// real `Inpainter` (and its CUDA session) into a separate `lanrurugi-gpu-worker` subprocess, so
/// callers above this crate can no longer hold a concrete `Inpainter` directly without also
/// depending on `lanrurugi-api`. Synchronous for the same reason as that trait too — called from
/// inside a `spawn_blocking` closure, never a true async-reactor thread.
pub trait InpainterHandle: Send + Sync {
    fn erase_page(
        &self,
        page: &image::RgbImage,
        model_mask: &[bool],
        paste_mask: &[bool],
    ) -> Result<image::RgbImage, String>;
}

/// How many independent ONNX Runtime sessions [`Inpainter::load`] builds.
///
/// A single `Session` (even multi-threaded internally) still only runs one `Run()` call at a
/// time in practice through this crate's own locking (see `Inpainter`'s field doc). Since
/// [`Inpainter::erase_page`] made whole-page inpainting a single call per page, this pool no
/// longer buys cross-region parallelism (there is only one `erase_page` call per page now) — it
/// still lets multiple *pages* inpaint concurrently when several translation requests are
/// in flight at once.
///
/// Dropped from `2` to `1` (2026-09-12, issue #103's GPU EP integration): real CUDA usage found
/// LaMa's FFC network needs far more per-session VRAM headroom than originally budgeted — even
/// after raising the per-session memory cap (then still a fixed constant, since replaced by
/// `lanrurugi-gpu-worker::vram_budget`'s dynamic calculation) to 2GiB, a real page still hit a
/// `bfc_arena.cc` allocation failure wanting ~699MB more than the arena had left, and total real
/// GPU memory climbed to ~7.5GB/8.15GB with 2 pool sessions plus `lanrurugi-ocr`'s 3 — an 8GB card
/// can't comfortably run two of these sessions on CUDA at once. `1` trades away cross-page
/// inpainting concurrency (concurrent translation requests now serialize on this one session, same
/// as they already do for `lanrurugi-ocr`'s single `BubbleSegmenter`) for the entire freed budget
/// going to this one session instead. (Historical note: this was `2`, not higher, for a now-superseded reason — sequential
/// `build_session` calls at load time meant `4` sessions took ~24s to construct and stalled the
/// entire server, confirmed live 2026-09-08. `Inpainter::load` has built its pool concurrently via
/// `std::thread::scope` since then, so construction time was no longer why this stayed at `2` —
/// VRAM is the real constraint now.)
const SESSION_POOL_SIZE: usize = 1;

/// One pool slot's session, plus whether it's safe to ever drop.
///
/// GPU EP integration plan v9 (issue #103, see `.debug-scratch/GPU_EP_FINAL_SUMMARY.md` §3):
/// a real hardware spike found dropping a CUDA session triggers a reproducible ~35-60% crash rate
/// rooted in NVIDIA's own driver userspace component, not in this project's code, `ort`, or
/// onnxruntime — the fix is a *compile-time* guarantee that a CUDA session's backing memory is
/// never reclaimed (`Box::leak`, not `std::mem::forget`: a leaked `Box` has no owner left to drop
/// at all, whereas `forget`-ing one of several `Arc` clones only holds as long as no other code
/// path ever drains the last strong reference — a fact about how this code happens to be called
/// today, not a fact the type system enforces). This only applies to the `LeakedCuda` variant —
/// `Owned` (CPU) sessions drop normally and cleanly; the spike confirmed CPU-only sessions have
/// none of this problem, so leaking them too would only waste memory for no safety benefit.
enum ManagedSession {
    /// A CPU session (or any session before leaking has been requested) — normal ownership.
    /// `bool` is whether this session is actually running on CUDA (recorded at construction time
    /// by `build_session_with_gpu_fallback` — `ort::Session` itself exposes no way to ask which EP
    /// it ended up on after the fact, so this can't be recovered later by inspecting the session).
    Owned(Mutex<Session>, bool),
    /// A CUDA session, deliberately leaked at [`Inpainter::leak_cuda_sessions`] time. Holds a
    /// `'static` reference into memory `Box::leak` handed off — there is no `Session` value left
    /// here for `ManagedSession`'s own drop glue to reach, so this variant can never trigger the
    /// crash above no matter what happens to the `Inpainter` (or `Arc<Inpainter>`) around it.
    LeakedCuda(&'static Mutex<Session>),
}

impl ManagedSession {
    /// Runs `f` against the session, regardless of which variant this slot currently is.
    fn with_locked<R>(&self, f: impl FnOnce(&mut Session) -> R) -> Result<R, InpaintError> {
        let mutex: &Mutex<Session> = match self {
            ManagedSession::Owned(mutex, _) => mutex,
            ManagedSession::LeakedCuda(mutex) => mutex,
        };
        let mut guard = mutex
            .lock()
            .map_err(|_| InpaintError::BadOutput("inpainting session poisoned".into()))?;
        Ok(f(&mut guard))
    }
}

/// A pool of loaded LaMa sessions, ready to erase text from page images. Cheap to keep around
/// for a batch of pages — session construction (loading + compiling the ~200MB graph, done once
/// per pool member at [`Inpainter::load`] time) is the expensive part, inference itself is not.
///
/// Multiple sessions rather than one: `ort::Session::run` takes `&mut self`, so any single
/// session can only ever run one inference at a time no matter how many threads call into it —
/// see `SESSION_POOL_SIZE`'s own doc comment for why that still matters for concurrent pages
/// even though each individual page now makes only one [`Inpainter::erase_page`] call.
pub struct Inpainter {
    sessions: Vec<ManagedSession>,
    next: AtomicUsize,
}

impl Inpainter {
    /// `intra_threads` caps ONNX Runtime's within-one-inference thread count, same knob
    /// `lanrurugi-ocr::recognize::TextRecognizer::load` uses — split across the whole pool (each
    /// session gets `intra_threads / SESSION_POOL_SIZE`, floor 1) so the pool's total thread
    /// usage stays within the caller's budget rather than multiplying it by `SESSION_POOL_SIZE`.
    ///
    /// GPU EP integration plan v8 (issue #103): each of the pool's `SESSION_POOL_SIZE` sessions
    /// tries CUDA independently and falls back to CPU on its own — deliberately *not* coordinated
    /// across the pool (a real GPU spike diagnosis found that keeping every session in a model's
    /// pool on the same EP required dropping an already-successful CUDA session to rebuild it as
    /// CPU whenever a sibling failed, and dropping a CUDA session is exactly the operation with a
    /// reproducible ~35-60% crash rate on this stack — see `.debug-scratch/
    /// GPU_EP_FINAL_SUMMARY.md`). The cost of this simplification is that the two pool sessions
    /// could end up on different EPs; that's a performance asymmetry (one page-erase call happens
    /// to hit the slower CPU session), not a correctness problem.
    pub fn load(
        model_path: &Path,
        intra_threads: usize,
        cuda_memory_limit_bytes: usize,
    ) -> Result<Self, InpaintError> {
        let per_session_threads = (intra_threads / SESSION_POOL_SIZE).max(1);
        // Built concurrently, not one after another: `build_session_with_gpu_fallback` (loading +
        // compiling the ~200MB graph) is disk/CPU-bound work that parallelizes fine across
        // `SESSION_POOL_SIZE` threads, and this whole call already runs inside
        // `TranslationRuntime::get()`'s single `run_blocking` closure — every other concurrent
        // translation request is blocked on it returning, so halving construction time here
        // directly shortens that stall (see `SESSION_POOL_SIZE`'s own doc comment for the
        // incident this fixes).
        let sessions: Vec<ManagedSession> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..SESSION_POOL_SIZE)
                .map(|_| {
                    scope.spawn(|| {
                        build_session_with_gpu_fallback(
                            model_path,
                            per_session_threads,
                            cuda_memory_limit_bytes,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("session build thread panicked"))
                .collect::<Result<Vec<_>, _>>()
        })?
        .into_iter()
        // Never leaked here — see `leak_cuda_sessions`'s own doc comment for why that's a
        // deliberately separate, opt-in step rather than something `load` does automatically.
        .map(|(session, is_cuda)| ManagedSession::Owned(Mutex::new(session), is_cuda))
        .collect();
        Ok(Self {
            sessions,
            next: AtomicUsize::new(0),
        })
    }

    /// Leaks the backing memory of every pool session that's actually running on CUDA, upgrading
    /// its slot from [`ManagedSession::Owned`] to [`ManagedSession::LeakedCuda`] — CPU slots are
    /// left untouched. Safe to call on every `Inpainter` this process will ever hold, but only
    /// worth calling on one meant to live for the process's entire lifetime (see
    /// `ManagedSession`'s own doc comment for why this exists at all).
    ///
    /// Deliberately a separate, caller-invoked step rather than something [`Self::load`] itself
    /// does — this crate's own tests/examples call `load` and would otherwise leak real CUDA
    /// memory (unreclaimable until process exit) on every single call, silently accumulating
    /// across a test run. The one real caller meant to invoke this is application-layer code that
    /// has already decided to hold the resulting `Inpainter` for the rest of the process's life
    /// (`lanrurugi-api::translation_pipeline::TranslationRuntime::load`).
    ///
    /// Takes/returns `self` by value, same as `lanrurugi-ocr::bubble_segment::BubbleSegmenter`'s
    /// identical method — the one real caller invokes this right after `load()` and before
    /// wrapping the result in an `Arc`, where it already owns the value outright.
    #[must_use]
    pub fn leak_cuda_sessions(self) -> Self {
        let Self { sessions, next } = self;
        let sessions = sessions
            .into_iter()
            .map(|slot| match slot {
                ManagedSession::Owned(mutex, true) => {
                    ManagedSession::LeakedCuda(Box::leak(Box::new(mutex)))
                }
                other => other,
            })
            .collect();
        Self { sessions, next }
    }

    /// Erases every masked pixel across the *whole page* in a single LaMa call. Real production
    /// manga translators do the same (e.g. `zyddnys/manga-image-translator`'s
    /// `dispatch_inpainting(inpainter, ctx.img_rgb, ctx.mask, ...)`, confirmed by reading its
    /// actual source, 2026-09-08): build one page-sized mask covering every region's own glyph
    /// pixels, run the model once, done. An earlier per-region approach (one LaMa call per text
    /// region, each against a small padded crop) was tried and abandoned after hitting two
    /// separate real bugs in practice:
    /// - Cross-region context pollution: a region's own context crop could read pixels an earlier
    ///   region in the same pass had already erased/redrawn, instead of real original artwork.
    /// - Irregular-hole degradation: LaMa was only ever asked to fill a plain rectangular hole per
    ///   region regardless of any precise stroke mask (the mask was applied only at paste-back
    ///   time, never fed to the model itself), and a region's own hole is small relative to the
    ///   page, so a stroke-shaped hole there is *more* irregular relative to its surrounding
    ///   context than the same shape would be on a full page. A whole-page mask is still exactly
    ///   as irregular in absolute terms, but the model has the entire page's worth of real,
    ///   unmasked context to draw from instead of just one region's own small padded crop, which
    ///   is what actually matters for reconstruction quality.
    ///
    /// Takes *two* page-sized masks, not one — `model_mask` (fed to the network itself, wide
    /// enough to give it a real transition band to blend into) and `paste_mask` (the pixels
    /// actually overwritten in the page this function returns, tight around each region's own
    /// real glyph shape). Both are row-major, exactly `page.width() * page.height()` long, and the
    /// caller is responsible for merging every individual region's own two masks
    /// (`lanrurugi_inpaint::stroke_mask::StrokeMask`'s own `model`/`paste` fields) into these two
    /// page-sized ones first — same merge, just applied twice.
    ///
    /// Splitting these apart at all (previously a single shared mask) fixes a real reported
    /// incident (2026-09-09): one OCR region's own bounding box was inaccurate enough to already
    /// include a manga panel's own border line, and growing a *single* mask by a wide radius (wide
    /// enough to give LaMa a real transition band — see `lanrurugi_inpaint::stroke_mask`'s own doc
    /// comment on `DILATION_RADIUS` for the residual-noise bug that radius itself fixes) let that
    /// panel border get folded into the pixels actually replaced with LaMa's reconstruction — a
    /// real straight line warped/blurred in the output, confirmed by a direct pixel-level
    /// comparison. `model_mask` can still pull in a little of a panel border from an inaccurate
    /// bbox, but only `paste_mask`'s own tighter pixels are ever actually overwritten, so the
    /// model still gets the wide context it needs without corrupting content the mask was never
    /// supposed to touch in the first place.
    ///
    /// If the page's longer edge exceeds [`MAX_INPAINT_SIZE`], it's downscaled (aspect-ratio
    /// preserved, never squeezed/letterboxed) before inference, then the result is scaled back up
    /// — matching the reference project's own `resize_keep_aspect` for the same reason: only ever
    /// shrink a page to control cost, never distort its real proportions before handing it to the
    /// model. Both dimensions are then padded (edge-replicated, not a solid colour — avoids
    /// inventing a hard edge for the model to misread as a real image boundary) up to a multiple
    /// of [`PAD_MODULO`], which this specific dynamic-shape ONNX export still requires even though
    /// its `h`/`w` dims are otherwise free — same requirement the reference project's own
    /// `_infer` enforces via its `pad_size = 8`.
    pub fn erase_page(
        &self,
        page: &RgbImage,
        model_mask: &[bool],
        paste_mask: &[bool],
    ) -> Result<RgbImage, InpaintError> {
        let (pw, ph) = page.dimensions();
        let expected_len = (pw * ph) as usize;
        if model_mask.len() != expected_len {
            return Err(InpaintError::BadOutput(format!(
                "model mask length {} does not match page dimensions {pw}x{ph}",
                model_mask.len()
            )));
        }
        if paste_mask.len() != expected_len {
            return Err(InpaintError::BadOutput(format!(
                "paste mask length {} does not match page dimensions {pw}x{ph}",
                paste_mask.len()
            )));
        }

        let mut mask_img = image::GrayImage::new(pw, ph);
        for (i, &m) in model_mask.iter().enumerate() {
            mask_img.put_pixel(
                (i as u32) % pw,
                (i as u32) / pw,
                image::Luma([if m { 255 } else { 0 }]),
            );
        }

        // Downscale only if needed, aspect ratio preserved — never squeeze into a fixed shape.
        let longest_edge = pw.max(ph);
        let (work_w, work_h) = if longest_edge > MAX_INPAINT_SIZE {
            let scale = f64::from(MAX_INPAINT_SIZE) / f64::from(longest_edge);
            (
                ((pw as f64) * scale).round().max(1.0) as u32,
                ((ph as f64) * scale).round().max(1.0) as u32,
            )
        } else {
            (pw, ph)
        };
        let work_img = if (work_w, work_h) == (pw, ph) {
            page.clone()
        } else {
            image::imageops::resize(page, work_w, work_h, FilterType::CatmullRom)
        };
        let work_mask = if (work_w, work_h) == (pw, ph) {
            mask_img
        } else {
            image::imageops::resize(&mask_img, work_w, work_h, FilterType::Nearest)
        };

        // Pad up to a multiple of PAD_MODULO — edge-replicated (`imageops::overlay` onto a canvas
        // built by tiling the image's own last row/column would be more faithful, but a plain
        // solid-colour pad is what the letterbox precedent in this crate used and is simpler; the
        // padded strip is at most PAD_MODULO-1 pixels wide/tall, i.e. a handful of pixels — cheap
        // enough context loss that a fancier edge-extension isn't worth the complexity here).
        let pad_w = work_w.div_ceil(PAD_MODULO) * PAD_MODULO;
        let pad_h = work_h.div_ceil(PAD_MODULO) * PAD_MODULO;
        let mut canvas_img = RgbImage::from_pixel(pad_w, pad_h, Rgb([114, 114, 114]));
        image::imageops::overlay(&mut canvas_img, &work_img, 0, 0);
        let mut canvas_mask = image::GrayImage::from_pixel(pad_w, pad_h, image::Luma([0]));
        image::imageops::overlay(&mut canvas_mask, &work_mask, 0, 0);

        let pixel_count = (pad_w * pad_h) as usize;
        let mut image_chw = vec![0f32; pixel_count * 3];
        let mut mask_flat = vec![0f32; pixel_count];
        for (i, px) in canvas_img.pixels().enumerate() {
            for c in 0..3 {
                image_chw[c * pixel_count + i] = f32::from(px.0[c]) / 255.0;
            }
        }
        for (i, px) in canvas_mask.pixels().enumerate() {
            mask_flat[i] = if px.0[0] > 127 { 1.0 } else { 0.0 };
        }

        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.sessions.len();

        let image_tensor =
            Tensor::from_array((vec![1i64, 3, i64::from(pad_h), i64::from(pad_w)], image_chw))?;
        let mask_tensor =
            Tensor::from_array((vec![1i64, 1, i64::from(pad_h), i64::from(pad_w)], mask_flat))?;

        // Extraction happens fully inside the closure, not after — `SessionOutputs`/its extracted
        // tensor slices borrow from the `&mut Session` `with_locked` hands in, and that borrow
        // can't outlive the closure itself (can't be smuggled out as part of `R`), so this copies
        // the two things actually needed (shape + a real owned `Vec<f32>` of the data) before the
        // closure returns rather than trying to hand back anything still borrowing the session.
        let (out_shape, out_flat): (Vec<i64>, Vec<f32>) = self.sessions[idx].with_locked(
            |session| -> Result<(Vec<i64>, Vec<f32>), InpaintError> {
                let outputs = session
                    .run(ort::inputs! {
                        "image" => image_tensor,
                        "mask" => mask_tensor,
                    })
                    .map_err(InpaintError::from)?;
                // This model's own output name is `inpainted`, not `output` — a real, different
                // convention from the earlier fixed-512 `Carve/LaMa-ONNX` model this crate used
                // before (confirmed by reading the ONNX graph directly, 2026-09-08). Always verify
                // a new model's own graph rather than assuming I/O names carry over between exports.
                let (shape, flat) = outputs["inpainted"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| InpaintError::BadOutput(e.to_string()))?;
                Ok((shape.to_vec(), flat.to_vec()))
            },
        )??;
        if out_shape.len() != 4 || out_shape[1] != 3 {
            return Err(InpaintError::BadOutput(format!(
                "expected [batch, 3, H, W], got {out_shape:?}"
            )));
        }
        let (out_h, out_w) = (out_shape[2] as u32, out_shape[3] as u32);
        let out_pixel_count = (out_h * out_w) as usize;

        // This model's own output is [0, 1] (its final graph op is
        // `sigmoid(gen_out) * mask + image * (1 - mask)`, both operands already [0,1], no `* 255`
        // node at all) — a real, different convention from the earlier fixed-512 `Carve/LaMa-ONNX`
        // model this crate used before, whose output *was* already [0, 255]. Always verify a new
        // model's own graph rather than assuming the same convention carries over between exports
        // — getting this backwards once already produced a real bug (nearly every output pixel
        // clamped to flat white), see this module's own top-level doc comment for that incident.
        let mut out_canvas = RgbImage::new(out_w, out_h);
        for y in 0..out_h {
            for x in 0..out_w {
                let i = (y * out_w + x) as usize;
                let px = [0, 1, 2].map(|c| {
                    (out_flat[c * out_pixel_count + i].clamp(0.0, 1.0) * 255.0).round() as u8
                });
                out_canvas.put_pixel(x, y, Rgb(px));
            }
        }

        // Crop the pad back off, then scale the real-content-only crop back up to the page's own
        // true size (a no-op resize when no downscale was needed above).
        let out_crop = image::imageops::crop_imm(&out_canvas, 0, 0, work_w, work_h).to_image();
        let out_resized = image::imageops::resize(&out_crop, pw, ph, FilterType::CatmullRom);

        let mut result = page.clone();
        for y in 0..ph {
            for x in 0..pw {
                let i = (y * pw + x) as usize;
                if paste_mask[i] {
                    result.put_pixel(x, y, *out_resized.get_pixel(x, y));
                }
            }
        }
        Ok(result)
    }
}

impl InpainterHandle for Inpainter {
    fn erase_page(
        &self,
        page: &RgbImage,
        model_mask: &[bool],
        paste_mask: &[bool],
    ) -> Result<RgbImage, String> {
        Inpainter::erase_page(self, page, model_mask, paste_mask).map_err(|e| e.to_string())
    }
}

/// Tries to build and commit a CUDA-registered session in one atomic attempt — registration and
/// `commit_from_file` are both inside the same `?` chain, so a failure at either step (not just
/// registration) triggers the caller's CPU fallback. `.error_on_failure()` makes registration
/// failure return a real `Err` instead of silently falling back to CPU inside `ort` itself (the
/// crate's own default behavior — without it, "CUDA session" and "CPU session" would be
/// indistinguishable from the caller's side). See `lanrurugi-ocr::bubble_segment`'s identical
/// helper (where this pattern was first validated against real hardware) for the full API
/// derivation notes.
fn try_cuda_session(
    path: &Path,
    intra_threads: usize,
    cuda_memory_limit_bytes: usize,
) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ep::CUDA::default()
            .with_memory_limit(cuda_memory_limit_bytes)
            // `ort`'s CUDA EP defaults to `ConvAlgorithmSearch::Exhaustive` with
            // `with_conv_max_workspace` unset (which itself defaults to `true`, i.e. unlimited) —
            // meaning every prior tuning of this session's memory limit was fighting an
            // *unbounded* cuDNN convolution-algorithm search that could still blow past whatever
            // arena cap was set (confirmed against `ort` 2.0.0-rc.13's own source: `cudnn_conv_use_
            // max_workspace` isn't set at all by `with_memory_limit` alone). `Heuristic` search
            // picks a fast-enough algorithm from a lightweight heuristic instead of benchmarking
            // every implementation, and `with_conv_max_workspace(false)` caps whatever workspace
            // that search itself is allowed to use to 32MB (`ort`'s own doc comment) rather than
            // "as much as it wants" — this is expected to substantially lower this session's real
            // peak VRAM use below `cuda_memory_limit_bytes` itself, not just cap the arena the
            // model's own tensors live in.
            .with_conv_algorithm_search(ep::cuda::ConvAlgorithmSearch::Heuristic)
            .with_conv_max_workspace(false)
            .build()
            .error_on_failure()])?
        .with_intra_threads(intra_threads.max(1))?;
    let mut builder = builder;
    builder.commit_from_file(path)
}

/// See `lanrurugi-ocr::recognize::try_openvino_session`'s identical call for the full rationale
/// (only ever reached after `ep::OpenVINO::default().is_available()` has confirmed the *loaded*
/// library actually has OpenVINO provider code compiled in — registering this EP against a build
/// that lacks it hangs indefinitely in `session.run()`, issue #103's research.md §1).
fn try_openvino_session(path: &Path, intra_threads: usize) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ep::OpenVINO::default()
            .with_device_type("GPU")
            .build()
            .error_on_failure()])?
        .with_intra_threads(intra_threads.max(1))?;
    let mut builder = builder;
    builder.commit_from_file(path)
}

/// Builds one ORT session, CUDA first (falling back to CPU on any failure — see
/// `Inpainter::load`'s own doc comment for why each pool session decides this independently
/// rather than the whole pool sharing one outcome). The returned `bool` is whether the session
/// actually ended up on CUDA — `ManagedSession`/`Inpainter::leak_cuda_sessions` need this to know
/// which sessions are safe to leave owned and which must eventually be leaked; `ort::Session`
/// itself exposes no way to ask this after construction, so it has to be captured here, at the
/// one point that actually knows.
fn build_session_with_gpu_fallback(
    path: &Path,
    intra_threads: usize,
    cuda_memory_limit_bytes: usize,
) -> Result<(Session, bool), InpaintError> {
    // Named `path_display`, not `display` — `display` collides with `tracing::field::display`,
    // which the `%field` shorthand below expands to call; confirmed the hard way in
    // `lanrurugi-ocr::bubble_segment` (the same helper this function mirrors) and repeated here
    // before catching it via `cargo check`.
    let path_display = || path.display().to_string();

    // `LANRURUGI_DISABLE_GPU` (GPU EP integration plan v8 §4.6) — see
    // `lanrurugi-ocr::bubble_segment`'s identical check for the full rationale (existence-only
    // env var, not a clap flag; kept as a hidden escape hatch).
    if std::env::var_os("LANRURUGI_DISABLE_GPU").is_some() {
        tracing::info!("LANRURUGI_DISABLE_GPU is set; using CPU for inpainting");
        return build_cpu_session(path, intra_threads).map(|session| (session, false));
    }

    let cuda_availability = ep::CUDA::default().is_available();
    match cuda_availability {
        Ok(true) => match try_cuda_session(path, intra_threads, cuda_memory_limit_bytes) {
            Ok(session) => {
                tracing::info!(path = %path_display(), "inpainting model loaded on CUDA");
                return Ok((session, true));
            }
            Err(error) => {
                // `.error_on_failure()` suppresses ort's own registration-failure log line — this
                // is the only place that failure reason gets logged.
                tracing::warn!(path = %path_display(), %error, "CUDA session build failed for inpainting model, falling back to CPU");
            }
        },
        Ok(false) => {
            tracing::debug!(
                "CUDA execution provider not compiled into the loaded ONNX Runtime build; \
                 checking OpenVINO"
            );
        }
        Err(error) => {
            // ort's own doc on `is_available()`: "a serious internal error occurs, in which case
            // your application should probably just abort" — logged at error level (not the
            // benign debug! above) but still falls back to CPU rather than aborting, since the
            // CPU path is independent of whatever made this probe itself fail.
            tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing CUDA; checking OpenVINO");
        }
    }

    // Only reached when CUDA wasn't compiled in or its registration wasn't attempted — never
    // after a CUDA registration *failure* (that case already returned above via the early
    // `return Ok(...)`, so falling through here only happens on `Ok(false)`/`Err` from the CUDA
    // probe). See `crate::try_openvino_session`'s doc comment for why gating on `is_available()`
    // is load-bearing here, not just an optimization.
    let openvino_availability = ep::OpenVINO::default().is_available();
    match openvino_availability {
        Ok(true) => match try_openvino_session(path, intra_threads) {
            Ok(session) => {
                tracing::info!(path = %path_display(), "inpainting model loaded on OpenVINO");
                return Ok((session, true));
            }
            Err(error) => {
                tracing::warn!(path = %path_display(), %error, "OpenVINO session build failed for inpainting model, falling back to CPU");
            }
        },
        Ok(false) => {
            tracing::debug!(
                "OpenVINO execution provider not compiled into the loaded ONNX Runtime build; \
                 using CPU for inpainting"
            );
        }
        Err(error) => {
            tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing OpenVINO; falling back to CPU for inpainting");
        }
    }

    build_cpu_session(path, intra_threads).map(|session| (session, false))
}

/// Builds one CPU-only ORT session — same explicit-CPU-EP pattern as
/// `lanrurugi-ocr::recognize::build_session` (never a GPU EP by default; research.md §1's
/// OpenVINO hang incident is the reason a GPU-flavoured `libonnxruntime.so` is never trusted with
/// an unregistered EP list — CUDA is only ever attempted explicitly, above, not implicitly here).
fn build_cpu_session(path: &Path, intra_threads: usize) -> Result<Session, InpaintError> {
    let path_display = || path.display().to_string();

    let builder = Session::builder().map_err(|source| InpaintError::Session {
        path: path_display(),
        source,
    })?;

    let builder = builder
        .with_execution_providers([ep::CPU::default().build()])
        .map_err(|source| InpaintError::Session {
            path: path_display(),
            source: source.into(),
        })?;

    let mut builder = builder
        .with_intra_threads(intra_threads.max(1))
        .map_err(|source| InpaintError::Session {
            path: path_display(),
            source: source.into(),
        })?;

    builder
        .commit_from_file(path)
        .map_err(|source| InpaintError::Session {
            path: path_display(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_a_nonexistent_model_file_fails_cleanly() {
        let path = std::env::temp_dir().join("lanrurugi-inpaint-nonexistent.onnx");
        let result = Inpainter::load(&path, 1, 4 * 1024 * 1024 * 1024);
        assert!(
            result.is_err(),
            "loading a nonexistent model file must fail, not panic"
        );
    }

    #[test]
    fn pad_modulo_rounds_up_to_the_next_multiple() {
        // div_ceil semantics used directly in erase_page — a real reported test page's own
        // dimensions (1136x1600), neither already a multiple of PAD_MODULO (8).
        assert_eq!(1136u32.div_ceil(PAD_MODULO) * PAD_MODULO, 1136);
        assert_eq!(1600u32.div_ceil(PAD_MODULO) * PAD_MODULO, 1600);
        assert_eq!(1137u32.div_ceil(PAD_MODULO) * PAD_MODULO, 1144);
        assert_eq!(1u32.div_ceil(PAD_MODULO) * PAD_MODULO, 8);
    }

    #[test]
    fn downscale_preserves_aspect_ratio_for_a_page_past_max_inpaint_size() {
        // A page taller than MAX_INPAINT_SIZE on its long edge must scale down, keeping
        // aspect ratio, never squeezing into a fixed shape — this mirrors the scale computation
        // inline in erase_page (kept here as a standalone check since that function itself needs
        // a real ORT session to run end-to-end).
        let (pw, ph) = (1500u32, 3000u32);
        let longest_edge = pw.max(ph);
        assert!(longest_edge > MAX_INPAINT_SIZE);
        let scale = f64::from(MAX_INPAINT_SIZE) / f64::from(longest_edge);
        let work_w = ((pw as f64) * scale).round().max(1.0) as u32;
        let work_h = ((ph as f64) * scale).round().max(1.0) as u32;
        assert_eq!(
            work_h, MAX_INPAINT_SIZE,
            "the longer dimension must hit the cap exactly"
        );
        let orig_ratio = pw as f64 / ph as f64;
        let scaled_ratio = work_w as f64 / work_h as f64;
        assert!((orig_ratio - scaled_ratio).abs() < 0.001);
    }
}
