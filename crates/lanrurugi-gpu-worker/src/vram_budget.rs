//! Dynamic, hardware-proportional VRAM budgets for each model's `with_memory_limit` — replaces
//! the fixed byte constants each crate used to hardcode (`lanrurugi-inpaint`/`lanrurugi-ocr`'s own
//! `CUDA_SESSION_MEMORY_LIMIT_BYTES`), which were tuned against one specific 8GB card and would
//! either starve a smaller card or waste VRAM a larger one didn't need to reserve at all (issue
//! #103's own history: 1GiB → 2GiB → 4GiB retuning for `Inpainter` alone, chasing one card's real
//! failures rather than scaling with whatever card is actually installed).
//!
//! Each budget is `percent_of_total` clamped to `[min_bytes, max_bytes]` — a fixed floor so a tiny
//! card doesn't get an unusably small arena, and a fixed ceiling so a huge card doesn't reserve far
//! more than a manga page's own peak activation-memory footprint could ever use (LaMa's FFC
//! convolutions at `MAX_INPAINT_SIZE` are the heaviest case among the three models this project
//! runs, and even that has a real, measured upper bound — see `lanrurugi-inpaint`'s own doc history
//! for the values that were found insufficient before landing on a fixed 4GiB).
//!
//! Total VRAM itself comes from `nvidia-smi --query-gpu=memory.total` (same shell-out-to-a-system-
//! tool discipline `~/jellyfin-suite/crates/frame-forge::gpu_compat` uses for its own hardware
//! queries, and the same tool `crate::main`'s `ModelGroup`-adjacent detection already assumes is
//! present in this image) rather than any `ort`/CUDA API — this has to be known *before* any ORT
//! session (let alone `with_memory_limit`, which takes the budget as an argument) is ever built.

use std::process::Command;

/// One worker-wide budget: `percent` of the card's total VRAM, clamped to `[min_bytes, max_bytes]`.
#[derive(Debug, Clone, Copy)]
pub struct VramBudget {
    pub percent: f64,
    pub min_bytes: usize,
    pub max_bytes: usize,
}

impl VramBudget {
    /// `total_vram_bytes` is `None` when `nvidia-smi` itself failed/wasn't found (no NVIDIA card,
    /// or a detection problem this process can't do anything about) — `min_bytes` is the safest
    /// assumption in that case, not `max_bytes`: better to under-ask and let `ort`'s own soft-limit
    /// arena grow if it turns out there was headroom, than to request more than a card that
    /// couldn't even be measured might not actually have.
    pub fn resolve(self, total_vram_bytes: Option<usize>) -> usize {
        match total_vram_bytes {
            Some(total) => {
                let scaled = (total as f64 * self.percent) as usize;
                scaled.clamp(self.min_bytes, self.max_bytes)
            }
            None => self.min_bytes,
        }
    }
}

/// Recognize worker's budget — `TextRecognizer` is the only model in that process, so this is its
/// whole worker-wide allocation, no further split needed.
///
/// `min_bytes` raised from an original 384MiB placeholder after a real 2026-09-14 incident: ORT's
/// CUDA BFCArena hit `Available memory of 0` allocation failures asking for a single ~277MiB
/// activation-tensor extension while the arena had already used only ~40MiB of its then-384MiB
/// limit — nowhere near a full physical-VRAM exhaustion (`nvidia-smi` showed <150MiB used by
/// anything else on the card at the time). Root cause: `arena_extend_strategy: kNextPowerOfTwo`
/// grows the arena in power-of-two-sized chunks, so a limit only modestly larger than one real
/// activation tensor leaves no room for the *next* power-of-two chunk to fit even though the
/// tensor itself would — fragmentation against the limit, not the card. 1GiB floor / 2GiB ceiling
/// leaves enough slack for that chunk growth pattern on manga-ocr's own ViT-shaped activations.
pub const RECOGNIZE_WORKER_BUDGET: VramBudget = VramBudget {
    percent: 0.15,
    min_bytes: 1024 * 1024 * 1024,
    max_bytes: 2 * 1024 * 1024 * 1024,
};

/// Image worker's *combined* budget for `BubbleSegmenter` + `Inpainter` together — the two models
/// split this one total rather than each getting an independent card-relative percentage, since
/// they're always loaded into (and reclaimed from) the same process as a unit — see
/// `lanrurugi_api::gpu_worker_client::WorkerKind`'s own doc comment for why.
///
/// `max_bytes` deliberately leaves real headroom below a card's own total rather than being able
/// to claim the whole thing: `WorkerKind::Recognize`'s 5-minute idle timeout (vs. `Image`'s 30
/// seconds) means the `Recognize` worker routinely still holds its own VRAM arena live when a
/// translation request's `Image`-worker call runs — the two are two separate long-lived processes,
/// not something a request-level call-ordering fix can serialize away. A real 2026-09-14 incident
/// on an 8GiB card: `max_bytes` at 8GiB (this budget could claim the entire card) reliably produced
/// `Available memory of 0` CUDA allocation failures and garbled OCR output on any page that needed
/// both workers close together. 5GiB leaves 1GiB of headroom below `RECOGNIZE_WORKER_BUDGET`'s own
/// 2GiB ceiling even on an 8GiB card.
///
/// `percent` raised from an original 0.4 the same day: with `min_bytes`/`max_bytes` at 3GiB/5GiB,
/// 0.4 of an 8151MiB card resolves to only ~3.2GiB — barely above the floor and nowhere near
/// `max_bytes`, so the ceiling this doc comment describes was never actually reached. Real logs
/// from that incident showed the *resolved* value wasn't the problem in the end (LaMa's own
/// `ConvTranspose` decoder stage genuinely needs more than a ~2.4GiB inpainter share could supply,
/// confirmed against `Available memory of 467035648 is smaller than requested bytes of 699183872`
/// — 445MiB free against a single 667MiB activation request, with `nvidia-smi` showing >4GiB
/// actually free on the card at the same moment) — 0.6 pushes the resolved value close enough to
/// `max_bytes` on an 8GiB card to give the inpainter's own share real headroom above that peak.
pub const IMAGE_WORKER_BUDGET: VramBudget = VramBudget {
    percent: 0.6,
    min_bytes: 3 * 1024 * 1024 * 1024,
    max_bytes: 5 * 1024 * 1024 * 1024,
};

/// How `IMAGE_WORKER_BUDGET`'s resolved total is split between the two models it covers — LaMa's
/// FFC convolutions need far more headroom than the bubble segmenter's fixed 1600x1600 YOLO input
/// (same relative weighting this project's old fixed constants used: 768MiB vs 4GiB, roughly
/// 1:5.3), kept as a ratio rather than each getting its own independent `VramBudget` so the pair
/// always sums to exactly what `IMAGE_WORKER_BUDGET` resolved to, regardless of card size.
const BUBBLE_SEGMENTER_SHARE: f64 = 768.0 / (768.0 + 4096.0);

/// Floor under the bubble segmenter's own share of `split_image_worker_budget`'s split, applied
/// *before* the ratio would otherwise shrink it further — a real 2026-09-14 incident found the
/// plain-ratio split alone giving the bubble segmenter as little as ~515MiB on an 8GiB card's own
/// `IMAGE_WORKER_BUDGET` resolution, and CUDA's BFCArena hit `Available memory of 0` extending that
/// arena by a single ~277MiB activation tensor despite the arena having used only ~40MiB of its own
/// 515MiB limit at the time — `kNextPowerOfTwo` arena growth needs real headroom above one
/// resident-at-once activation tensor's own peak size, not just "the ratio's share of whatever the
/// worker-wide total happened to resolve to". 768MiB is this project's own previously-hardcoded
/// single-card bubble-segmenter figure (see this file's own `RECOGNIZE_WORKER_BUDGET` doc comment
/// for the same floor-restoration reasoning applied there).
const BUBBLE_SEGMENTER_MIN_BYTES: usize = 768 * 1024 * 1024;

/// Splits an already-resolved Image-worker total between `BubbleSegmenter` and `Inpainter`,
/// returning `(bubble_segmenter_bytes, inpainter_bytes)`. The ratio share is clamped up to
/// `BUBBLE_SEGMENTER_MIN_BYTES` first — see that constant's own doc comment — so `Inpainter` never
/// gets *less* than `total_bytes - BUBBLE_SEGMENTER_MIN_BYTES` even on a total small enough that the
/// plain ratio would have starved the bubble segmenter.
pub fn split_image_worker_budget(total_bytes: usize) -> (usize, usize) {
    let ratio_bubble = (total_bytes as f64 * BUBBLE_SEGMENTER_SHARE) as usize;
    let bubble = ratio_bubble
        .max(BUBBLE_SEGMENTER_MIN_BYTES)
        .min(total_bytes);
    let inpaint = total_bytes.saturating_sub(bubble);
    (bubble, inpaint)
}

/// Total VRAM on GPU index 0, in bytes — `None` if `nvidia-smi` is missing, fails, or its output
/// isn't a parseable integer (e.g. no NVIDIA card present at all, the expected case for an Intel-
/// only or CPU-only host). Only ever consulted for NVIDIA's own budget calculation — an Intel host
/// (`WorkerKind`'s `GpuVendor::Intel` path) has no equivalent "how much VRAM does this card have"
/// query wired up yet, so its sessions still get whatever `min_bytes` floor applies; revisit if/
/// when this project gets real Intel GPU hardware to measure against (see `gpu_worker_client`'s own
/// module doc on why that path hasn't been hardware-verified at all yet).
pub fn detect_total_vram_bytes() -> Option<usize> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.total",
            "--format=csv,noheader,nounits",
            "-i",
            "0",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mib: usize = text.trim().parse().ok()?;
    Some(mib * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_scales_with_total_within_bounds() {
        let budget = VramBudget {
            percent: 0.5,
            min_bytes: 100,
            max_bytes: 1000,
        };
        assert_eq!(budget.resolve(Some(400)), 200);
    }

    #[test]
    fn resolve_clamps_to_min_when_percent_scales_below_it() {
        let budget = VramBudget {
            percent: 0.05,
            min_bytes: 384 * 1024 * 1024,
            max_bytes: 1024 * 1024 * 1024,
        };
        // A tiny/undetected card scaled at 5% would fall well under the 384MiB floor.
        assert_eq!(budget.resolve(Some(1024 * 1024 * 1024)), 384 * 1024 * 1024);
    }

    #[test]
    fn resolve_clamps_to_max_on_a_huge_card() {
        let budget = VramBudget {
            percent: 0.55,
            min_bytes: 3 * 1024 * 1024 * 1024,
            max_bytes: 8 * 1024 * 1024 * 1024,
        };
        // 55% of a 48GiB card would be ~26.4GiB, far past the 8GiB ceiling.
        assert_eq!(
            budget.resolve(Some(48 * 1024 * 1024 * 1024)),
            8 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn resolve_falls_back_to_min_when_total_is_unknown() {
        let budget = VramBudget {
            percent: 0.05,
            min_bytes: 384 * 1024 * 1024,
            max_bytes: 1024 * 1024 * 1024,
        };
        assert_eq!(budget.resolve(None), 384 * 1024 * 1024);
    }

    #[test]
    fn split_image_worker_budget_sums_to_the_total() {
        let (bubble, inpaint) = split_image_worker_budget(4_864_000_000);
        assert_eq!(bubble + inpaint, 4_864_000_000);
        // Bubble segmenter's share should be the smaller half (roughly 1:5.3 against inpainting).
        assert!(bubble < inpaint);
    }
}
