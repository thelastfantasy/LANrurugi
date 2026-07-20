import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 3000,
    proxy: {
      '/api': 'http://127.0.0.1:3001',
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
