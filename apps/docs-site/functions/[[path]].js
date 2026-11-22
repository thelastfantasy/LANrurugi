// Serves documentation versions that are no longer part of the static deployment, straight out of
// their release zip in R2 (#305). Only the paths `_routes.json` lists reach this function — one
// `/v<version>/*` rule per archived version, written by `scripts/assemble-versions.mjs` — so the
// latest docs (at the site root) and the few recent versions kept as static files never pay for it.
//
// Old versions stay readable at their own URLs but are not what search engines should index: every
// HTML page served from here carries `noindex` and a canonical link to the same page at the root.

import { readEntry } from './_lib/zip.js'

const VERSION_PATH = /^\/v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(\/.*)?$/

const TYPES = {
  html: 'text/html; charset=utf-8',
  css: 'text/css; charset=utf-8',
  js: 'text/javascript; charset=utf-8',
  mjs: 'text/javascript; charset=utf-8',
  json: 'application/json; charset=utf-8',
  xml: 'application/xml; charset=utf-8',
  txt: 'text/plain; charset=utf-8',
  svg: 'image/svg+xml',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  ico: 'image/x-icon',
  woff: 'font/woff',
  woff2: 'font/woff2',
  wasm: 'application/wasm',
  pf_meta: 'application/octet-stream',
}

function contentType(path) {
  const ext = path.slice(path.lastIndexOf('.') + 1).toLowerCase()
  return TYPES[ext] ?? 'application/octet-stream'
}

/** The page's own canonical pointed at the root (latest) copy, and `noindex` added. */
export function markArchived(html, origin, rest) {
  const canonical = `${origin}${rest}`
  const withoutCanonical = html.replace(/<link\s+rel="canonical"[^>]*>/gi, '')
  const tags = `<meta name="robots" content="noindex"><link rel="canonical" href="${canonical}">`
  return withoutCanonical.includes('</head>')
    ? withoutCanonical.replace('</head>', `${tags}</head>`)
    : tags + withoutCanonical
}

async function notFound(context) {
  const page = await context.env.ASSETS.fetch(new URL('/404.html', context.request.url))
  return new Response(page.body, { status: 404, headers: page.headers })
}

export async function onRequestGet(context) {
  const url = new URL(context.request.url)
  const match = VERSION_PATH.exec(url.pathname)
  const bucket = context.env.DOCS_VERSIONS
  if (!match || !bucket) return context.next()
  const [, version, rest = ''] = match
  if (rest === '') return Response.redirect(`${url.origin}/v${version}/`, 301)

  const cache = caches.default
  const cached = await cache.match(context.request)
  if (cached) return cached

  const key = `${version}.zip`
  let path = rest.slice(1)
  if (path === '' || path.endsWith('/')) path += 'index.html'
  let bytes = await readEntry(bucket, key, path)
  if (!bytes && !path.includes('.', path.lastIndexOf('/') + 1)) {
    // `/guide/sharing` without its trailing slash: the site's pages are directories.
    if (await readEntry(bucket, key, `${path}/index.html`)) {
      return Response.redirect(`${url.origin}/v${version}${rest}/`, 301)
    }
  }
  if (!bytes) return notFound(context)

  const type = contentType(path)
  let body = bytes
  if (type.startsWith('text/html')) {
    const page = rest.endsWith('/') ? rest : rest.replace(/index\.html$/, '')
    body = markArchived(new TextDecoder().decode(bytes), url.origin, page)
  }
  const response = new Response(body, {
    headers: {
      'content-type': type,
      // A released version's files never change.
      'cache-control': 'public, max-age=86400, s-maxage=31536000, immutable',
      ...(type.startsWith('text/html') ? { 'x-robots-tag': 'noindex' } : {}),
    },
  })
  context.waitUntil(cache.put(context.request, response.clone()))
  return response
}
