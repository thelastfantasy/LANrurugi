import path from 'node:path'

import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 3000,
    fs: {
      // Explicit monorepo-root allow — Vite's own default allow-list only ever widens to a
      // detected workspace root via its own project-root/`package.json`-workspaces heuristics,
      // which don't recognize this repo's plain `pnpm-workspace.yaml`-based layout (confirmed
      // live: the default list here was just `apps/frontend` + Vite's own client dist dir,
      // nothing wider, even with `pnpm-workspace.yaml` present at the repo root). Without this,
      // any asset resolved through the *root* `node_modules/.pnpm/...` — not `apps/frontend`'s
      // own — 403s, which is exactly how pnpm hoists/symlinks real package contents in a
      // workspace; this silently broke every Font Awesome icon in the app (its webfont files
      // live there), degrading them all to plain underlined text with no visible error beyond a
      // console 403.
      allow: [path.resolve(__dirname, '../..')],
    },
    proxy: {
      // Target port is overridable via env var (mirrors `preview.proxy` below) — the plain
      // `cargo run`/bare-metal dev backend defaults to 3001, but the same override lets `vite
      // dev` point at a `compose.yaml`-run container instead (which always binds 3000 itself,
      // per its own `network_mode: host` + `LANRURUGI_BIND` default — meaning `vite dev` can't
      // also bind 3000 in that case; run it with `--port <other>` alongside this override).
      '/api': `http://127.0.0.1:${process.env.LANRURUGI_DEV_BACKEND_PORT ?? '3001'}`,
    },
  },
  // `vite preview` (used by Playwright's per-worker e2e fixture, see tests/e2e/fixtures.ts) needs
  // its own proxy config — `server.proxy` above only applies to `vite dev`. Target port is
  // per-worker (spec FR-014's isolation — each worker gets its own backend process), read from an
  // env var the fixture sets before spawning `vite preview`, defaulting to the plain dev-mode
  // backend port so a bare `vite preview` outside the e2e harness still works sensibly.
  preview: {
    proxy: {
      '/api': `http://127.0.0.1:${process.env.LANRURUGI_E2E_BACKEND_PORT ?? '3000'}`,
    },
  },
})
