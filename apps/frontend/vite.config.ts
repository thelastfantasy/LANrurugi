import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 3000,
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
