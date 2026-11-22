// Assembles the deployed docs site (#305): the latest version at the root, recent older versions as
// static trees under `/v<version>/`, and every older one still reachable through the Pages Function
// that reads its release zip from R2.
//
//   node scripts/assemble-versions.mjs --current <version> [--archived <v1,v2,...>]
//
// Input: `dist/` is the current version, built with base `/`; `dist/v<version>/` are the older
// versions kept as static files (unzipped from their releases); `--archived` lists the versions
// served from R2 instead. Output, in `dist/`:
// - `versions.json`, every version newest first, with the path it is served from;
// - `_redirects`: `/v<current>/…` → the same page at the root, so old links and the version picker
//   (which always targets `/v<version>/`) land on the canonical copy;
// - `robots.txt` with the sitemap the root build generated;
// - in every static older tree, the canonical link pointed at the same page at the root, or
//   `noindex` where the latest version has no such page — the old copies stay readable without
//   competing with the latest one in search results;
// - `_routes.json`, sending only the archived versions' paths to the Function.
import { existsSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'

const here = dirname(fileURLToPath(import.meta.url))
const dist = resolve(here, '../dist')
const site = (process.env.DOCS_SITE_URL ?? 'https://docs.lrrg.dev').replace(/\/$/, '')

const { values } = parseArgs({
  options: { current: { type: 'string' }, archived: { type: 'string', default: '' } },
})
const current = values.current
if (!current) {
  console.error('assemble-versions: --current <version> is required')
  process.exit(1)
}
const archived = values.archived.split(',').map((v) => v.trim().replace(/^v/, '')).filter(Boolean)

/** semver-ish ordering good enough for `1.2.3` and `1.2.3-beta.4`: numeric core, then prerelease last. */
function compare(left, right) {
  const parse = (value) => {
    const [core, pre] = value.split('-', 2)
    return { numbers: core.split('.').map((n) => Number.parseInt(n, 10) || 0), pre: pre ?? '' }
  }
  const a = parse(left)
  const b = parse(right)
  for (let i = 0; i < 3; i += 1) {
    const diff = (a.numbers[i] ?? 0) - (b.numbers[i] ?? 0)
    if (diff !== 0) return diff
  }
  // A prerelease sorts below the release it precedes.
  if (a.pre === b.pre) return 0
  if (a.pre === '') return 1
  if (b.pre === '') return -1
  return a.pre.localeCompare(b.pre, undefined, { numeric: true })
}

function readMeta(dir, version) {
  const fallback = { version, date: '', prerelease: version.includes('-') }
  try {
    return { ...fallback, ...JSON.parse(readFileSync(join(dir, 'meta.json'), 'utf8')), version }
  } catch {
    return fallback
  }
}

function htmlFiles(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...htmlFiles(path))
    else if (entry.name.endsWith('.html')) out.push(path)
  }
  return out
}

// The current version lives at the root; a leftover copy under its own path would only duplicate it.
rmSync(join(dist, `v${current}`), { recursive: true, force: true })

const staticVersions = readdirSync(dist, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && /^v\d+\.\d+\.\d+/.test(entry.name))
  .map((entry) => entry.name.slice(1))

const versions = [
  { ...readMeta(dist, current), path: '' },
  ...staticVersions.map((version) => ({ ...readMeta(join(dist, `v${version}`), version), path: `v${version}` })),
  ...archived
    .filter((version) => version !== current && !staticVersions.includes(version))
    .map((version) => ({ version, date: '', prerelease: version.includes('-'), path: `v${version}` })),
]
versions.sort((a, b) => compare(b.version, a.version))
writeFileSync(join(dist, 'versions.json'), `${JSON.stringify(versions, null, 2)}\n`)

// Older static trees: canonical to the root copy where the latest version has the page, else noindex.
let rewritten = 0
for (const version of staticVersions) {
  const tree = join(dist, `v${version}`)
  for (const file of htmlFiles(tree)) {
    const page = `/${relative(tree, file).replace(/\\/g, '/')}`.replace(/index\.html$/, '')
    const rootFile = join(dist, page, page.endsWith('/') ? 'index.html' : '')
    const latestHasIt = page.endsWith('/') && existsSync(rootFile) && statSync(rootFile).isFile()
    let html = readFileSync(file, 'utf8').replace(/<link\s+rel="canonical"[^>]*>/gi, '')
    const tag = latestHasIt
      ? `<link rel="canonical" href="${site}${page}">`
      : '<meta name="robots" content="noindex">'
    html = html.includes('</head>') ? html.replace('</head>', `${tag}</head>`) : tag + html
    writeFileSync(file, html)
    rewritten += 1
  }
}

// The root's own not-found page: served with a 404 status, and marked so it is never indexed either.
const notFoundPage = join(dist, '404.html')
if (existsSync(notFoundPage)) {
  const html = readFileSync(notFoundPage, 'utf8')
  if (!html.includes('name="robots"')) {
    writeFileSync(notFoundPage, html.replace('</head>', '<meta name="robots" content="noindex"></head>'))
  }
}

const redirectsPath = join(dist, '_redirects')
const existingRedirects = existsSync(redirectsPath) ? readFileSync(redirectsPath, 'utf8') : ''
writeFileSync(
  redirectsPath,
  `${existingRedirects}/v${current} / 301\n/v${current}/* /:splat 301\n`,
)

writeFileSync(
  join(dist, 'robots.txt'),
  `User-agent: *\nAllow: /\n\nSitemap: ${site}/sitemap-index.xml\n`,
)

// Pages requires at least one include rule; a path no page can have stands in when nothing is
// archived, so the Function is never invoked.
const routes = archived
  .filter((version) => !staticVersions.includes(version) && version !== current)
  .flatMap((version) => [`/v${version}`, `/v${version}/*`])
writeFileSync(
  join(dist, '_routes.json'),
  `${JSON.stringify({ version: 1, include: routes.length ? routes : ['/__no-archived-docs__'], exclude: [] }, null, 2)}\n`,
)

console.log(`root: v${current}`)
console.log(`static: ${staticVersions.join(', ') || '(none)'} — ${rewritten} page(s) pointed at the root copy`)
console.log(`archived (R2): ${routes.length / 2} version(s)`)
console.log(`versions.json: ${versions.length} version(s)`)
