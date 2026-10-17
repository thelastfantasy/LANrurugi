# Quickstart: Translation and Erase Support for Pages Beyond the Worker Payload Limit

Validation guide for feature `012-large-page-translation`. Scenarios 1–3 need no GPU and no model
weights; scenarios 4–6 need the full dev deployment with the erase model present.

## Prerequisites

- Repository checked out, workspace built through the project's own tasks. Never invoke `cargo`
  directly — every build and test goes through `mise run`, which applies the project's resource caps.
- Scenarios 1–3 and 6 exist as opt-in automated tests (see each scenario below); running them needs
  `mise run build` first, because the worker executable they spawn is **not** produced by
  `cargo test -p lanrurugi-gpu-worker` (`--bins` under `cargo test` builds a hashed test-harness
  binary in `target/debug/deps/`, not `target/debug/lanrurugi-gpu-worker`).
- Scenarios 4–5 only: the dev container running with the erase model mounted.
- For the pixel-identity comparison in scenario 6: a build of the pre-change revision, because the
  reference output must come from the *old* binary rather than from a re-implementation.

### Where the model has to live

The container `mise run test`/`check-crate` runs in is built from `Dockerfile.build`, which ships **no
ONNX Runtime at all** — so any test that loads a model must point the worker at one explicitly:

```sh
# in .env.test.local (gitignored; read only by scripts/cargo-container-run.sh)
LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH=/workspace/data/models/libonnxruntime.so
```

Without it the worker is handed the client's vendor-guessed path, fails to load it, and reports the
misleading `no inpainting model loaded in this worker`. See the feature's `tasks.md` Implementation
Record, finding 3.

### Page fixtures are synthetic, not archival

No real archive is needed, and none should be committed. Every input these scenarios use is generated
in-process at a chosen resolution, which is what the behaviour actually depends on: the boundary
decision is a pure function of `width * height`, so a synthetic 4000x3600 page exercises exactly what
a 4000x3600 scan would. This keeps the checks deterministic and independent of what any particular
library happens to contain.

If you want to sanity-check the resolution figures against a real library, probe page dimensions
without extracting whole archives:

```sh
# First page of every archive, dimensions only — anything whose width*height exceeds ~13.4 million
# would previously have been refused at the boundary.
for z in data/manga/*.zip; do
  first=$(unzip -Z1 "$z" | grep -iE '\.(png|jpg|jpeg|webp)$' | head -1)
  [ -z "$first" ] && continue
  dims=$(unzip -p "$z" "$first" | head -c 200000 | file - | sed 's/.*image data, //;s/,.*//')
  echo "$dims  $z"
done | sort -rn
```

`file` does not decode every JPEG's frame header within that window; treat its silence as "unknown",
not as "small". The dimension figures recorded in the feature's `spec.md` Context section came from a
full header parse over 209 archives.

## Scenario 1 — The size arithmetic matches the real wire format

**Proves**: FR-013's precondition. The declared request size is not a restatement of the
implementation's own arithmetic but a measurement of what actually gets serialized.

```sh
mise run check-crate -- lanrurugi-gpu-ipc
```

**Expected**: a test that serializes the same three arguments an erase request carries — page, model
mask, paste mask — with `bincode` at default options, and asserts the byte length equals
`48 + 5*w*h` for several `(w, h)` pairs including a large one. It passes.

**Why it matters**: if this ever fails, the payload arithmetic has drifted from the transport's actual
framing, and every size decision downstream is being made on a stale number.

## Scenario 2 — The surveyed worst-case pages are provably over the limit, and the reduced path fits

**Proves**: FR-002, FR-003, FR-005, and FR-013's outcome, without needing a GPU.

```sh
mise run check-crate -- lanrurugi-gpu-ipc lanrurugi-inpaint
```

**Expected**: tests asserting, for each page dimension surveyed in the feature's research (in
particular 6336x9504 and 6084x3943):

1. The full-resolution request does **not** provably fit.
2. `work_dims` returns a pair whose longest edge equals the working resolution exactly.
3. The reduced request fits with at least a 4x margin, and its size does not vary with the original
   page's dimensions once above the threshold.
4. `work_dims` is idempotent: `work_dims(work_dims(w, h)) == work_dims(w, h)`.

Point 2 is the load-bearing one — it is what makes the worker skip its own reduction.

## Scenario 3 — A page already within the working resolution is never resampled

**Proves**: FR-009 and the "provably fits" branch of the decision predicate.

```sh
mise run check-crate -- lanrurugi-inpaint
```

**Expected**: a test asserting `work_dims(800, 1000) == (800, 1000)` — note that the comparison is on
the *longest edge*, so a page like 800x1200 is already over the cap and is correctly reduced — that
`downscale_to_work` returns data identical to its input for such a page, and that the size predicate
for that page is true — i.e. it takes the existing path and the pixel-identity guarantee applies to it
by construction.

## Scenario 4 — An over-limit page erases (the actual defect)

**Proves**: US1, SC-001, SC-002, SC-004.

Automated, at the layer this feature changes:

```sh
# once, after any source change; not needed again until the worker crate is rebuilt
mise run build
# LANRURUGI_GPU_WORKER_BOUNDARY_E2E=1 in .env.test.local
mise run test-crate -- lanrurugi-api
```

**Expected**: `tests/gpu_worker_boundary.rs` passes, having erased a synthetic 4000x3600 page (14.4 MP
— over the limit) through a real worker and real weights, and asserting the returned page keeps its
dimensions and is byte-identical to its input outside the paste mask. The test prints the payload
figures, which should show the full-resolution request refused and the reduced one accepted with an
order of magnitude to spare.

**What this does not cover**: the reader-facing end of the scenario — requesting translation for a real
archive through the HTTP API and being served a translated image. That path was not exercised; it needs
the dev container rebuilt with this change, and the boundary call it depends on is what this test
verifies. Recorded rather than glossed over.

## Scenario 5 — Bubble detection above its own limit is unchanged

**Proves**: FR-021 (bubble detection deliberately untouched).

**Expected**: unchanged from before this feature — a page above ~22.4 MP still degrades to rectangular
erasure boxes. **Not runnable in this environment**: no bubble-segmentation model is installed, so the
call cannot be exercised at all (the worker logs `no bubble segmentation model installed`). Nothing in
this feature claims to have verified it.

## Scenario 6 — Pages that already worked are pixel-identical

**Proves**: US2, FR-008, SC-003. This is the gate that makes the whole feature safe, and it is the one
scenario that cannot be satisfied by reasoning about the code.

Automated, using a capture test that deliberately compiles against **both** revisions (it uses no
API this feature added):

```sh
# 1. Pre-change reference: stash this feature's four files, rebuild, capture.
git stash push -- crates/lanrurugi-inpaint/src/lib.rs crates/lanrurugi-gpu-ipc/src/lib.rs \
                   crates/lanrurugi-gpu-ipc/Cargo.toml crates/lanrurugi-api/src/gpu_worker_client.rs
#    (also move tests/gpu_worker_boundary.rs aside — it references API that does not exist yet)
#    LANRURUGI_GPU_WORKER_ERASE_CAPTURE=<path> in .env.test.local
mise run test-crate -- lanrurugi-api
cp <path> before.raw

# 2. Post-change: restore everything, rebuild, capture again.
git stash pop
mise run test-crate -- lanrurugi-api
cp <path> after.raw

# 3. Compare.
sha256sum before.raw after.raw
```

**Expected**: identical checksums. Both runs also print the input's hash, which must match — otherwise
the comparison is between different inputs and proves nothing.

**Note on what is compared**: the capture is the erase boundary's own raw output, not an HTTP-served
PNG. That is the layer this feature changes, and it avoids re-encoding in the middle, but it is not
literally the "served image" the scenario is worded around. The deviation is deliberate and recorded.

## Scenario 7 — Nothing to erase still makes no boundary call

**Proves**: FR-012.

**Expected**: for a page where every detected region is handled by a flat fill, no erase call is made,
as today. Observable by the absence of any erase-related log line for that page. This feature must not
turn the existing short-circuit into a wasted round trip.

## Rollback

The change is confined to in-flight computation with no persisted state, so rollback is reverting the
commit. No migration, no cache invalidation, and no data repair is required in either direction.
