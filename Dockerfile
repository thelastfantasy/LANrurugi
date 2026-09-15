# syntax=docker/dockerfile:1
#
# Multi-stage build per constitution Technology Stack Constraints: Debian bookworm-slim runtime
# (glibc, required by the official Deno distribution — Principle IV), a pinned-version Deno binary
# copied in (never `curl | sh`), fonts-noto-cjk bundled for correct CJK text rasterization, and a
# release build of the single `lanrurugi-server` binary serving the built frontend as static assets
# (Principle III — one deployable).

ARG RUST_VERSION=1.97.1
ARG NODE_VERSION=24.18.0
ARG DENO_VERSION=2.9.1
ARG PNPM_VERSION=11.10.0
ARG REDIS_VERSION=7.4.9
# GPU EP integration (issue #103) — this project's own `Dockerfile.dev` already made this same
# choice for the dev image; production defaults to the plain CPU-only image (unchanged behavior,
# ~430MB smaller, no NVIDIA Container Toolkit/driver prerequisite on the host) and opts into the
# NVIDIA CUDA variant only via `--build-arg LANRURUGI_RUNTIME_BASE=runtime-base-gpu` (selects the
# `runtime-base-gpu` stage below instead of `runtime-base-cpu` — Docker's `FROM ${ARG}` syntax can
# only select between whole named stages, not branch on a boolean inside a single stage, hence the
# two full candidate stages rather than one conditionally-configured one). Either way, the CUDA
# execution provider itself is always attempted-then-CPU-fallback at the *code* level
# (`build_session_with_gpu_fallback` in `lanrurugi-ocr`/`lanrurugi-inpaint`) — this ARG only
# controls whether the CUDA runtime libraries and ORT's CUDA provider plugin are even present in
# the image for that code to find; the GPU variant still runs fine on a host with no GPU at all
# (`ep::CUDA::default().is_available()` returns `Ok(false)`, same CPU fallback path either way),
# it's just needlessly larger for that case.
ARG LANRURUGI_RUNTIME_BASE=runtime-base-cpu

FROM rust:${RUST_VERSION}-slim-bookworm AS rust-builder
ARG LANRURUGI_GIT_SHA=unknown
WORKDIR /build
# cmake + libclang-dev build `libarchive2-sys` (bundles libarchive's C source, built via CMake,
# bindgen needs libclang — see `crates/lanrurugi-scanner/src/archive_format.rs`); the zlib1g-dev/
# libbz2-dev/liblzma-dev/libzstd-dev/liblz4-dev/libxml2-dev/libacl1-dev set are what libarchive
# itself links against (matches `Dockerfile.build`'s dev image, which needs the same set).
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        make \
        gcc \
        g++ \
        cmake \
        libclang-dev \
        zlib1g-dev \
        libbz2-dev \
        liblzma-dev \
        libzstd-dev \
        liblz4-dev \
        libxml2-dev \
        libacl1-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
ENV LANRURUGI_GIT_SHA=${LANRURUGI_GIT_SHA}
RUN cargo build --release -p lanrurugi-server

FROM node:${NODE_VERSION}-slim AS frontend-builder
ARG PNPM_VERSION
WORKDIR /build
RUN corepack enable && corepack prepare pnpm@${PNPM_VERSION} --activate
COPY pnpm-workspace.yaml pnpm-lock.yaml ./
COPY apps/frontend ./apps/frontend
RUN pnpm install --frozen-lockfile --filter lanrurugi-frontend
RUN pnpm --filter lanrurugi-frontend run build

# Official Deno "bin" image variant exists specifically to be copied from in multi-stage builds —
# a pinned tag, never `latest` and never an unverified installer script (Principle IV). It's just
# the `/deno` binary with no shell (nothing else `RUN`s in this stage), so `docs-builder` below
# copies it into a real shell-having image rather than trying to `RUN deno doc` here directly.
FROM docker.io/denoland/deno:bin-${DENO_VERSION} AS deno-bin

# Source of the bundled `redis-server`/`redis-cli` binaries for the `runtime` stage below — the
# official image's own `-bookworm` variant (Debian/glibc, not `-alpine`/musl, to link cleanly
# against the same runtime's glibc) at the exact version Redis data in this project has already
# been written with (`7.4.9`; Debian bookworm's own `apt` package is a much older `7.0.15`, which
# cannot read the newer on-disk RDB/AOF format version 7.4.9 writes — verified the hard way, not
# a hypothetical: pointing this project's real recovered data at a 7.0.15 binary failed outright
# with "Can't handle RDB format version 12").
FROM docker.io/library/redis:${REDIS_VERSION}-bookworm AS redis-bin

# Plugin-authoring SDK reference (`mise run plugin-sdk-docs`'s own build-time equivalent) — built
# fresh from this exact image's own copy of the source `.ts` files, never copied in from a
# developer's possibly-stale local `target/plugin-sdk-docs/` (gitignored, never committed). Same
# runtime dependency set as the `runtime` stage below since it's the identical `deno` binary, just
# used here to generate a static site instead of dispatching plugin calls.
# Phase 2 on-page manga translation (specs/004-ocr-manga-translation T004): fetches the
# Apache-2.0 `manga-ocr` recognition model in its own stage so the ~460MB download is cached
# independently of the Rust/frontend builds and never re-runs when only code changes. Detection
# models are not fetched here — `oar-ocr` manages those itself (research.md §1).
FROM debian:bookworm-slim AS ocr-model
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY scripts/fetch-ocr-model.sh ./
RUN chmod +x fetch-ocr-model.sh && ./fetch-ocr-model.sh /models/manga-ocr

# `lanrurugi-inpaint`'s LaMa model, same own-stage-for-caching reasoning as `ocr-model` above (a
# ~200MB download that shouldn't re-run on every code change). Missing this stage's output is not
# fatal at runtime — `TranslationRuntime::get()` degrades to the pre-inpainting flat-fill/outline-
# stroke compositing when the model directory isn't found — but it IS bundled into the default
# image here, same as the OCR model, so a fresh deployment gets the improved compositing without
# any extra manual step.
FROM debian:bookworm-slim AS inpaint-model
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY scripts/fetch-inpaint-model.sh ./
RUN chmod +x fetch-inpaint-model.sh && ./fetch-inpaint-model.sh /models/inpaint

# `lanrurugi-ocr::bubble_segment`'s YOLO11n-seg model — unlike `ocr-model`/`inpaint-model` above,
# this one isn't a plain download: the upstream repo only publishes Ultralytics `.pt` weights, so
# `fetch-bubble-seg-model.sh` converts to ONNX itself via a throwaway `uv`-managed Python venv (see
# that script's own docs on why, and on what's actually pinned/reproducible about this step versus
# a plain checksummed download). `uv`'s official installer needs no system Python-dev package.
# Missing this stage's output is not fatal at runtime, same as `inpaint-model` — `inpainter` (when
# itself available) just falls back to plain rectangular erasure instead of a bubble's real outline.
FROM debian:bookworm-slim AS bubble-seg-model
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:${PATH}"
WORKDIR /build
COPY scripts/fetch-bubble-seg-model.sh ./
RUN chmod +x fetch-bubble-seg-model.sh && ./fetch-bubble-seg-model.sh /models/bubble-seg

FROM debian:bookworm-slim AS docs-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libstdc++6 \
        libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=deno-bin /deno /usr/local/bin/deno
WORKDIR /build
COPY crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts crates/lanrurugi-plugin/dispatcher/dispatcher.ts ./
RUN deno doc --html --name="LANrurugi Plugin SDK" --output=/build/docs plugin-sdk.ts dispatcher.ts

FROM debian:bookworm-slim AS runtime
# No external `unrar`/`7z` needed: archive extraction (incl. RAR5/7z/LZH) is linked in statically
# via libarchive (see `crates/lanrurugi-scanner/src/archive_format.rs` + `Dockerfile.build`). This
# also drops the old `unrar-free`, whose getopt CLI never accepted the RARLAB `lb`/`p -inul` flags
# the previous shell-out used — RAR/CBR was silently broken in production with it.
#
# `redis-server`/`redis-cli` are bundled into this same image/container, matching legacy
# LANraragi's own real single-container deployment convention (verified against
# tools/Documentation/installing-lanraragi/docker.md: one `difegue/lanraragi` container, three
# bind mounts under `/home/koyomi/lanraragi/{content,thumb,database}` — no separate Redis
# container in their own documented `docker run`/`docker-compose.yml` examples) — not a
# split-into-two-services topology. Copied from the `redis-bin` stage above (not installed via
# `apt`) — see that stage's own doc comment for why (Debian bookworm's own package is a much
# older, on-disk-format-incompatible 7.0.15).
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        fonts-noto-cjk \
        libstdc++6 \
        zlib1g \
        libbz2-1.0 \
        liblzma5 \
        libzstd1 \
        liblz4-1 \
        libxml2 \
        libacl1 \
        libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=deno-bin /deno /usr/local/bin/deno
COPY --from=redis-bin /usr/local/bin/redis-server /usr/local/bin/redis-cli /usr/local/bin/
COPY --from=rust-builder /build/target/release/lanrurugi-server /usr/local/bin/lanrurugi-server

# MaxMind's own `geoipupdate` CLI — not in Debian bookworm's apt repos at all (confirmed: neither
# the base repo nor `bookworm-backports` carries it), so this installs the official `.deb` release
# directly from GitHub, the same "official upstream release artifact, not a distro repackage"
# pattern this Dockerfile already uses for `redis-server`/ONNX Runtime above. Pinned to a specific
# version (not "latest") for reproducible builds — bump deliberately, not silently on every
# rebuild. `docker-entrypoint.sh` runs this at container start (best-effort, skipped without
# LANRURUGI_GEOIP_ACCOUNT_ID/LANRURUGI_GEOIP_LICENSE_KEY) to fetch/refresh the GeoLite2-City
# database `lanrurugi_api::geoip` reads — installing the CLI here is required either way; the
# database itself is never baked into the image (MaxMind's license forbids redistributing it, and
# it needs a free MaxMind account's own credentials to fetch at all).
ARG GEOIPUPDATE_VERSION=8.0.0
RUN curl -fsSL -o /tmp/geoipupdate.deb \
        https://github.com/maxmind/geoipupdate/releases/download/v${GEOIPUPDATE_VERSION}/geoipupdate_${GEOIPUPDATE_VERSION}_linux_amd64.deb \
    && dpkg -i /tmp/geoipupdate.deb \
    && rm -f /tmp/geoipupdate.deb

# ONNX Runtime CPU (`libonnxruntime.so`) — `lanrurugi-recommend`'s embedding model loads it via
# `ort`'s `load-dynamic` feature (dlopen at runtime). Pinned to the same 1.26.0 version
# jellyfin-suite's frame-forge validates against; CPU flavor from the official GitHub release
# tarball. Extracted as the real `.so.1.26.0` file (the tarball's `libonnxruntime.so` is a
# symlink) with the two symlinks recreated, then `ldconfig` for the soname.
ARG ORT_VERSION=1.27.0
RUN curl -fsSL -o /tmp/ort.tgz https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-x64-${ORT_VERSION}.tgz \
    && tar xzf /tmp/ort.tgz -C /tmp onnxruntime-linux-x64-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION} \
    && cp /tmp/onnxruntime-linux-x64-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION} /usr/local/lib/ \
    && ln -sf libonnxruntime.so.${ORT_VERSION} /usr/local/lib/libonnxruntime.so.1 \
    && ln -sf libonnxruntime.so.1 /usr/local/lib/libonnxruntime.so \
    && ldconfig \
    && rm -rf /tmp/ort.tgz /tmp/onnxruntime-linux-x64-${ORT_VERSION}
# `manga-ocr` recognition model — matches `lanrurugi-ocr::model_discovery`'s container candidate
# path, so no env var is needed in the default image.
COPY --from=ocr-model /models/manga-ocr /app/models/manga-ocr
# LaMa inpainting model — matches `lanrurugi-inpaint::model_discovery`'s container candidate path.
COPY --from=inpaint-model /models/inpaint /app/models/inpaint
# Bubble segmentation model — matches `lanrurugi-ocr::bubble_segment::model_discovery`'s container
# candidate path.
COPY --from=bubble-seg-model /models/bubble-seg /app/models/bubble-seg
COPY --from=frontend-builder /build/apps/frontend/dist /usr/local/share/lanrurugi/frontend
# Bakes in whatever converted `.ts` plugins live in the repo's `./plugins/` at build time.
# `discover_namespaces` in `lanrurugi-api::plugins` recursively scans this directory (e.g.
# `plugins/metadata/ehentai.ts` → namespace-path `metadata/ehentai`), so category subfolders and
# the `custom/` upload destination are both picked up. This is the *default*; it's still a plain
# directory under `LANRURUGI_PLUGINS_DIR`, so a deployment can override it with
# `-v host/path:/plugins` + `-e LANRURUGI_PLUGINS_DIR=/plugins` to use a different set without
# rebuilding the image.
COPY plugins /usr/local/share/lanrurugi/plugins
# Plugin-authoring SDK reference, served at `/docs` (`build_app`'s own docs) — a reference for
# anyone writing/porting a plugin against this specific running instance, not needed by the
# instance itself to function (unlike the frontend/plugins above).
COPY --from=docs-builder /build/docs /usr/local/share/lanrurugi/docs

# Bundled Redis's own config (see redis.conf's doc comment) + the supervisor script that starts
# both processes in this one container (see scripts/docker-entrypoint.sh's own doc comment for why
# it's a plain trap-based wrapper rather than a real init system like legacy's s6-overlay).
COPY redis.conf /etc/lanrurugi/redis.conf
COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh \
    && mkdir -p /home/koyomi/lanrurugi/content /home/koyomi/lanrurugi/thumb /home/koyomi/lanrurugi/database \
    && mkdir -p /var/lib/GeoIP

ENV LANRURUGI_STATIC_DIR=/usr/local/share/lanrurugi/frontend
ENV LANRURUGI_PLUGINS_DIR=/usr/local/share/lanrurugi/plugins
ENV LANRURUGI_DOCS_DIR=/usr/local/share/lanrurugi/docs
# Legacy-mirrored paths (`/home/koyomi/lanraragi/{content,thumb}` with the project name swapped —
# see redis.conf's own `dir` for the `database` counterpart) — not this binary's own bare
# `./library`/`./thumb` CLI defaults (see crates/lanrurugi-server/src/main.rs), which are meant
# for non-Docker/bare-metal use where no such convention applies.
ENV LANRURUGI_LIBRARY_PATH=/home/koyomi/lanrurugi/content
ENV LANRURUGI_THUMB_DIR=/home/koyomi/lanrurugi/thumb
ENV LANRURUGI_REDIS_URL=redis://127.0.0.1:16379
EXPOSE 3000
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["serve", "--bind", "0.0.0.0:3000"]
