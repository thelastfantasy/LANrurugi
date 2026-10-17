//! Permutohedral lattice — the high-dimensional Gaussian-filtering data structure
//! [`DenseCrf`](crate::densecrf::DenseCrf) uses to make its pairwise potentials tractable
//! (Adams, Baek & Davis, "Fast High-Dimensional Filtering Using the Permutohedral Lattice", 2010).
//!
//! This is a line-for-line port of the scalar (non-SSE) code path in
//! `philkr/densecrf`'s own `permutohedral.cpp` (the C++ library `pydensecrf` — the library
//! `zyddnys/manga-image-translator`'s own `mask_refinement/text_mask_utils.py::refine_mask` calls
//! into — binds; source obtained via `pip download pydensecrf --no-binary :all:`, extracted to
//! `~/pydensecrf` for reference, 2026-09-16), not an independent re-derivation from the paper. A
//! from-scratch reimplementation of a numerical algorithm like this is exactly the kind of code
//! that can be subtly wrong without ever panicking or failing a superficial test — matching a
//! known-correct reference implementation line-for-line, function-for-function, is the actual
//! correctness argument here, not "this looks like it should work." The SSE-vectorised code path
//! in the original is skipped entirely — it's a performance optimisation of the exact same
//! algorithm the scalar path already implements correctly, not a behavioural difference, and this
//! crate's own crop sizes (a manga text region, at most a few hundred pixels per side) never
//! approach the scale where that would matter.
//!
//! Naming follows the original C++ as closely as Rust idiom allows (`d_`, `N_`, `M_`,
//! `offset_`, `rank_`, `barycentric_`, `blur_neighbors_`) specifically so a future reader can
//! diff this against the original source side-by-side rather than having to first mentally
//! rename everything.

/// One lattice vertex's two neighbours along a given axis, found during [`Permutohedral::init`]
/// and consulted during every [`Permutohedral::compute`] call's blur pass. `-1` (via `Option`
/// here, `-1` `int` in the original) means "no such neighbour exists in the lattice" — a real,
/// expected case at the lattice's own boundary, not an error.
#[derive(Debug, Clone, Copy)]
struct Neighbors {
    n1: Option<usize>,
    n2: Option<usize>,
}

/// The permutohedral lattice built for one fixed set of `N` `d`-dimensional feature vectors —
/// see [`Permutohedral::init`]. Once built, [`Permutohedral::compute`] can filter any per-point
/// value array (any `value_size`) against the same lattice geometry, which is exactly how
/// [`crate::densecrf::DenseCrf`] reuses one lattice across every mean-field iteration: the
/// *positions* (pixel coordinates, pixel colours) never change, only the per-pixel *values*
/// (the current label-probability belief `Q`) being filtered do.
pub struct Permutohedral {
    d: usize,
    n: usize,
    m: usize,
    /// `(d+1) * n` — for each of the `n` input points, the lattice-vertex index (into the `m`
    /// vertices this lattice actually has) of each of its `d+1` enclosing simplex corners.
    offset: Vec<usize>,
    /// `(d+1) * n` — for each input point, which position (0..=d) each of its `d+1` simplex
    /// corners occupies in that point's own sorted-coordinate rank. Only [`Permutohedral`]'s own
    /// (currently unused) gradient computation needs this — kept for parity with the original,
    /// harmless to carry.
    #[allow(dead_code)]
    rank: Vec<i32>,
    /// `(d+1) * n` — the barycentric weight of each input point's `d+1` enclosing simplex
    /// corners (how much of that point's own value gets splatted onto each corner).
    barycentric: Vec<f32>,
    /// `(d+1) * m` — for each of the `m` lattice vertices, its two neighbours along each of the
    /// `d+1` lattice axes (used by the blur pass).
    blur_neighbors: Vec<Neighbors>,
}

/// Open-addressing hash table keyed by a `d`-`i16`-element lattice coordinate — `philkr/densecrf`'s
/// own `HashTable` (`permutohedral.cpp`), used only during [`Permutohedral::init`] to give each
/// distinct lattice vertex touched by any input point a stable, dense `0..m` index. Not a general-
/// purpose hash map: `find` doubles as both lookup (`create: false`) and insert-if-absent
/// (`create: true`), matching the original's own dual-purpose method exactly (kept as one method,
/// not split into two, for the same side-by-side-diffable reason as this module's own top-level
/// doc comment explains).
struct HashTable {
    key_size: usize,
    keys: Vec<i16>,
    /// Maps a key's hash bucket to the vertex index stored there, `None` when empty. A `Vec`
    /// indexed by bucket, not a real `std::collections::HashMap`, because the original's own
    /// linear-probing collision resolution (walk forward from the hash bucket until an empty slot
    /// or a matching key) is part of what makes `grow`'s own rehash correct — swapping in a
    /// generic hash map would change the collision behaviour, not just the storage.
    table: Vec<Option<usize>>,
    filled: usize,
}

impl HashTable {
    fn new(key_size: usize, expected_capacity: usize) -> Self {
        let capacity = (2 * expected_capacity).max(1);
        Self {
            key_size,
            keys: Vec::with_capacity((capacity / 2 + 10) * key_size),
            table: vec![None; capacity],
            filled: 0,
        }
    }

    fn hash(key: &[i16]) -> usize {
        let mut r: u64 = 0;
        for &k in key {
            // The original computes this in native (32-bit on most platforms `philkr/densecrf`
            // targeted) `size_t` arithmetic, wrapping on overflow rather than panicking — `size_t`
            // has no overflow-checked variant in C++, wrapping *is* its defined behaviour. `u64`
            // with explicit `wrapping_*` reproduces exactly that, just at a wider width (harmless:
            // only the low bits feed the final `% capacity_`, and wrapping arithmetic's low bits
            // are unaffected by operating at a wider width throughout).
            r = r.wrapping_add(k as i64 as u64);
            r = r.wrapping_mul(1_664_525);
        }
        r as usize
    }

    fn grow(&mut self) {
        let old_capacity = self.table.len();
        let new_capacity = old_capacity * 2;
        let mut new_table: Vec<Option<usize>> = vec![None; new_capacity];
        for old_bucket in 0..old_capacity {
            let Some(e) = self.table[old_bucket] else {
                continue;
            };
            let key = self.key(e).to_vec();
            let mut h = Self::hash(&key) % new_capacity;
            while new_table[h].is_some() {
                h = if h + 1 < new_capacity { h + 1 } else { 0 };
            }
            new_table[h] = Some(e);
        }
        self.table = new_table;
    }

    fn key(&self, index: usize) -> &[i16] {
        &self.keys[index * self.key_size..(index + 1) * self.key_size]
    }

    /// Finds `key`'s vertex index, inserting a new one (appending to `keys`, assigning it the
    /// next `filled` index) if `create` is set and it isn't already present. Returns `None` on a
    /// lookup miss with `create: false` — the original's own `-1` sentinel.
    fn find(&mut self, key: &[i16], create: bool) -> Option<usize> {
        debug_assert_eq!(key.len(), self.key_size);
        if 2 * self.filled >= self.table.len() {
            self.grow();
        }
        let capacity = self.table.len();
        let mut h = Self::hash(key) % capacity;
        loop {
            match self.table[h] {
                None => {
                    if !create {
                        return None;
                    }
                    self.keys.extend_from_slice(key);
                    let e = self.filled;
                    self.filled += 1;
                    self.table[h] = Some(e);
                    return Some(e);
                }
                Some(e) => {
                    if self.key(e) == key {
                        return Some(e);
                    }
                    h = if h + 1 < capacity { h + 1 } else { 0 };
                }
            }
        }
    }

    fn size(&self) -> usize {
        self.filled
    }
}

impl Permutohedral {
    /// Builds the lattice for `features` (`d` rows, `n` columns — one `d`-dimensional feature
    /// vector per column, matching the original's own `Eigen::MatrixXf` column-major convention)
    /// — the "elevate a point into (d+1)-space, round to the nearest lattice vertex, compute
    /// barycentric weights against its enclosing simplex" step described in §3 of Adams et al.
    /// 2010, ported line-for-line from `permutohedral.cpp`'s own scalar `init` (the `#else` branch
    /// guarded by `SSE_PERMUTOHEDRAL` in the original — see this module's own top-level doc
    /// comment for why the SSE path is skipped).
    pub fn init(features: &[Vec<f32>]) -> Self {
        let d = features.len();
        let n = features.first().map_or(0, |f| f.len());
        debug_assert!(features.iter().all(|f| f.len() == n));

        let mut hash_table = HashTable::new(d, n * (d + 1));

        let mut offset = vec![0usize; (d + 1) * n];
        let mut rank = vec![0i32; (d + 1) * n];
        let mut barycentric = vec![0f32; (d + 1) * n];

        // Compute the canonical simplex (p.4 in Adams et al. 2010) — `canonical[i][j]` for
        // `i in 0..=d`, `j in 0..=d`.
        let mut canonical = vec![0i16; (d + 1) * (d + 1)];
        for i in 0..=d {
            for j in 0..=(d - i) {
                canonical[i * (d + 1) + j] = i as i16;
            }
            for j in (d - i + 1)..=d {
                canonical[i * (d + 1) + j] = i as i16 - (d as i16 + 1);
            }
        }

        // Expected standard deviation of the filter (p.6 in Adams et al. 2010).
        let inv_std_dev = (2.0f64 / 3.0).sqrt() * (d as f64 + 1.0);
        // The diagonal part of the elevation matrix E (p.5 in Adams et al. 2010).
        let scale_factor: Vec<f64> = (0..d)
            .map(|i| 1.0 / (((i + 2) * (i + 1)) as f64).sqrt() * inv_std_dev)
            .collect();

        let mut elevated = vec![0f64; d + 1];
        let mut rem0 = vec![0f64; d + 1];
        let mut point_rank = vec![0i32; d + 1];
        let mut barycentric_local = vec![0f64; d + 2];
        // `d`-length, not `d+1`: only `key[0..d]` is ever written below — the original's own
        // `short key[d_+1]` over-allocates by one element that `HashTable::find` never reads
        // (its internal loops are bounded by `key_size_ == d_`), same as the `n1`/`n2` buffers
        // further down in this function.
        let mut key = vec![0i16; d];

        for point in 0..n {
            let f: Vec<f64> = (0..d).map(|j| features[j][point] as f64).collect();

            // Elevate the feature (y = Ep, p.5 in Adams et al. 2010).
            let mut sm = 0.0;
            for j in (1..=d).rev() {
                let cf = f[j - 1] * scale_factor[j - 1];
                elevated[j] = sm - (j as f64) * cf;
                sm += cf;
            }
            elevated[0] = sm;

            // Find the closest 0-coloured simplex through rounding.
            let down_factor = 1.0 / (d as f64 + 1.0);
            let up_factor = d as f64 + 1.0;
            let mut sum = 0i32;
            for i in 0..=d {
                let v = down_factor * elevated[i];
                let up = v.ceil() * up_factor;
                let down = v.floor() * up_factor;
                let rd = if up - elevated[i] < elevated[i] - down {
                    up
                } else {
                    down
                };
                rem0[i] = rd;
                sum += (rd * down_factor).round() as i32;
            }

            // Find the simplex we are in and store it in `point_rank` — `point_rank[i]` is i's
            // position in the sorted order of `elevated[i] - rem0[i]`.
            for r in point_rank.iter_mut().take(d + 1) {
                *r = 0;
            }
            for i in 0..d {
                let di = elevated[i] - rem0[i];
                for j in (i + 1)..=d {
                    let dj = elevated[j] - rem0[j];
                    if di < dj {
                        point_rank[i] += 1;
                    } else {
                        point_rank[j] += 1;
                    }
                }
            }

            // If the point doesn't lie on the plane (sum != 0), bring it back.
            for i in 0..=d {
                point_rank[i] += sum;
                if point_rank[i] < 0 {
                    point_rank[i] += d as i32 + 1;
                    rem0[i] += d as f64 + 1.0;
                } else if point_rank[i] > d as i32 {
                    point_rank[i] -= d as i32 + 1;
                    rem0[i] -= d as f64 + 1.0;
                }
            }

            // Compute the barycentric coordinates (p.10 in Adams et al. 2010).
            for b in barycentric_local.iter_mut().take(d + 2) {
                *b = 0.0;
            }
            for i in 0..=d {
                let v = (elevated[i] - rem0[i]) * down_factor;
                barycentric_local[d - point_rank[i] as usize] += v;
                barycentric_local[d - point_rank[i] as usize + 1] -= v;
            }
            // Wrap around.
            barycentric_local[0] += 1.0 + barycentric_local[d + 1];

            // Compute all vertices and their offset.
            for remainder in 0..=d {
                for i in 0..d {
                    key[i] =
                        rem0[i] as i16 + canonical[remainder * (d + 1) + point_rank[i] as usize];
                }
                let vertex = hash_table
                    .find(&key, true)
                    .expect("create: true always finds");
                offset[point * (d + 1) + remainder] = vertex;
                rank[point * (d + 1) + remainder] = point_rank[remainder];
                barycentric[point * (d + 1) + remainder] = barycentric_local[remainder] as f32;
            }
        }

        // Find the neighbours of each lattice vertex — used by the blur pass.
        let m = hash_table.size();
        let mut blur_neighbors = vec![Neighbors { n1: None, n2: None }; (d + 1) * m];
        // The original allocates these as `d_+1`-length buffers but `find` only ever reads the
        // first `d_` elements through a raw pointer — a harmless historical over-allocation, not
        // part of the algorithm. This port's own `HashTable::find` asserts its input is exactly
        // `key_size` (`d`) long, so these stay `d`-length here rather than reproducing the extra
        // unused slot.
        let mut n1 = vec![0i16; d];
        let mut n2 = vec![0i16; d];
        for axis in 0..=d {
            for vertex in 0..m {
                let vertex_key = hash_table.key(vertex).to_vec();
                for k in 0..d {
                    n1[k] = vertex_key[k] - 1;
                    n2[k] = vertex_key[k] + 1;
                }
                if axis < d {
                    n1[axis] = vertex_key[axis] + d as i16;
                    n2[axis] = vertex_key[axis] - d as i16;
                }
                blur_neighbors[axis * m + vertex] = Neighbors {
                    n1: hash_table.find(&n1, false),
                    n2: hash_table.find(&n2, false),
                };
            }
        }

        Self {
            d,
            n,
            m,
            offset,
            rank,
            barycentric,
            blur_neighbors,
        }
    }

    /// Filters `values` (`n` points, each a `value_size`-vector, row-major: `values[i * value_size
    /// + k]`) through the lattice built by [`init`](Self::init) — `philkr/densecrf`'s own
    /// `seqCompute`. The three named phases (splat, blur, slice) are exactly Adams et al. 2010's
    /// own algorithm: splat scatters each input point's value onto its enclosing simplex's `d+1`
    /// lattice vertices (weighted by barycentric coordinate), blur convolves along the lattice
    /// (approximating the high-dimensional Gaussian this whole structure exists to make tractable
    /// — see this module's own top-level doc comment), slice gathers each point's own filtered
    /// value back out by re-reading the same `d+1` vertices its splat touched.
    ///
    /// `reverse` runs the blur axes in the opposite order — [`crate::densecrf::pairwise`]'s own
    /// `apply`/`applyTranspose` distinction, which this crate doesn't currently call
    /// (`applyTranspose` is only needed for gradient computation, which nothing here does), but
    /// kept for parity with the original since dropping it would silently make this a different,
    /// harder-to-diff function than its source.
    pub fn compute(&self, values: &[f32], value_size: usize, reverse: bool) -> Vec<f32> {
        debug_assert_eq!(values.len(), self.n * value_size);
        // Shift all lattice-vertex indices by 1 so that "no neighbour" (`None`, `-1` in the
        // original) maps to index 0 — a real, dedicated all-zero sentinel row/column at the front
        // of `lattice_values`, exactly as the original's own `+1`-everywhere indexing scheme
        // relies on.
        let mut lattice_values = vec![0f32; (self.m + 2) * value_size];
        let mut new_lattice_values = vec![0f32; (self.m + 2) * value_size];

        // Splat.
        for i in 0..self.n {
            for j in 0..=self.d {
                let o = self.offset[i * (self.d + 1) + j] + 1;
                let w = self.barycentric[i * (self.d + 1) + j];
                for k in 0..value_size {
                    lattice_values[o * value_size + k] += w * values[i * value_size + k];
                }
            }
        }

        // Blur — one pass per lattice axis, each pass a 3-tap (self, n1, n2) filter along that
        // axis (p.7 in Adams et al. 2010: this is what approximates the full (d+1)-dimensional
        // Gaussian as a sequence of cheap 1-D blurs).
        let mut current = lattice_values.as_mut_slice();
        let mut next = new_lattice_values.as_mut_slice();
        let axes: Vec<usize> = if reverse {
            (0..=self.d).rev().collect()
        } else {
            (0..=self.d).collect()
        };
        for axis in axes {
            for i in 0..self.m {
                let neighbors = self.blur_neighbors[axis * self.m + i];
                let n1 = neighbors.n1.map_or(0, |v| v + 1);
                let n2 = neighbors.n2.map_or(0, |v| v + 1);
                let self_idx = i + 1;
                for k in 0..value_size {
                    next[self_idx * value_size + k] = current[self_idx * value_size + k]
                        + 0.5 * (current[n1 * value_size + k] + current[n2 * value_size + k]);
                }
            }
            std::mem::swap(&mut current, &mut next);
        }

        // Alpha is a magic scaling constant matching the blur normalisation — see this crate's
        // own doc comment on why this stays a literal transcription rather than a "derived"
        // constant: the original's own comment ("write Andrew if you really wanna understand
        // this") is itself the citation for why this isn't independently re-derived here.
        let alpha = 1.0f32 / (1.0 + 2f32.powf(-(self.d as f32)));

        // Slice.
        let mut out = vec![0f32; self.n * value_size];
        for i in 0..self.n {
            for j in 0..=self.d {
                let o = self.offset[i * (self.d + 1) + j] + 1;
                let w = self.barycentric[i * (self.d + 1) + j];
                for k in 0..value_size {
                    out[i * value_size + k] += w * current[o * value_size + k] * alpha;
                }
            }
        }
        out
    }
}
