// Assembles the deployed docs site from every released version tree.
//
// Input: `docs/dist/v<version>/` directories — the current build already placed there, plus one
// directory per `docs-*.zip` downloaded from GitHub Releases. Output: the site-wide
// `docs/dist/versions.json`, and `docs/dist/index.html` redirecting to the newest stable release
// (or the newest prerelease while no stable exists).
//
// The manifest is derived from the directories that are actually present, so the header selector can
// never offer a version whose docs are missing.
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const dist = resolve(here, '../dist')

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

const versions = []
for (const entry of readdirSync(dist, { withFileTypes: true })) {
  if (!entry.isDirectory() || !entry.name.startsWith('v')) continue
  const dir = join(dist, entry.name)
  const metaPath = join(dir, 'meta.json')
  const fromName = entry.name.slice(1)
  let meta = { version: fromName, date: '', prerelease: fromName.includes('-') }
  if (existsSync(metaPath)) {
    try {
      meta = { ...meta, ...JSON.parse(readFileSync(metaPath, 'utf8')) }
    } catch {
      // A tree with an unreadable meta.json still belongs in the list; the directory name is enough.
    }
  }
  versions.push({ ...meta, path: entry.name })
}

versions.sort((a, b) => compare(b.version, a.version))
writeFileSync(join(dist, 'versions.json'), `${JSON.stringify(versions, null, 2)}\n`)

const preferred = versions.find((entry) => !entry.prerelease) ?? versions[0]
if (preferred) {
  writeFileSync(
    join(dist, 'index.html'),
    `<!doctype html><meta charset="utf-8"><title>LANrurugi docs</title>` +
      `<script>location.replace('/${preferred.path}/')</script>` +
      `<p><a href="/${preferred.path}/">LANrurugi docs</a></p>\n`,
  )
}

console.log(
  `versions.json: ${versions.length} version(s) → ${versions.map((v) => v.version).join(', ')}`,
)
console.log(`root → /${preferred?.path ?? ''}/`)
