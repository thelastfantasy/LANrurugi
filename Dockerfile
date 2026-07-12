# syntax=docker/dockerfile:1
#
# Multi-stage build per constitution Technology Stack Constraints: Debian bookworm-slim runtime
# (glibc, required by the official Deno distribution — Principle IV), a pinned-version Deno binary
# copied in (never `curl | sh`), fonts-noto-cjk bundled for correct CJK text rasterization, and a
# release build of the single `lanrurugi-server` binary serving the built frontend as static assets
# (Principle III — one deployable).

ARG RUST_VERSION=1.96.0
ARG NODE_VERSION=24.18.0
ARG DENO_VERSION=2.9.1
ARG PNPM_VERSION=11.10.0

FROM rust:${RUST_VERSION}-slim-bookworm AS rust-builder
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

# Plugin-authoring SDK reference (`mise run plugin-sdk-docs`'s own build-time equivalent) — built
# fresh from this exact image's own copy of the source `.ts` files, never copied in from a
# developer's possibly-stale local `target/plugin-sdk-docs/` (gitignored, never committed). Same
# runtime dependency set as the `runtime` stage below since it's the identical `deno` binary, just
# used here to generate a static site instead of dispatching plugin calls.
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
COPY --from=rust-builder /build/target/release/lanrurugi-server /usr/local/bin/lanrurugi-server
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

ENV LANRURUGI_STATIC_DIR=/usr/local/share/lanrurugi/frontend
ENV LANRURUGI_PLUGINS_DIR=/usr/local/share/lanrurugi/plugins
ENV LANRURUGI_DOCS_DIR=/usr/local/share/lanrurugi/docs
EXPOSE 3000
ENTRYPOINT ["lanrurugi-server"]
CMD ["serve", "--bind", "0.0.0.0:3000"]
