#!/usr/bin/env node
// Normalise `deno doc --json` output so no absolute filesystem path from the generating machine
// reaches a published artefact.
//
// Why this is needed at all: the JSON form of `deno doc` keys its `nodes` map on `file://` URLs and
// embeds those URLs inside module documentation. Confirmed live — a run over this repository's SDK
// sources produced keys like `file:///home/<user>/LANrurugi/crates/...`. The HTML form does *not*
// have this problem (verified by grepping the generated tree for the workspace path: zero hits),
// which is why only the JSON goes through here.
//
// Rewrites are limited to stripping a `file://<root>` prefix down to a repository-relative path.
// Keys are also sorted, so the artefact does not churn between runs.
//
// Usage: deno doc --json <files...> | node scripts/normalize-sdk-doc-json.mjs <repo-root> > out.json

import { readFileSync } from 'node:fs'
import { relative, resolve, sep } from 'node:path'

const root = resolve(process.argv[2] ?? process.cwd())

const input = readFileSync(0, 'utf8')
let document
try {
  document = JSON.parse(input)
} catch (error) {
  console.error(`normalize-sdk-doc-json: input is not valid JSON: ${error.message}`)
  process.exit(2)
}

/** `file:///abs/path` -> `<repo-relative path>`, leaving anything outside the repo untouched. */
function relativise(value) {
  if (typeof value !== 'string') return value
  const marker = 'file://'
  const at = value.indexOf(marker)
  if (at === -1) return value
  const url = value.slice(at + marker.length)
  let decoded
  try {
    decoded = decodeURIComponent(url)
  } catch {
    decoded = url
  }
  const absolute = resolve(sep === '/' ? `/${decoded.replace(/^\/+/, '')}` : decoded)
  const relativePath = relative(root, absolute)
  if (relativePath.startsWith('..')) return value
  return value.slice(0, at) + relativePath.split(sep).join('/')
}

/** Recursively relativise every string and sort every object's keys. */
function walk(node) {
  if (Array.isArray(node)) return node.map(walk)
  if (node && typeof node === 'object') {
    const out = {}
    for (const key of Object.keys(node).sort()) {
      out[relativise(key)] = walk(node[key])
    }
    return out
  }
  return relativise(node)
}

process.stdout.write(`${JSON.stringify(walk(document), null, 2)}\n`)
