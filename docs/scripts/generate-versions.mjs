// Builds `public/versions.json`, the single source both the header selector and the version-history
// page read. One entry per release tag, newest first.
//
// The tags — not `Cargo.toml` — are the list: the selector exists to reach the docs *of a release*,
// and every release is exactly one `v*` tag. The workspace manifest only supplies the answer for a
// tree that has no tags yet (a fresh checkout, or the very first build before the initial release),
// so the selector is never empty.
import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '../..')
const output = resolve(here, '../public/versions.json')

function git(args) {
  return execFileSync('git', args, { cwd: repoRoot, encoding: 'utf8' }).trim()
}

function currentVersion() {
  try {
    const manifest = readFileSync(resolve(repoRoot, 'Cargo.toml'), 'utf8')
    return /^version = "(.*)"/m.exec(manifest)?.[1] ?? null
  } catch {
    return null
  }
}

let tags = []
try {
  tags = git(['tag', '--list', 'v*', '--sort=-v:refname']).split('\n').filter(Boolean)
} catch {
  // Not a git checkout (a packaged copy of the repo): fall through to the manifest answer.
}

const versions = tags.map((tag) => {
  let date = ''
  try {
    date = git(['log', '-1', '--format=%cs', tag])
  } catch {
    // A tag without a reachable commit still belongs in the list; the date is decoration.
  }
  return { version: tag.replace(/^v/, ''), tag, date, prerelease: tag.replace(/^v/, '').includes('-') }
})

if (versions.length === 0) {
  const version = currentVersion() ?? 'dev'
  versions.push({
    version,
    tag: `v${version}`,
    date: new Date().toISOString().slice(0, 10),
    prerelease: version.includes('-'),
  })
}

writeFileSync(output, `${JSON.stringify(versions, null, 2)}\n`)
console.log(`versions.json: ${versions.length} version(s) → ${versions.map((v) => v.version).join(', ')}`)
