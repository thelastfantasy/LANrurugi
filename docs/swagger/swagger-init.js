// Swagger UI bootstrap for the standalone API explorer.
//
// A separate file rather than a `<script>` written from the build script, because generating JS
// inside a JS template literal means every backslash survives two levels of escaping — a scheme
// regex shipped as a literal `///` and a path-normalising `\/+$` shipped as `//+$` (which turned the
// following line into a comment and broke the whole page). None of that can happen here.
//
// The one thing this adds over the stock page is a base-URL input. Swagger UI has none of its own:
// its top-bar box is the *spec* URL, and the Servers section is a read-only dropdown built from the
// document's own `servers`. With a single server declared, a reader had no way to point the page at
// their own instance — even though the server's own description told them to do exactly that.

// Set per page by the build script: the Chinese page renders `openapi.zh.json`, the English page
// `openapi.json`. The fallback keeps this file usable if it is ever opened without that script.
const LOCALE = window.__LRR_SWAGGER__ ?? { spec: './openapi.json', switcher: null }
const SPEC_URL = LOCALE.spec
const SERVER_KEY = 'lanrurugi-swagger:server'

const serverOverride = () => (localStorage.getItem(SERVER_KEY) || '').trim()

/**
 * Parse the reader's entry into an origin to send requests to, or `null` to leave requests on the
 * document's own server.
 *
 * Forgiving about a missing scheme on purpose: `192.168.1.10:3000` is what people type, and it does
 * not parse as a URL (the leading segment is read as a scheme). Everything else is rejected so a
 * typo falls back to the documented default instead of producing a request that cannot succeed.
 */
function parseOverride(value) {
  if (!value || /\s/.test(value)) return null
  const candidate = value.includes('://') ? value : `http://${value}`
  try {
    const url = new URL(candidate)
    if (!url.hostname) return null
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
    return url
  } catch {
    return null
  }
}

/** Rewrite one outgoing request to the override's origin, preserving its own path and query. */
function rewriteToOverride(url) {
  const origin = parseOverride(serverOverride())
  if (!origin) return url
  try {
    const target = new URL(url, window.location.href)
    // protocol/hostname/port separately, never `host`: assigning `host` from an origin that carries
    // no port leaves the OLD port in place (an entry of `https://lrr.example.com` produced
    // `https://lrr.example.com:3000/...`). Assigning `port` clears it when the entry has none.
    target.protocol = origin.protocol
    target.hostname = origin.hostname
    target.port = origin.port
    // Keep a path prefix the reader typed: an instance behind a reverse proxy can live at
    // `https://host/lrr`, and copying only the origin would send requests to `https://host/api/...`
    // and 404. The document's own server has no prefix, so this is a no-op in the common case.
    const prefix = origin.pathname.replace(/\/+$/, '')
    if (prefix) target.pathname = prefix + target.pathname
    return target.toString()
  } catch {
    // A half-typed address must never break a request.
    return url
  }
}

/**
 * Inject the base-URL input above Swagger UI's own Servers block, once.
 *
 * Inserted *into* the element that owns the Servers title rather than prepended to the page's
 * scheme container. The container indents its contents (the title and the select both start 83px in,
 * at a narrower width than the container itself) and an element dropped in beside it inherits none of
 * that — the first version sat hard against the left edge, visibly out of line with the Servers block
 * directly beneath it. Inheriting the real container gets the offset, the width and the typography
 * for free instead of copying pixel values that a Swagger UI upgrade would invalidate.
 *
 * Returns false while the block is not on the page yet, so the caller can keep polling.
 */
function installServerInput() {
  // `.servers-title` is the structural anchor; `.scheme-container` is the fallback, used only if a
  // future Swagger UI renames it. Position degrades but the control still reaches the reader.
  const title = document.querySelector('.servers-title')
  const host = title?.parentElement ?? document.querySelector('.scheme-container')
  if (!host || host.querySelector('#server-override')) return false

  const wrapper = document.createElement('div')
  wrapper.className = 'server-override'
  wrapper.innerHTML =
    '<label for="server-override" class="server-override__title">服务器地址（覆盖下面的 Servers）</label>' +
    '<input id="server-override" type="text" spellcheck="false" placeholder="http://192.168.1.10:3000" />' +
    '<span class="server-override__hint">留空则用文档里的默认值。请求由浏览器直接发往该地址，不经过本项目；' +
    '跨域与 HTTPS 的限制见文档站的「用文档站测试 API」。</span>'

  const input = wrapper.querySelector('input')
  input.value = serverOverride()
  input.addEventListener('input', () => localStorage.setItem(SERVER_KEY, input.value))
  // Enter should feel like it did something even though every request already reads the value live.
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') input.blur()
  })

  host.prepend(wrapper)
  return true
}

/**
 * Path-aware filtering, replacing Swagger UI's tag-only filter.
 *
 * Its built-in is `opsFilter(s, o) { return s.filter((s, i) => -1 !== i.indexOf(o)) }` — `i` is the
 * *tag* name, so the box only ever matches tags. Reported live: typing `/api/search` answered "No
 * operations defined", and a reader who knows an endpoint's path but not which module it lives in
 * (37 tags, 231 operations, virtualised list) has no way to reach it.
 *
 * The upstream selector hands this an OrderedMap of `tag -> Map{ tagDetails, operations: List }`,
 * so the replacement can also narrow *within* a tag. A tag whose own name matches keeps all of its
 * operations, exactly like the built-in; otherwise only the operations that match survive, and an
 * emptied tag disappears.
 */
function operationMatches(operation, needle) {
  const fields = [
    operation.get('path'),
    operation.getIn(['operation', 'summary']),
    operation.getIn(['operation', 'operationId']),
  ]
  return fields.some((field) => String(field ?? '').toLowerCase().includes(needle))
}

const PathAwareFilter = () => ({
  fn: {
    opsFilter(taggedOperations, phrase) {
      const needle = String(phrase ?? '').trim().toLowerCase()
      if (!needle) return taggedOperations
      return taggedOperations
        .map((entry, tag) => {
          const operations = entry.get('operations')
          if (!operations || typeof operations.filter !== 'function') return entry
          if (String(tag).toLowerCase().includes(needle)) return entry
          return entry.set('operations', operations.filter((op) => operationMatches(op, needle)))
        })
        .filter((entry) => (entry.get('operations')?.size ?? 0) > 0)
    },
  },
})

/** Correct the filter box's hardcoded placeholder — it says "Filter by tag", which this build's
 *  `PathAwareFilter` no longer is, and that mislabelling is why it went unnoticed as the page's
 *  actual search. */
function relabelFilter() {
  const box = document.querySelector('.operation-filter-input')
  if (!box) return false
  if (box.placeholder !== '按 tag、路径或摘要过滤') {
    box.placeholder = '按 tag、路径或摘要过滤'
  }
  return true
}

/**
 * A link to the other locale, in the top bar.
 *
 * Swagger UI has no notion of locales, so this is added the same way as the base-URL input: the two
 * pages are separate static files and this link is the only thing tying them together.
 */
function installLocaleSwitcher() {
  if (!LOCALE.switcher) return true
  const wrapper = document.querySelector('.topbar-wrapper')
  if (!wrapper || wrapper.querySelector('.locale-switcher')) return false
  const link = document.createElement('a')
  link.className = 'locale-switcher'
  link.href = LOCALE.switcher.href
  link.textContent = LOCALE.switcher.label
  wrapper.appendChild(link)
  return true
}

window.addEventListener('load', () => {
  window.ui = SwaggerUIBundle({
    url: SPEC_URL,
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    plugins: [SwaggerUIBundle.plugins.DownloadUrl, PathAwareFilter],
    layout: 'StandaloneLayout',
    deepLinking: true,
    // Without this there is no filter box, and this document has 231 operations across 37 tags —
    // reaching a known endpoint means scrolling a virtualised list blind. Reported live: "/api/search
    // 里面怎么没找到" — it was in the document the whole time, with no way to look for it.
    filter: true,
    // Surface the `x-lanrurugi-*` fields. They carry things OpenAPI's own model cannot express —
    // which operations an *administrator* token is required for (117 of them), which are reachable
    // without credentials only in guest mode, which have no documented shapes yet, and where each
    // route is registered. Off by default, which meant all five were invisible in the UI.
    showExtensions: true,
    docExpansion: 'list',
    defaultModelsExpandDepth: 1,
    defaultModelExpandDepth: 2,
    displayRequestDuration: true,
    persistAuthorization: true,
    tryItOutEnabled: true,
    // No request to validator.swagger.io: this page is also served from the server image for
    // offline NAS reading, and a spec this size should not be shipped to a third party to be
    // validated on every load.
    validatorUrl: null,
    requestInterceptor: (req) => {
      req.url = rewriteToOverride(req.url)
      return req
    },
  })

  // Swagger UI renders asynchronously; poll briefly rather than guessing a delay.
  let tries = 0
  const timer = setInterval(() => {
    const done = installServerInput() && relabelFilter() && installLocaleSwitcher()
    if (done) {
      clearInterval(timer)
      return
    }
    if (++tries > 40) {
      clearInterval(timer)
      // Loud on purpose. This is the one part of the page that depends on Swagger UI's own DOM, so
      // a future upgrade that renames those hooks would otherwise remove the reader's only way to
      // point the page at their instance — silently, and with nothing to notice but a missing box.
      console.warn(
        'swagger-init: could not find every Swagger UI hook it installs into ' +
          '(.servers-title / .scheme-container / .operation-filter-input), so the base-URL input ' +
          'or the filter relabel is missing. Update this file for the new markup.',
      )
    }
  }, 250)
})
