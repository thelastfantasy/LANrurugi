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

/** Fills in `index.html`'s inline anti-flash-of-default-theme script's `id="theme-init"
 * data-theme="..."` attribute (see that file's own docs) with the real current theme in `vite
 * dev` too, not just in a production build served by `lanrurugi-server::app::serve_index`. Uses
 * the `transformIndexHtml` hook — Vite's own dedicated, documented mechanism for this exact kind
 * of "adjust the HTML Vite is about to serve" task — rather than intercepting the raw HTTP
 * request/response in `configureServer` middleware: an earlier version of this fetched the
 * *entire* `index.html` from the Rust backend's own `/` response and used that as the template,
 * which seemed to work (the attribute really did come back filled in) but was actually fetching
 * `lanrurugi-server`'s production-mode response — built from `dist/index.html`, which references
 * bundled assets (`/assets/index-XXXX.js`) instead of the real dev-mode source entry
 * (`/src/main.tsx`) `vite dev` itself needs — silently producing a blank page (a 404 on the
 * bundled script vite dev never serves) confirmed live via the browser console. Only fetching the
 * *theme value itself* here (the same `GET /api/theme` the production frontend's own
 * `usePublicTheme()`/`useSettings()` already call) and substituting it into Vite's own
 * already-correct dev-mode HTML sidesteps that whole class of bug — nothing about vite dev's own
 * asset resolution is touched. */
function injectServerTheme(): Plugin {
  return {
    name: 'inject-server-theme',
    // This hook exists for the Vite dev server's live index.html transform only. Production is
    // served by lanrurugi-server's `serve_index`, which fills `data-theme` at request time from
    // Redis; running this during `vite build` would bake whatever local backend happened to be
    // running into `dist/index.html` and prevent request-time substitution.
    apply: 'serve',
    async transformIndexHtml(html, ctx) {
      const backendPort = process.env.LANRURUGI_DEV_BACKEND_PORT ?? '3001'
      try {
        const response = await fetch(`http://127.0.0.1:${backendPort}/api/theme`)
        if (!response.ok) return html
        const data = (await response.json()) as { theme?: string; admin_theme?: string }
        // The Login page is an admin surface; keep the initial paint on the admin's theme even
        // when guest mode is on and the guest theme differs. `ctx.path` is the rewritten internal
        // path (`/index.html` for SPA fallback); `originalUrl` is the actual browser route.
        const requestPath = (ctx.originalUrl ?? ctx.path).split('?')[0]
        const theme =
          requestPath === '/login' || requestPath.startsWith('/login/')
            ? (data.admin_theme ?? data.theme)
            : data.theme
        if (!theme) return html
        return html.replace('id="theme-init" data-theme=""', `id="theme-init" data-theme="${theme}"`)
      } catch {
        // Backend unreachable (not started yet, crashed, etc.) — leave the placeholder empty, same
        // as a production `serve_index` that couldn't reach Redis; the script's own client-side
        // fallback chain (`localStorage` then `modern.css`) still applies.
        return html
      }
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
  plugins: [react(), tailwindcss(), injectServerTheme()],
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
