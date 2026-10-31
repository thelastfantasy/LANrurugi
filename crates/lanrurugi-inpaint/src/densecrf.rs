//! Dense (fully-connected) conditional random field inference — Krähenbühl & Koltun, "Efficient
//! Inference in Fully Connected CRFs with Gaussian Edge Potentials" (NIPS 2011), used here to
//! refine a text-region mask the same way `zyddnys/manga-image-translator`'s own
//! `mask_refinement/text_mask_utils.py::refine_mask` does: a per-pixel unary belief (from the
//! OCR/segmentation model) gets pulled toward agreement with nearby pixels of similar position
//! and colour, which is exactly what lets a mask follow real glyph edges instead of the coarse
//! blob a segmentation model alone produces.
//!
//! Like [`crate::permutohedral`], this is a line-for-line port of `philkr/densecrf` (the C++
//! library `pydensecrf` binds; source in `~/pydensecrf`, 2026-09-16) rather than an independent
//! re-derivation from the paper — see that module's own doc comment for the rationale. This file
//! ports only the subset of `densecrf.cpp`/`pairwise.cpp`/`labelcompatibility.cpp` that
//! `text_mask_utils.py::refine_mask` actually exercises: `DenseCRF2D`, `setUnaryEnergy` from a
//! softmax (`unary_from_softmax` in `pydensecrf/utils.py`), `addPairwiseGaussian` and
//! `addPairwiseBilateral` (both always called there with `kernel=DIAG_KERNEL,
//! normalization=NO_NORMALIZATION`), `PottsCompatibility` (a plain scalar `compat=` weight, no
//! per-label matrix), and `inference`. The learning/gradient machinery (`stepInference`'s
//! step-by-step variant, `klDivergence`, every `*Gradient`/`*Parameters` method, `FULL_KERNEL`,
//! `NORMALIZE_BEFORE`/`NORMALIZE_AFTER`/`NORMALIZE_SYMMETRIC`, `DiagonalCompatibility`,
//! `MatrixCompatibility`) is all genuinely unused dead weight for this crate's one job (refining a
//! fixed mask, never training a model) — leaving it out is not a simplification of the algorithm,
//! it's not porting code paths nothing here calls.
//!
//! Naming again follows the original C++ (`N_`, `M_`, `W_`, `H_`, `unary_`, `pairwise_`) for the
//! same side-by-side-diffability reason given in `permutohedral.rs`.

use crate::permutohedral::Permutohedral;

/// One pairwise term: a permutohedral lattice built over some per-pixel feature space (position
/// alone for `addPairwiseGaussian`, position+colour for `addPairwiseBilateral`), plus the Potts
/// weight applied to its filtered output. Corresponds to `PairwisePotential` wrapping a
/// `DenseKernel` (always `DIAG_KERNEL`) and a `PottsCompatibility` in the original — collapsed
/// into one struct here since this crate never uses any other kernel or compatibility type (see
/// this module's own top-level doc comment).
struct PairwiseTerm {
    lattice: Permutohedral,
    /// `philkr/densecrf`'s own `DenseKernel::norm_`, computed once in `initLattice` by filtering
    /// an all-ones signal through the lattice. Under `NO_NORMALIZATION` (the only mode this crate
    /// uses) `filter` never actually reads `norm_` — see [`PairwiseTerm::apply`] — so this field
    /// is dead in the same way it already is on the original's own `NO_NORMALIZATION` path; kept
    /// only so [`PairwiseTerm::new`] stays a faithful transcription of `initLattice` rather than
    /// silently dropping a step the original always performs.
    #[allow(dead_code)]
    norm: Vec<f32>,
    /// The Potts weight `w_` — `PottsCompatibility::apply` is just `out = -w_ * Q`.
    weight: f32,
}

impl PairwiseTerm {
    /// `DenseKernel`'s constructor + `initLattice`, specialised to `DIAG_KERNEL` +
    /// `NO_NORMALIZATION` (`pairwise.cpp` lines 40-61, the `else` branch since neither
    /// `NORMALIZE_SYMMETRIC` nor any other mode is ever selected here).
    fn new(features: &[Vec<f32>], weight: f32) -> Self {
        let lattice = Permutohedral::init(features);
        let n = features.first().map_or(0, |f| f.len());

        let ones = vec![1f32; n];
        let mut norm = lattice.compute(&ones, 1, false);

        // `NO_NORMALIZATION`: replace every entry with the same mean norm (pairwise.cpp lines
        // 46-53) rather than a per-pixel value — the original's own documented "substantial
        // approximation error" tradeoff in exchange for speed, and exactly what
        // `text_mask_utils.py::refine_mask` opts into for both its pairwise terms.
        let mean_norm = if n > 0 {
            (n as f32) / norm.iter().sum::<f32>()
        } else {
            0.0
        };
        for v in norm.iter_mut() {
            *v = mean_norm;
        }

        Self {
            lattice,
            norm,
            weight,
        }
    }

    /// `PairwisePotential::apply`: `DenseKernel::filter` (`NO_NORMALIZATION`, non-transpose —
    /// `pairwise.cpp` lines 63-80: since `ntype_` is neither `NORMALIZE_SYMMETRIC` nor
    /// `NORMALIZE_BEFORE`-with-`!transpose`, the pre-filter branch is skipped, only the
    /// post-filter `NORMALIZE_AFTER`-with-`!transpose`... which also doesn't match, so `out` stays
    /// the raw lattice output — `NO_NORMALIZATION` genuinely applies `norm_` nowhere in `filter`
    /// itself, only inside `initLattice`'s one-time mean-norm computation above) followed by
    /// `PottsCompatibility::apply`.
    fn apply(&self, q: &[f32], m: usize, n: usize) -> Vec<f32> {
        // `q` is `M_ x N_` column-major in the original (label-major per pixel here instead —
        // see [`DenseCrf::inference`] for the row/column convention actually used in this port).
        let filtered = self.lattice.compute(q, m, false);
        debug_assert_eq!(filtered.len(), m * n);
        filtered.iter().map(|&v| -self.weight * v).collect()
    }
}

/// A 2-D dense CRF over `w * h` pixels with `m` labels — `DenseCRF2D` in the original, flattened
/// to this crate's own row-major `(x, y) -> y * w + x` pixel order (matching
/// `DenseCRF2D::addPairwiseGaussian`'s own `j*W_+i` indexing exactly, so no reordering is needed
/// anywhere in this port).
pub struct DenseCrf2d {
    w: usize,
    h: usize,
    m: usize,
    /// `M_ x N_`, but stored row-major *per pixel* (`unary[pixel * m + label]`) rather than the
    /// original's column-major `Eigen::MatrixXf` — chosen because every access pattern in this
    /// port (splat/slice per pixel, softmax per pixel) is naturally pixel-major; the original's
    /// per-label-column layout is a BLAS-friendliness choice this port has no need to replicate.
    unary: Vec<f32>,
    pairwise: Vec<PairwiseTerm>,
}

impl DenseCrf2d {
    pub fn new(w: usize, h: usize, m: usize) -> Self {
        Self {
            w,
            h,
            m,
            unary: vec![0.0; w * h * m],
            pairwise: Vec::new(),
        }
    }

    /// `pydensecrf.utils.unary_from_softmax(sm, clip=1e-5)` (the only call site
    /// `text_mask_utils.py::refine_mask` uses — no `scale` argument passed, so that branch is
    /// skipped): unary energy is the negative log of the (clipped) class probability. `probs` is
    /// pixel-major, `probs[pixel * m + label]`, matching this struct's own `unary` layout.
    pub fn set_unary_energy_from_softmax(&mut self, probs: &[f32]) {
        debug_assert_eq!(probs.len(), self.w * self.h * self.m);
        const CLIP: f32 = 1e-5;
        for (u, &p) in self.unary.iter_mut().zip(probs.iter()) {
            *u = -p.max(CLIP).ln();
        }
    }

    /// `DenseCRF2D::addPairwiseGaussian` (`densecrf.cpp` lines 61-69): feature space is just
    /// `(x/sx, y/sy)`.
    pub fn add_pairwise_gaussian(&mut self, sx: f32, sy: f32, compat: f32) {
        let n = self.w * self.h;
        let mut fx = vec![0f32; n];
        let mut fy = vec![0f32; n];
        for j in 0..self.h {
            for i in 0..self.w {
                let idx = j * self.w + i;
                fx[idx] = i as f32 / sx;
                fy[idx] = j as f32 / sy;
            }
        }
        self.pairwise.push(PairwiseTerm::new(&[fx, fy], compat));
    }

    /// `DenseCRF2D::addPairwiseBilateral` (`densecrf.cpp` lines 70-81): feature space is
    /// `(x/sx, y/sy, r/sr, g/sg, b/sb)`. `rgb` is interleaved `[r,g,b,r,g,b,...]`, `w*h` pixels,
    /// matching the original's own `im[(i+j*W_)*3+c]` indexing.
    #[allow(clippy::too_many_arguments)]
    pub fn add_pairwise_bilateral(
        &mut self,
        sx: f32,
        sy: f32,
        sr: f32,
        sg: f32,
        sb: f32,
        rgb: &[u8],
        compat: f32,
    ) {
        let n = self.w * self.h;
        debug_assert_eq!(rgb.len(), n * 3);
        let mut fx = vec![0f32; n];
        let mut fy = vec![0f32; n];
        let mut fr = vec![0f32; n];
        let mut fg = vec![0f32; n];
        let mut fb = vec![0f32; n];
        for j in 0..self.h {
            for i in 0..self.w {
                let idx = j * self.w + i;
                fx[idx] = i as f32 / sx;
                fy[idx] = j as f32 / sy;
                fr[idx] = rgb[idx * 3] as f32 / sr;
                fg[idx] = rgb[idx * 3 + 1] as f32 / sg;
                fb[idx] = rgb[idx * 3 + 2] as f32 / sb;
            }
        }
        self.pairwise
            .push(PairwiseTerm::new(&[fx, fy, fr, fg, fb], compat));
    }

    /// `expAndNormalize` (`densecrf.cpp` lines 98-106), specialised to this port's pixel-major
    /// layout: softmax over the `m` labels of one pixel, in place per pixel rather than per
    /// Eigen column.
    fn exp_and_normalize(out: &mut [f32], input: &[f32], n: usize, m: usize) {
        for pixel in 0..n {
            let row = &input[pixel * m..(pixel + 1) * m];
            let max = row.iter().copied().fold(f32::MIN, f32::max);
            let mut sum = 0f32;
            let out_row = &mut out[pixel * m..(pixel + 1) * m];
            for (o, &v) in out_row.iter_mut().zip(row.iter()) {
                let e = (v - max).exp();
                *o = e;
                sum += e;
            }
            if sum > 0.0 {
                for o in out_row.iter_mut() {
                    *o /= sum;
                }
            }
        }
    }

    /// `DenseCRF::inference` (`densecrf.cpp` lines 115-131): mean-field fixed-point iteration.
    /// `Q = softmax(-unary)` initially, then `n_iterations` rounds of `Q =
    /// softmax(-unary - sum_k pairwise_k.apply(Q))`. Returns pixel-major label probabilities,
    /// same layout as [`set_unary_energy_from_softmax`]'s input.
    pub fn inference(&self, n_iterations: usize) -> Vec<f32> {
        let n = self.w * self.h;
        let m = self.m;
        let mut q = vec![0f32; n * m];
        Self::exp_and_normalize(
            &mut q,
            &self.unary.iter().map(|&u| -u).collect::<Vec<_>>(),
            n,
            m,
        );

        // Each `PairwiseTerm::apply`/`Permutohedral::compute` call expects label-major-per-pixel
        // input as `[value_size (=m) per point, n points]` conceptually — but `Permutohedral`'s
        // own `compute` signature is `(values: &[f32], value_size, reverse)` with `values` laid
        // out `[point0_v0..point0_vk, point1_v0..point1_vk, ...]`, i.e. pixel-major with `m`
        // values per pixel — exactly this struct's own `q` layout already, so no transposition is
        // needed here (unlike the original's column-major `Eigen::MatrixXf` where each pairwise
        // term's `Q` argument is literally the same memory `DenseCRF::inference` holds).
        let mut tmp1 = vec![0f32; n * m];
        for _ in 0..n_iterations {
            for (i, u) in self.unary.iter().enumerate() {
                tmp1[i] = -u;
            }
            for term in &self.pairwise {
                let filtered = term.apply(&q, m, n);
                for (t, f) in tmp1.iter_mut().zip(filtered.iter()) {
                    *t -= f;
                }
            }
            Self::exp_and_normalize(&mut q, &tmp1, n, m);
        }
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no pairwise terms at all, `inference` must reduce to a single `softmax(-unary)` —
    /// the `n_iterations` loop still runs, but `tmp1 = -unary` unchanged each time, so every
    /// iteration recomputes the exact same `Q`.
    #[test]
    fn inference_with_no_pairwise_terms_is_just_unary_softmax() {
        let mut crf = DenseCrf2d::new(2, 1, 2);
        // Pixel 0 strongly prefers label 0, pixel 1 strongly prefers label 1.
        crf.set_unary_energy_from_softmax(&[0.9, 0.1, 0.1, 0.9]);
        let q = crf.inference(5);
        assert!(q[0] > q[1], "pixel 0 should favour label 0: {q:?}");
        assert!(q[3] > q[2], "pixel 1 should favour label 1: {q:?}");
    }

    /// A uniform unary (no preference at all) with a Gaussian pairwise term over two identical
    /// pixels must stay perfectly symmetric between the two labels — nothing breaks the tie.
    #[test]
    fn symmetric_input_stays_symmetric() {
        let mut crf = DenseCrf2d::new(2, 1, 2);
        crf.set_unary_energy_from_softmax(&[0.5, 0.5, 0.5, 0.5]);
        crf.add_pairwise_gaussian(1.0, 1.0, 3.0);
        let q = crf.inference(5);
        assert!((q[0] - q[1]).abs() < 1e-4, "expected a tie: {q:?}");
        assert!((q[2] - q[3]).abs() < 1e-4, "expected a tie: {q:?}");
    }

    /// Two pixels of identical colour, next to each other, should pull each other's belief
    /// together under a bilateral term when the unary alone only weakly favours one side —
    /// verifying the pairwise term actually has a directional smoothing effect rather than being
    /// a no-op.
    #[test]
    fn bilateral_term_pulls_similar_neighbours_together() {
        let mut crf = DenseCrf2d::new(2, 1, 2);
        // Pixel 0 weakly favours label 0; pixel 1 is neutral. Same colour for both pixels.
        crf.set_unary_energy_from_softmax(&[0.6, 0.4, 0.5, 0.5]);
        let rgb = [200u8, 200, 200, 200, 200, 200];
        crf.add_pairwise_bilateral(10.0, 10.0, 10.0, 10.0, 10.0, &rgb, 5.0);
        let q = crf.inference(5);
        // Pixel 1 should get pulled toward label 0 too, past its own neutral unary.
        assert!(
            q[2] > 0.5,
            "neighbour should be pulled toward pixel 0's preferred label: {q:?}"
        );
    }
}
