import path from 'node:path'

import { defineConfig, type Plugin, type ProxyOptions } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

/** `http-proxy` (what Vite's own `server.proxy`/`preview.proxy` wrap) rewrites the outgoing
 * `Host` header to match the proxy *target* by default — fine for `/api/*` (nothing there reads
 * `Host`), but `GET /opensearch.xml` (issue #90) specifically builds its URL template from the
 * real browser-visible `Host`/`X-Forwarded-Host`, so without this the dev-mode-installed search
 * engine would point at the backend's own internal `127.0.0.1:3001` instead of whatever
 * `localhost:3000`/LAN address the user actually opened — confirmed live. Sets
 * `X-Forwarded-Host`/`X-Forwarded-Proto` from the *inbound* request (which still has the real
 * values at this point) — `resolve_base_url` already prefers those over the bare `Host` header,
 * matching this app's own documented reverse-proxy deployment convention, so dev mode now takes
 * exactly the same code path a real production reverse proxy would. */
function preserveOriginalHostHeader(): NonNullable<ProxyOptions['configure']> {
  return (proxy) => {
    proxy.on('proxyReq', (proxyReq, req) => {
      const host = req.headers.host
      if (host) proxyReq.setHeader('X-Forwarded-Host', host)
      proxyReq.setHeader('X-Forwarded-Proto', 'http')
    })
  }
}

/** Routes Vite dev's own HTML requests through the real Rust `serve_index` fallback first, so
 * dev and production use the same request-time `data-theme` injection (Redis → theme resolution →
 * `serve_index`). The Rust response is then passed through Vite's `transformIndexHtml` pipeline so
 * the normal dev-only HMR client / React refresh preamble still gets injected. This keeps the
 * production-safe injection logic in Rust only, without a second Vite-side theme-resolution
 * implementation. */
function rustHtmlMiddleware(): Plugin {
  return {
    name: 'rust-html-middleware',
    apply: 'serve',
    configureServer(server) {
      const backendPort = process.env.LANRURUGI_DEV_BACKEND_PORT ?? '3001'

      server.middlewares.use(async (req, res, next) => {
        if (req.method !== 'GET' && req.method !== 'HEAD') return next()

        const accept = req.headers.accept ?? ''
        if (!accept.includes('text/html') && !accept.includes('*/*')) return next()

        const originalUrl = req.originalUrl ?? req.url ?? '/'
        const url = new URL(originalUrl, 'http://localhost')
        // Only SPA HTML documents go to Rust; Vite keeps handling its own assets/modules/API proxy.
        if (
          url.pathname.startsWith('/api/') ||
          url.pathname.startsWith('/src/') ||
          url.pathname.startsWith('/@') ||
          url.pathname.startsWith('/node_modules/') ||
          url.pathname.startsWith('/legacy/') ||
          url.pathname === '/favicon.ico'
        ) {
          return next()
        }

        try {
          const headers = new Headers()
          if (req.headers.cookie) headers.set('cookie', String(req.headers.cookie))
          if (req.headers.authorization) headers.set('authorization', String(req.headers.authorization))

          const backendUrl = `http://127.0.0.1:${backendPort}${url.pathname}${url.search}`
          const response = await fetch(backendUrl, { headers })
          const contentType = response.headers.get('content-type') ?? ''
          if (!response.ok || !contentType.includes('text/html')) return next()

          const html = await response.text()
          const transformed = await server.transformIndexHtml(url.pathname, html, originalUrl)

          res.statusCode = response.status
          res.setHeader('content-type', contentType)
          res.end(transformed)
        } catch {
          // Backend unreachable or not serving HTML in this dev setup — fall through to Vite's
          // normal SPA behaviour, which still works (it just loses the pre-paint theme injection).
          next()
        }
      })
    },
  }
}

// https://vite.dev/config/
export default defineConfig({
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
    // Forces a single React/React DOM module instance across the whole dep graph — without this,
    // `@floating-ui/react` (added for `FilenameTemplateEditor.tsx`'s popover positioning) resolved
    // its own `react` copy separately from the app's own, and its internal `useId()` call threw
    // "Cannot read properties of null" (React's hook dispatcher was null — the classic symptom of
    // two React module instances coexisting, not two INSTALLED versions: `pnpm ls react` here only
    // ever shows the one 19.2.7 copy). Root-caused directly (not guessed) after clearing Vite's
    // `optimizeDeps` cache didn't fix it, ruling out a stale-prebundle explanation.
    dedupe: ['react', 'react-dom'],
  },
  plugins: [react(), tailwindcss(), rustHtmlMiddleware()],
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
      '/api': {
        target: `http://127.0.0.1:${process.env.LANRURUGI_DEV_BACKEND_PORT ?? '3001'}`,
        configure: preserveOriginalHostHeader(),
      },
      // `/opensearch.xml` (issue #90) lives outside `/api` on purpose — `lanrurugi_api::opensearch`'s
      // own docs explain why (must stay reachable before login, same as `/api/login` itself, but
      // mounted at a bare path since a browser's OpenSearch autodiscovery fetches exactly the
      // `href` `index.html`'s own `<link rel="search">` names, verbatim, no `/api` prefix to add).
      // Without this entry, `vite dev`'s own SPA fallback silently serves `index.html` for it
      // instead (confirmed live: Firefox's "could not install search engine" error was actually
      // Vite returning HTML with `Content-Type: text/html`, not the Rust backend's real XML —
      // nothing to do with login state, which this path never required in the first place).
      '/opensearch.xml': {
        target: `http://127.0.0.1:${process.env.LANRURUGI_DEV_BACKEND_PORT ?? '3001'}`,
        configure: preserveOriginalHostHeader(),
      },
    },
  },
  // `vite preview` (used by Playwright's per-worker e2e fixture, see tests/e2e/fixtures.ts) needs
  // its own proxy config — `server.proxy` above only applies to `vite dev`. Target port is
  // per-worker (spec FR-014's isolation — each worker gets its own backend process), read from an
  // env var the fixture sets before spawning `vite preview`, defaulting to the plain dev-mode
  // backend port so a bare `vite preview` outside the e2e harness still works sensibly.
  preview: {
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${process.env.LANRURUGI_E2E_BACKEND_PORT ?? '3000'}`,
        configure: preserveOriginalHostHeader(),
      },
      '/opensearch.xml': {
        target: `http://127.0.0.1:${process.env.LANRURUGI_E2E_BACKEND_PORT ?? '3000'}`,
        configure: preserveOriginalHostHeader(),
      },
    },
  },
})
