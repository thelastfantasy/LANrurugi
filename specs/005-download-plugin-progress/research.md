# Phase 0 Research: Download Plugin Progress, Concurrency & Rate Limiting

## 1. Zip-writing crate for the `bundle_as_archive: true` case

**Decision**: `zip` crate (latest stable **8.6.0**, MIT-licensed — compatible with this project's
MIT license per constitution Technology Stack Constraints), used synchronously inside
`tokio::task::spawn_blocking`.

**Rationale**: `ZipWriter<W>` is generic over any `std::io::Write + Seek`, so every downloaded
resource's bytes can be written straight into an in-memory `Cursor<Vec<u8>>` — no per-entry
temp-file round-trip needed before the final archive is persisted to the library directory in one
write. It's a synchronous/blocking API; this project's constitution (Principle III) already
establishes the exact pattern needed here — bridge blocking/CPU-bound work via
`tokio::task::spawn_blocking` rather than running it directly on a Tokio worker thread (the same
approach already used for rayon-based hashing/thumbnailing) — so no async-native zip crate is
needed or sought.

**Alternatives considered**: A genuinely async zip-writing crate was searched for and not found
at comparable maturity/adoption to `zip` 8.6.0; introducing one would add dependency risk for no
real benefit once the write is already isolated in `spawn_blocking`.

## 2. Rate limiting: bytes/sec throttling for streamed downloads

**Decision**: `governor` (latest stable **0.10.4**, MIT-licensed), using
`RateLimiter::until_n_ready(NonZeroU32)` with N = bytes read per chunk (not the default one-token-
per-call usage) to throttle a `reqwest` byte stream to a configured bytes/sec cap.

**Rationale**: `governor` directly supports consuming an arbitrary token count per check/wait call
(`check_n`/`until_n_ready`/`until_n_ready_with_jitter`), which maps naturally onto "N bytes just
arrived in this chunk, wait until the bucket can afford them" — no need to call it once per byte.
It's already a widely-used, actively maintained general-purpose Rust rate-limiting crate (not
tied to a specific I/O framework), which keeps the download-manager's rate-limit logic decoupled
from exactly how the byte stream is produced (useful since both the single-URL and per-resource
multi-download cases funnel through the same limiter instance per domain rule).

**Alternatives considered**: Stream-native bandwidth-throttle crates (`async-speed-limit`,
`async-io-throttling`) that wrap an `AsyncRead`/`AsyncWrite` directly were found but not chosen for
this plan — they'd reduce a small amount of byte-accounting boilerplate around each chunk, but
introduce a second, more narrowly-scoped dependency for functionality `governor` (already chosen
for its general token-bucket semantics, and a natural fit for the *concurrency* side too via
`governor`'s own primitives if useful there) already covers. Not using them keeps the download-
manager's rate-limiting logic in one library rather than split across two.

## 3. `reqwest` streaming feature flag

**Decision**: Add `"stream"` to this project's existing `reqwest` feature list in the workspace
`Cargo.toml` (currently `{ version = "0.13", default-features = false, features = ["json",
"rustls-native-certs", "multipart"] }`).

**Rationale**: Confirmed directly against the published `reqwest` 0.13.4 `Cargo.toml` (current
latest stable, MIT OR Apache-2.0 licensed) that `Response::bytes_stream()` — the chunk-by-chunk
read needed to track `downloaded_bytes` incrementally and feed the rate limiter per chunk — is
gated behind the `stream` feature, which pulls in `futures-util`/`tokio-util`/`tokio/fs`. This
feature is not currently enabled in this workspace and must be added as part of this feature's
Setup work; per constitution's "dependencies default to latest stable release, verified at
implementation time" rule, the exact `reqwest` version pin should be re-verified against
crates.io at actual implementation time rather than hard-coded to 0.13.4 from this research pass.

**Alternatives considered**: None — this is a single, unambiguous feature-flag addition to an
already-chosen, already-in-the-workspace dependency, not a library choice.

## 4. Frontend home for per-plugin settings UI

**Decision**: `apps/frontend/src/pages/Plugins.tsx` — the existing plugin-management page — gains
the new per-plugin settings form (User Story 4), rather than a new standalone page.

**Rationale**: This page already lists every installed plugin (grouped by type: Login, Downloaders,
Metadata) as cards with name/version/author/description, and its own existing code comment
explicitly notes that per-plugin parameter/settings editing is "not wired in yet" — i.e. this is
already the intended, anticipated home for exactly this kind of UI, not a new information
architecture decision this feature needs to make. A download-type plugin card gains a "Settings"
affordance that opens the new form (FR-012/FR-015: shown only when `pluginOptions()` declares
configurable fields at all); non-download plugin types are unaffected by this feature.

**Alternatives considered**: A new dedicated top-level "Plugin Settings" page was considered but
rejected — it would duplicate the plugin list this page already renders, and would separate a
plugin's identity information (name/version/author) from its configuration in a way the existing
page's own structure doesn't call for.
