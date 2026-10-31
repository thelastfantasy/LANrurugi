//! Page-sequence alignment between two versions of the same work (e.g. a print scan vs. a digital
//! release with a different page count) — perceptual hashing (per issue #77's own research
//! comments: pHash/dHash are the standard, lightweight approach for this class of "same content,
//! different scan" matching) plus a banded dynamic-time-warping-style alignment, rather than
//! either assuming pages line up 1:1 by index or doing a full O(n*m) all-pairs comparison.
//!
//! The banded restriction (not full DP) relies on the same real-world assumption issue #77's own
//! design comments make explicit: two versions of the same work are page-order-preserving with
//! only local insertions/deletions (an extra title page, a missing blank page), not wholesale
//! reordering — so the optimal alignment path never strays far from the diagonal, and searching
//! only a narrow band around it is both correct for this problem shape and far cheaper than a full
//! matrix.

use image::DynamicImage;
use image_hasher::{HasherConfig, ImageHash};

/// Hamming-distance threshold (out of the hasher's own 64-bit hash length) below which two pages
/// are considered "the same content" — chosen conservatively (roughly 10% of bits) since a false
/// "same page" match would silently pair up two genuinely different pages, which is worse for this
/// feature's own purpose (quality comparison between truly corresponding pages) than occasionally
/// failing to align a real match and falling back to the caller's own "ask the user" path.
const MAX_HAMMING_DISTANCE: u32 = 6;

/// Cost credited to a page pair [`align_sequences_with_rescue`]'s own `rescue` callback confirmed,
/// after the primary Hamming check rejected it — deliberately equal to (not below)
/// `MAX_HAMMING_DISTANCE` itself: the DP must still prefer a REAL Hamming match over a rescued one
/// whenever both are available (a rescue is corroborating evidence from a different, coarser
/// signal — see `crop_align`'s own docs — not a stronger one than a clean hash match), while still
/// beating the skip penalty (`MAX_HAMMING_DISTANCE + 1`), or the rescue would never actually change
/// which path the DP picks.
const RESCUED_MATCH_DISTANCE: u32 = MAX_HAMMING_DISTANCE;

/// Minimum `crop_align::estimate_crop_alignment_with_confidence` correlation score a caller should
/// require before treating a rescue as confirmed — deliberately higher than that module's own
/// `MIN_CONFIDENCE` (0.5, its "don't guess a transform at all" floor for the magnifier's own
/// unrelated use). The rescue path's own accepted bar, per real measurement: a page with an added
/// white border plus scanner-realistic speckle noise (i.e. this feature's own real motivating
/// case — the ~95%-similar-core-content-occupying-≥85%-of-frame scenario confirmed against
/// real extracted pages during development) scored 0.967. Set below that but well above the
/// generic floor, so genuinely weak/coincidental correlations (which `MIN_CONFIDENCE` alone would
/// still accept) don't get promoted into an alignment match this DP will trust as strongly as a
/// real Hamming hit.
pub const RESCUE_CONFIDENCE_THRESHOLD: f64 = 0.9;

/// The distance a confirmed rescue should report back through the `rescue` callback — callers
/// outside this module (e.g. `lib.rs`'s own `compare_archives`) build their rescue closures around
/// this rather than a copy of the value, so [`RESCUED_MATCH_DISTANCE`]'s own doc comment (why it
/// equals, not undercuts, [`MAX_HAMMING_DISTANCE`]) stays the single source of truth.
pub fn rescued_match_distance() -> u32 {
    RESCUED_MATCH_DISTANCE
}

/// How far the alignment path is allowed to stray from the diagonal — generous enough to absorb a
/// handful of inserted/removed pages (title pages, blank pages, differing front-matter) without
/// blowing up the search space; two versions differing by more than this many pages' worth of
/// drift are unusual enough that a full match likely isn't the right model anyway.
const BAND_WIDTH: i64 = 20;

pub fn hash_page(image: &DynamicImage) -> ImageHash {
    HasherConfig::new().to_hasher().hash_image(image)
}

/// One matched pair — indices into the two original page-hash slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedPair {
    pub a_index: usize,
    pub b_index: usize,
}

/// Aligns two page-hash sequences, returning the pairs judged to be the same underlying content —
/// see [`align_sequences_with_rescue`] for the full algorithm; this is that function with no
/// rescue callback (`|_, _| None`), i.e. identical to this module's original, hash-only behavior.
pub fn align_sequences(a: &[ImageHash], b: &[ImageHash]) -> Vec<AlignedPair> {
    align_sequences_with_rescue(a, b, |_, _| None)
}

/// Like [`align_sequences`], but for a page whose entire in-band row has no
/// [`MAX_HAMMING_DISTANCE`] candidate at all (perceptual hashing dead ends — e.g. one side has a
/// systematic crop-margin/scan-resolution difference the hash itself is sensitive to, see
/// `crop_align`'s own docs), calls `rescue(a_index, b_index)` (0-based) against a single
/// best-guess B index for that row, and — if it returns `Some(distance)` — treats that as if
/// perceptual hashing itself had reported `distance.min(RESCUED_MATCH_DISTANCE)`. `rescue` is
/// called AT MOST ONCE per orphaned row (not once per candidate in its band): even `crop_align`'s
/// own tuned ~38ms/call is still too expensive to run across every one of a band's
/// `2*BAND_WIDTH+1` cells for every row (minutes per comparison for a real ~200-page book) — this
/// keeps the worst-case rescue-call budget at O(n).
///
/// The rescue target for an orphaned row comes from a rolling A->B index offset, updated after
/// every row (real-Hamming or successfully-rescued) that DOES find an in-band match —
/// causal/forward-only (never looking ahead at rows not yet processed), so it naturally tracks a
/// drifting offset (e.g. a run of inserted pages partway through one side) instead of assuming a
/// single fixed offset for the whole book. Falls back to offset 0 (`b_index == a_index`) before
/// any row has established a trend yet (e.g. the very first page).
///
/// Banded DP: `cost[i][j]` is the minimum total distance to align the first `i` pages of `a`
/// against the first `j` pages of `b`, restricted to `|i - j| <= BAND_WIDTH`. Three transitions
/// per cell: match `a[i-1]` with `b[j-1]` (cost = their Hamming distance, or a rescued distance if
/// the raw hash distance exceeds the threshold but `rescue` confirmed it — either way only allowed
/// when the resulting cost is within `MAX_HAMMING_DISTANCE`, else treated as unavailable, forcing
/// a skip instead of a bad match), or skip a page in either sequence (cost = a fixed skip penalty,
/// so the algorithm prefers real matches over skipping whenever one exists within threshold).
pub fn align_sequences_with_rescue(
    a: &[ImageHash],
    b: &[ImageHash],
    mut rescue: impl FnMut(usize, usize) -> Option<u32>,
) -> Vec<AlignedPair> {
    let n = a.len();
    let m = b.len();
    if n == 0 || m == 0 {
        return Vec::new();
    }

    let band_index = |i: i64, j: i64| -> Option<usize> {
        let offset = j - i + BAND_WIDTH;
        if (0..2 * BAND_WIDTH + 1).contains(&offset) {
            Some(offset as usize)
        } else {
            None
        }
    };

    // Pass 1: per-row local Hamming scan + rolling offset + at most one rescue attempt per
    // orphaned row. Produces `rescued`, consulted by Pass 2's DP below as if it were a real
    // (capped) Hamming distance for that specific (a_index, b_index) pair. Deliberately a plain
    // local scan, not routed through the DP's own cost accumulation below — a row's "does any
    // in-band candidate pass Hamming" is independent of the DP's later global path optimization,
    // so this doesn't need (and can't cheaply get) confirmed-match state from the DP itself, which
    // only exists after backtracking the fully-filled table.
    let mut rescued: std::collections::HashMap<(usize, usize), u32> =
        std::collections::HashMap::new();
    let mut rolling_offset: i64 = 0;
    let mut have_trend = false;
    for (i, a_hash) in a.iter().enumerate() {
        let mut best: Option<(usize, u32)> = None;
        for (j, b_hash) in b.iter().enumerate() {
            if band_index(i as i64 + 1, j as i64 + 1).is_none() {
                continue;
            }
            let distance = a_hash.dist(b_hash);
            if distance <= MAX_HAMMING_DISTANCE && best.is_none_or(|(_, d)| distance < d) {
                best = Some((j, distance));
            }
        }
        if let Some((j, _)) = best {
            rolling_offset = j as i64 - i as i64;
            have_trend = true;
            continue;
        }
        let target_j = i as i64 + if have_trend { rolling_offset } else { 0 };
        if target_j < 0 || target_j >= m as i64 {
            continue;
        }
        let target_j = target_j as usize;
        if band_index(i as i64 + 1, target_j as i64 + 1).is_none() {
            continue; // the DP's own band wouldn't reach this cell regardless — not worth trying.
        }
        if let Some(distance) = rescue(i, target_j) {
            let distance = distance.min(RESCUED_MATCH_DISTANCE);
            rescued.insert((i, target_j), distance);
            rolling_offset = target_j as i64 - i as i64;
            have_trend = true;
        }
    }

    // Pass 2: the banded DP itself — identical shape to this module's original single-pass
    // version, except the diagonal transition also allows a `rescued` pair through at its own
    // (capped) distance when the raw Hamming distance alone would have rejected it.
    let skip_penalty = MAX_HAMMING_DISTANCE + 1;
    let band_size = (2 * BAND_WIDTH + 1) as usize;
    const UNREACHABLE: u32 = u32::MAX;
    let mut cost = vec![vec![UNREACHABLE; band_size]; n + 1];
    // 0 = diagonal (match), 1 = skip in a (move down), 2 = skip in b (move right).
    let mut from = vec![vec![0u8; band_size]; n + 1];

    if let Some(idx) = band_index(0, 0) {
        cost[0][idx] = 0;
    }
    for i in 0..=n {
        for j in 0..=m {
            if i == 0 && j == 0 {
                continue;
            }
            let Some(idx) = band_index(i as i64, j as i64) else {
                continue;
            };
            let mut best = UNREACHABLE;
            let mut best_from = 0u8;

            if i > 0 && j > 0 {
                if let Some(prev_idx) = band_index(i as i64 - 1, j as i64 - 1) {
                    let prev = cost[i - 1][prev_idx];
                    if prev != UNREACHABLE {
                        let raw_distance = a[i - 1].dist(&b[j - 1]);
                        let distance = if raw_distance <= MAX_HAMMING_DISTANCE {
                            Some(raw_distance)
                        } else {
                            rescued.get(&(i - 1, j - 1)).copied()
                        };
                        if let Some(distance) = distance {
                            let candidate = prev + distance;
                            if candidate < best {
                                best = candidate;
                                best_from = 0;
                            }
                        }
                    }
                }
            }
            if i > 0 {
                if let Some(prev_idx) = band_index(i as i64 - 1, j as i64) {
                    let prev = cost[i - 1][prev_idx];
                    if prev != UNREACHABLE {
                        let candidate = prev + skip_penalty;
                        if candidate < best {
                            best = candidate;
                            best_from = 1;
                        }
                    }
                }
            }
            if j > 0 {
                if let Some(prev_idx) = band_index(i as i64, j as i64 - 1) {
                    let prev = cost[i][prev_idx];
                    if prev != UNREACHABLE {
                        let candidate = prev + skip_penalty;
                        if candidate < best {
                            best = candidate;
                            best_from = 2;
                        }
                    }
                }
            }

            cost[i][idx] = best;
            from[i][idx] = best_from;
        }
    }

    // Backtrack from whichever reachable (n, j) cell has the lowest cost. Ties are common — e.g.
    // "skip a's page then skip b's page" (2 * skip_penalty) costs exactly the same as "match one
    // real pair elsewhere and skip both members of another" — and since a real match is strictly
    // more useful to this algorithm's own purpose than an equal-cost path that skips its way past
    // a matchable pair, ties are broken by preferring the candidate whose backtrack yields more
    // matched pairs (confirmed necessary live: a 3-page/3-page fixture with a real match at both
    // ends and an unrelated middle page tied end-of-row costs at j=1 — stopping right after the
    // first match, both real pages skipped — against j=3 — both real matches kept, the middle
    // pair mutually skipped — and picked the FIRST-visited (smaller j, fewer matches) cell purely
    // because `<` never re-selects on a tie).
    let backtrack_from = |mut i: usize, mut j: usize| -> Vec<AlignedPair> {
        let mut pairs = Vec::new();
        while i > 0 || j > 0 {
            let Some(idx) = band_index(i as i64, j as i64) else {
                break;
            };
            match from[i][idx] {
                0 if i > 0 && j > 0 => {
                    pairs.push(AlignedPair {
                        a_index: i - 1,
                        b_index: j - 1,
                    });
                    i -= 1;
                    j -= 1;
                }
                1 if i > 0 => i -= 1,
                2 if j > 0 => j -= 1,
                _ => break,
            }
        }
        pairs.reverse();
        pairs
    };

    let mut best: Option<(usize, u32, usize)> = None; // (j, cost, match_count)
    for j in 0..=m {
        let Some(idx) = band_index(n as i64, j as i64) else {
            continue;
        };
        let c = cost[n][idx];
        if c == UNREACHABLE {
            continue;
        }
        let is_new_low = best.is_none_or(|(_, best_c, _)| c < best_c);
        let is_tie = best.is_some_and(|(_, best_c, _)| c == best_c);
        if !is_new_low && !is_tie {
            continue;
        }
        let count = backtrack_from(n, j).len();
        let better = is_new_low || best.is_some_and(|(_, _, best_count)| count > best_count);
        if better {
            best = Some((j, c, count));
        }
    }
    let Some((j, _, _)) = best else {
        return Vec::new();
    };
    backtrack_from(n, j)
}

/// Indices into `a` (0-based) that `pairs` never matched to anything in `b` — pages unique to `a`,
/// in their original order. Deliberately a plain post-hoc set difference over [`align_sequences`]'s
/// own output rather than a change to the DP itself: every existing caller of `align_sequences`
/// only ever wanted the matched pairs, and computing this separately keeps that function's own
/// contract unchanged (issue #77's own follow-on design — "多出来的页面" — needs this for the
/// "keep some of A's unique pages even when picking B overall" patch-export flow).
pub fn unmatched_a_indices(a_len: usize, pairs: &[AlignedPair]) -> Vec<usize> {
    let matched: std::collections::HashSet<usize> = pairs.iter().map(|p| p.a_index).collect();
    (0..a_len).filter(|i| !matched.contains(i)).collect()
}

/// The `b`-side mirror of [`unmatched_a_indices`] — pages unique to `b`, needed for the symmetric
/// "keep some of B's unique pages even when picking A overall" case (issue #77's own follow-on
/// design confirmed this needs to work both directions, not just A-unique-onto-B).
pub fn unmatched_b_indices(b_len: usize, pairs: &[AlignedPair]) -> Vec<usize> {
    let matched: std::collections::HashSet<usize> = pairs.iter().map(|p| p.b_index).collect();
    (0..b_len).filter(|i| !matched.contains(i)).collect()
}

/// For each unmatched A index, the B index it should default to being inserted after — the
/// nearest matched pair *before* it in `a`'s own sequence, mapped through to that pair's own B
/// index. `None` means "insert at the very start" (no matched pair precedes it at all — e.g. a
/// bonus page before the rest of the content even begins). This needs no AI/LLM judgment: the
/// alignment DP's own output already encodes exactly this adjacency (confirmed design — "这本质
/// 是个确定性的区间查找问题...不需要 AI"), it's just never been read back out in this shape
/// before. The frontend's drag-and-drop UI (issue #77's own follow-on design) uses this as each
/// unmatched page's starting position, which a user can still drag elsewhere afterward — this
/// is a default, not a constraint.
pub fn default_insert_after_b_index(a_index: usize, pairs: &[AlignedPair]) -> Option<usize> {
    pairs
        .iter()
        .filter(|p| p.a_index < a_index)
        .max_by_key(|p| p.a_index)
        .map(|p| p.b_index)
}

/// The `b`-side mirror of [`default_insert_after_b_index`] — for an unmatched B index, the
/// nearest preceding matched pair's own A index to default to inserting after.
pub fn default_insert_after_a_index(b_index: usize, pairs: &[AlignedPair]) -> Option<usize> {
    pairs
        .iter()
        .filter(|p| p.b_index < b_index)
        .max_by_key(|p| p.b_index)
        .map(|p| p.a_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// A perceptual hash of a perfectly UNIFORM color block has zero internal gradient/frequency
    /// content to hash — confirmed live: every solid-color test image originally used here hashed
    /// to the same (or a near-identical) value regardless of color, since `image_hasher`'s default
    /// algorithm reads structure, not raw color. Real manga pages always have real internal
    /// structure (line art, screentone, text), so this generates a small deterministic noise
    /// pattern per `seed` instead — distinct seeds produce genuinely distinguishable hashes, which
    /// is what these tests actually need to exercise the alignment logic meaningfully.
    fn noise_page(seed: u32) -> DynamicImage {
        let mut state = seed.wrapping_mul(2654435761).wrapping_add(1);
        let mut next = move || {
            // xorshift32 — good enough for deterministic test fixture noise, not cryptographic.
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(32, 32, |_, _| {
            let v = next();
            Rgb([
                (v & 0xff) as u8,
                ((v >> 8) & 0xff) as u8,
                ((v >> 16) & 0xff) as u8,
            ])
        });
        DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn align_sequences_matches_identical_pages_at_identical_positions() {
        let a_pages = [noise_page(1), noise_page(2), noise_page(3)];
        let b_pages = a_pages.clone();
        let a_hashes: Vec<_> = a_pages.iter().map(hash_page).collect();
        let b_hashes: Vec<_> = b_pages.iter().map(hash_page).collect();

        let pairs = align_sequences(&a_hashes, &b_hashes);
        assert_eq!(pairs.len(), 3);
        for (k, pair) in pairs.iter().enumerate() {
            assert_eq!(pair.a_index, k);
            assert_eq!(pair.b_index, k);
        }
    }

    #[test]
    fn align_sequences_handles_an_inserted_page_in_the_middle() {
        // b has an extra page (a title/insert page) between what were originally adjacent pages
        // in a — real-world equivalent of "digital release has a bonus page the print scan
        // doesn't".
        let page1 = noise_page(1);
        let page2 = noise_page(2);
        let page3 = noise_page(3);
        let inserted = noise_page(4);

        let a_hashes: Vec<_> = [&page1, &page2, &page3]
            .iter()
            .map(|p| hash_page(p))
            .collect();
        let b_hashes: Vec<_> = [&page1, &inserted, &page2, &page3]
            .iter()
            .map(|p| hash_page(p))
            .collect();

        let pairs = align_sequences(&a_hashes, &b_hashes);
        // page1 <-> page1 (0,0), page2 <-> page2 (1,2), page3 <-> page3 (2,3) — the inserted page
        // at b[1] has no counterpart in a and must be skipped, not force-matched to anything.
        assert_eq!(pairs.len(), 3);
        assert_eq!(
            pairs[0],
            AlignedPair {
                a_index: 0,
                b_index: 0
            }
        );
        assert_eq!(
            pairs[1],
            AlignedPair {
                a_index: 1,
                b_index: 2
            }
        );
        assert_eq!(
            pairs[2],
            AlignedPair {
                a_index: 2,
                b_index: 3
            }
        );
    }

    #[test]
    fn align_sequences_never_pairs_two_genuinely_different_pages() {
        let a_hashes: Vec<_> = [noise_page(1)].iter().map(hash_page).collect();
        let b_hashes: Vec<_> = [noise_page(99)].iter().map(hash_page).collect();

        // Two unrelated noise patterns are far apart in Hamming distance for a real perceptual
        // hash — must NOT be forced into a match just because they're both the only candidate.
        let pairs = align_sequences(&a_hashes, &b_hashes);
        assert!(pairs.is_empty(), "got {pairs:?}");
    }

    #[test]
    fn align_sequences_returns_empty_for_either_empty_input() {
        assert!(align_sequences(&[], &[hash_page(&noise_page(1))]).is_empty());
        assert!(align_sequences(&[hash_page(&noise_page(1))], &[]).is_empty());
    }

    #[test]
    fn align_sequences_with_rescue_recovers_a_pair_the_hash_alone_rejects() {
        // page2's B-side counterpart is deliberately a DIFFERENT noise seed (simulating a
        // systematic border/frame difference perceptual hashing can't see past) — plain
        // `align_sequences` must fail to pair it, since noise_page seeds have no real pHash
        // similarity to each other. The rescue closure recognizes the specific (a_index, b_index)
        // pair anyway (standing in for `crop_align` confirming it via correlation).
        let page1 = noise_page(1);
        let page2 = noise_page(2);
        let page3 = noise_page(3);
        let page2_bordered = noise_page(20); // stands in for "page2 with an added white border"

        let a_hashes: Vec<_> = [&page1, &page2, &page3]
            .iter()
            .map(|p| hash_page(p))
            .collect();
        let b_hashes: Vec<_> = [&page1, &page2_bordered, &page3]
            .iter()
            .map(|p| hash_page(p))
            .collect();

        // Sanity check: without rescue, the middle page is dropped, not force-matched.
        let plain = align_sequences(&a_hashes, &b_hashes);
        assert_eq!(plain.len(), 2, "got {plain:?}");
        assert!(!plain.iter().any(|p| p.a_index == 1));

        let mut rescue_calls = Vec::new();
        let rescued = align_sequences_with_rescue(&a_hashes, &b_hashes, |a_index, b_index| {
            rescue_calls.push((a_index, b_index));
            if (a_index, b_index) == (1, 1) {
                Some(RESCUED_MATCH_DISTANCE)
            } else {
                None
            }
        });

        assert_eq!(rescued.len(), 3, "got {rescued:?}");
        assert_eq!(
            rescued[1],
            AlignedPair {
                a_index: 1,
                b_index: 1
            }
        );
        // Exactly one rescue attempt (the orphaned row) — not one per band candidate.
        assert_eq!(rescue_calls, vec![(1, 1)]);
    }

    #[test]
    fn align_sequences_with_rescue_updates_the_rolling_offset_after_a_confirmed_rescue() {
        // b has an extra leading page (b[0]) with no counterpart in a at all, so a[0] really
        // matches b[1] — a real (non-rescued) Hamming hit, since both are the same noise seed.
        // This establishes a rolling offset of +1 (b_index == a_index + 1) before row 1 is ever
        // reached. a[1] then has NO real match anywhere in b (rejects the naive same-index
        // default target_j == 1) and can only be rescued at the ROLLED-FORWARD target
        // b_index == 1 + 1 == 2 — proving the offset established at row 0 actually carried
        // forward, not just "some target was tried."
        let b_extra_lead = noise_page(50);
        let page0 = noise_page(1);
        let page1 = noise_page(2);
        let page1_alt = noise_page(21); // page1's B counterpart: unrelated hash, needs rescue.

        let a_hashes: Vec<_> = [&page0, &page1].iter().map(|p| hash_page(p)).collect();
        let b_hashes: Vec<_> = [&b_extra_lead, &page0, &page1_alt]
            .iter()
            .map(|p| hash_page(p))
            .collect();

        let mut rescue_calls = Vec::new();
        let rescued = align_sequences_with_rescue(&a_hashes, &b_hashes, |a_index, b_index| {
            rescue_calls.push((a_index, b_index));
            match (a_index, b_index) {
                (1, 2) => Some(RESCUED_MATCH_DISTANCE),
                _ => None,
            }
        });

        // Row 0 needs no rescue at all — a real Hamming match already pairs it with b[1].
        assert!(!rescue_calls.contains(&(0, 0)) && !rescue_calls.contains(&(0, 1)));
        // Row 1's ONLY rescue attempt must be at the rolled-forward target, not the naive
        // same-index default (which would be (1, 1)).
        assert_eq!(rescue_calls, vec![(1, 2)]);
        assert_eq!(
            rescued,
            vec![
                AlignedPair {
                    a_index: 0,
                    b_index: 1
                },
                AlignedPair {
                    a_index: 1,
                    b_index: 2
                },
            ]
        );
    }

    #[test]
    fn unmatched_a_indices_returns_pages_a_has_no_counterpart_for() {
        let pairs = vec![
            AlignedPair {
                a_index: 0,
                b_index: 0,
            },
            AlignedPair {
                a_index: 2,
                b_index: 1,
            },
        ];
        assert_eq!(unmatched_a_indices(4, &pairs), vec![1, 3]);
    }

    #[test]
    fn unmatched_a_indices_is_empty_when_every_page_matched() {
        let pairs = vec![
            AlignedPair {
                a_index: 0,
                b_index: 0,
            },
            AlignedPair {
                a_index: 1,
                b_index: 1,
            },
        ];
        assert!(unmatched_a_indices(2, &pairs).is_empty());
    }

    #[test]
    fn unmatched_b_indices_returns_pages_b_has_no_counterpart_for() {
        let pairs = vec![
            AlignedPair {
                a_index: 0,
                b_index: 1,
            },
            AlignedPair {
                a_index: 1,
                b_index: 3,
            },
        ];
        assert_eq!(unmatched_b_indices(4, &pairs), vec![0, 2]);
    }

    #[test]
    fn default_insert_after_b_index_uses_the_nearest_preceding_matched_pair() {
        let pairs = vec![
            AlignedPair {
                a_index: 0,
                b_index: 0,
            },
            AlignedPair {
                a_index: 3,
                b_index: 2,
            },
        ];
        // Unmatched a=1 and a=2 both sit between the same two matched pairs (a=0 and a=3) — both
        // should default to right after the nearer-preceding one (a=0 -> b=0), not the farther one.
        assert_eq!(default_insert_after_b_index(1, &pairs), Some(0));
        assert_eq!(default_insert_after_b_index(2, &pairs), Some(0));
    }

    #[test]
    fn default_insert_after_b_index_is_none_when_nothing_precedes_it() {
        let pairs = vec![AlignedPair {
            a_index: 3,
            b_index: 2,
        }];
        // a=0 has no matched pair before it at all — a bonus page ahead of all real content.
        assert_eq!(default_insert_after_b_index(0, &pairs), None);
    }

    #[test]
    fn default_insert_after_a_index_uses_the_nearest_preceding_matched_pair() {
        let pairs = vec![
            AlignedPair {
                a_index: 0,
                b_index: 0,
            },
            AlignedPair {
                a_index: 2,
                b_index: 3,
            },
        ];
        assert_eq!(default_insert_after_a_index(1, &pairs), Some(0));
        assert_eq!(default_insert_after_a_index(2, &pairs), Some(0));
    }

    #[test]
    fn default_insert_after_a_index_is_none_when_nothing_precedes_it() {
        let pairs = vec![AlignedPair {
            a_index: 2,
            b_index: 3,
        }];
        assert_eq!(default_insert_after_a_index(0, &pairs), None);
    }
}
