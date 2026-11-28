#!/usr/bin/env node
// Search-engine indexing gate for the four sites published under lrrg.dev, run against the exact
// directories CI is about to deploy. Every rule here is a Search Console report this project has
// actually received: pages that only redirect, a sitemap or canonical naming a URL that redirects or
// does not exist, old docs versions competing with the latest one, and two pages that look identical
// to a crawler with nothing telling it which one is canonical.
//
//   node apps/docs-site/scripts/check-indexing.mjs
//
// Pass `--site name=<dir>=<url>` (repeatable) to check other directories; the defaults are the
// build outputs below. Exits 1 and lists every problem when a rule fails.
//
// Cloudflare Pages behaviour the link rules encode: `/a/` serves `a/index.html`; `/a` serves
// `a.html`, or 308-redirects to `/a/` when only the directory exists; `/a.html` 308-redirects to `/a`;
// `_redirects` rules fire before any file; `_routes.json` paths go to the Pages Function.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const DEFAULT_SITES = [
  `docs=${join(repo, 'apps/docs-site/dist')}=https://docs.lrrg.dev/`,
  `api=${join(repo, 'apps/docs-site/dist-swagger')}=https://api.lrrg.dev/`,
  `sdk=${join(repo, 'apps/docs-site/dist-sdk')}=https://sdk.lrrg.dev/`,
  `home=${join(repo, 'homepage/dist')}=https://lrrg.dev/`,
]
const { values } = parseArgs({ options: { site: { type: 'string', multiple: true } } })
const VERSION_DIR = /^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/

const sites = (values.site ?? DEFAULT_SITES).map((spec) => {
  const [name, dir, url] = spec.split('=')
  return { name, dir: resolve(dir), origin: new URL(url).origin }
})

const problems = new Map()
function report(site, rule, detail) {
  const key = `[${site.name}] ${rule}`
  if (!problems.has(key)) problems.set(key, [])
  problems.get(key).push(detail)
}

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    return entry.isDirectory() ? walk(path) : [path]
  })
}

const isFile = (path) => existsSync(path) && statSync(path).isFile()
const decode = (value) =>
  value.replace(/&#x2F;|&#47;/gi, '/').replace(/&amp;/g, '&').replace(/&quot;/g, '"').replace(/&#39;/g, "'")

/** The public path a file is served at, or null for files that are not pages. */
function pagePath(site, file) {
  const rel = relative(site.dir, file).split(sep).join('/')
  if (!rel.endsWith('.html')) return null
  if (rel === 'index.html') return '/'
  if (rel.endsWith('/index.html')) return `/${rel.slice(0, -'index.html'.length)}`
  return `/${rel.slice(0, -'.html'.length)}`
}

/** `_redirects` sources (`/a`, `/a/*`) and `_routes.json` includes, as matchers. */
function loadRules(site) {
  const toMatcher = (pattern) =>
    pattern.endsWith('/*')
      ? (path) => path.startsWith(pattern.slice(0, -1)) || path === pattern.slice(0, -2)
      : (path) => path === pattern
  const redirects = []
  const redirectsFile = join(site.dir, '_redirects')
  if (isFile(redirectsFile)) {
    for (const line of readFileSync(redirectsFile, 'utf8').split('\n')) {
      const source = line.trim().split(/\s+/)[0]
      if (source && !source.startsWith('#')) redirects.push(toMatcher(source))
    }
  }
  const functionRoutes = []
  const routesFile = join(site.dir, '_routes.json')
  if (isFile(routesFile)) {
    for (const pattern of JSON.parse(readFileSync(routesFile, 'utf8')).include ?? []) functionRoutes.push(toMatcher(pattern))
  }
  return { redirects, functionRoutes }
}

for (const site of sites) {
  if (!existsSync(site.dir)) {
    report(site, 'build output missing', site.dir)
    continue
  }
  site.rules = loadRules(site)
  site.pages = new Map()
  for (const file of walk(site.dir)) {
    const path = pagePath(site, file)
    if (path === null) continue
    const html = readFileSync(file, 'utf8')
    const top = path.split('/')[1] ?? ''
    site.pages.set(path, {
      file,
      html,
      old: VERSION_DIR.test(top),
      notFound: path === '/404',
      noindex: /<meta\s+name="robots"\s+content="[^"]*noindex/i.test(html),
      canonicals: [...html.matchAll(/<link\s+rel="canonical"\s+href="([^"]+)"/gi)].map((m) => decode(m[1])),
    })
  }
}

/** How Pages answers `url`: 'page' (200 with a page), 'file', 'function', 'redirect', 'missing', or null when off-site. */
function resolveUrl(url) {
  const site = sites.find((candidate) => candidate.origin === url.origin && candidate.pages)
  if (!site) return { kind: null }
  const path = decodeURIComponent(url.pathname)
  if (site.rules.redirects.some((match) => match(path))) return { site, kind: 'redirect' }
  if (site.rules.functionRoutes.some((match) => match(path))) return { site, kind: 'function' }
  if (path.endsWith('.html')) return { site, kind: 'redirect' }
  if (site.pages.has(path)) return { site, kind: 'page', page: site.pages.get(path) }
  if (!path.endsWith('/') && site.pages.has(`${path}/`)) return { site, kind: 'redirect' }
  if (isFile(join(site.dir, path))) return { site, kind: 'file' }
  return { site, kind: 'missing' }
}

function sitemapUrls(site) {
  const robots = join(site.dir, 'robots.txt')
  if (!isFile(robots)) {
    report(site, 'no robots.txt', '')
    return null
  }
  const declared = [...readFileSync(robots, 'utf8').matchAll(/^Sitemap:\s*(\S+)/gim)].map((m) => m[1])
  if (declared.length === 0) report(site, 'robots.txt names no sitemap', '')
  const urls = new Set()
  const pending = [...declared]
  while (pending.length) {
    const location = new URL(pending.pop())
    if (location.origin !== site.origin) {
      report(site, 'sitemap on another host', location.href)
      continue
    }
    const file = join(site.dir, decodeURIComponent(location.pathname))
    if (!isFile(file)) {
      report(site, 'sitemap file missing', location.href)
      continue
    }
    const xml = readFileSync(file, 'utf8')
    const locs = [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => decode(m[1]))
    if (xml.includes('<sitemapindex')) pending.push(...locs)
    else locs.forEach((loc) => urls.add(loc))
  }
  return urls
}

for (const site of sites.filter((candidate) => candidate.pages)) {
  const inSitemap = sitemapUrls(site)

  for (const [path, page] of site.pages) {
    const own = `${site.origin}${path}`
    if (page.notFound) {
      if (!page.noindex) report(site, '404 page without noindex', path)
      if (page.canonicals.length || /<link\b[^>]*\bhreflang=/i.test(page.html)) {
        report(site, '404 page names a canonical or language alternate', path)
      }
      continue
    }
    if (page.old) {
      if (!page.noindex && page.canonicals.every((canonical) => canonical === own)) {
        report(site, 'old version page competes with the latest (no noindex, self canonical)', path)
      }
      continue
    }
    if (page.noindex) {
      if (inSitemap?.has(own)) report(site, 'noindex page listed in the sitemap', path)
      continue
    }
    if (page.canonicals.length !== 1) report(site, `indexable page with ${page.canonicals.length} canonical links`, path)
    else if (page.canonicals[0] !== own) report(site, 'canonical is not the page itself', `${path} -> ${page.canonicals[0]}`)
    if (inSitemap && !inSitemap.has(own)) report(site, 'indexable page missing from the sitemap', path)

    const hrefs = [
      ...[...page.html.matchAll(/<a\b[^>]*?\bhref="([^"]+)"/gi)].map((m) => ['link', m[1]]),
      ...[...page.html.matchAll(/<link\b[^>]*?\bhreflang="[^"]*"[^>]*?\bhref="([^"]+)"/gi)].map((m) => ['hreflang', m[1]]),
    ]
    for (const [kind, raw] of hrefs) {
      const href = decode(raw)
      if (/^(mailto|javascript|data|tel):/i.test(href) || href.startsWith('#')) continue
      let target
      try {
        target = new URL(href, own)
      } catch {
        report(site, 'unparseable link', `${path} -> ${href}`)
        continue
      }
      target.hash = ''
      target.search = ''
      const answer = resolveUrl(target)
      if (answer.kind === 'redirect') report(site, `${kind} to a URL that redirects`, `${path} -> ${target.href}`)
      else if (answer.kind === 'missing') report(site, `${kind} to a URL that does not exist`, `${path} -> ${target.href}`)
      else if (kind === 'hreflang' && answer.kind === 'page' && answer.page.noindex) {
        report(site, 'hreflang to a noindex page', `${path} -> ${target.href}`)
      }
    }
  }

  for (const url of inSitemap ?? []) {
    const answer = resolveUrl(new URL(url))
    if (answer.kind !== 'page') report(site, `sitemap URL that is not a page (${answer.kind ?? 'off-site'})`, url)
  }

  // An old version's own sitemap lists exactly the URLs its canonicals point away from.
  for (const entry of readdirSync(site.dir, { withFileTypes: true })) {
    if (!entry.isDirectory() || !VERSION_DIR.test(entry.name)) continue
    for (const file of readdirSync(join(site.dir, entry.name))) {
      if (/^sitemap.*\.xml$/.test(file)) report(site, 'old version ships its own sitemap', `${entry.name}/${file}`)
    }
  }
}

const checked = sites.map((site) => `${site.name} ${site.pages?.size ?? 0} page(s)`).join(', ')
if (problems.size === 0) {
  console.log(`check-indexing: ok — ${checked}`)
  process.exit(0)
}
console.error(`check-indexing: ${problems.size} rule(s) failed — ${checked}`)
for (const [rule, details] of problems) {
  console.error(`\n${rule} (${details.length})`)
  for (const detail of details.slice(0, 20)) console.error(`  ${detail}`)
  if (details.length > 20) console.error(`  … ${details.length - 20} more`)
}
process.exit(1)
