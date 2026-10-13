#!/usr/bin/env node
// Copy the static assets this site serves but does not own, so that:
//
//   1. the branded icon stays identical to the one the product frontend serves, and
//   2. the target's absence fails the build here rather than shipping a site with a missing asset.
//
// Runs as part of both `dev` and `build` (see package.json), so local development, CI and the Docker
// image build share one definition of where these come from — and so the icon cannot drift from its
// real source, which a hand-copied file in `public/` eventually would.

import { copyFileSync, existsSync, mkdirSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const docsRoot = join(here, '..')
const repoRoot = join(docsRoot, '..')

let failed = false

function report(label, to) {
  const megabytes = (statSync(to).size / 1024 / 1024).toFixed(1)
  console.log(`prepare-static-assets: ${label} — copied ${relative(repoRoot, to)} (${megabytes}MB)`)
}

function fail(message) {
  console.error(`prepare-static-assets: ${message}`)
  failed = true
}

// ── Scalar's runtime: no longer published ───────────────────────────────────────────────────
//
// This script used to copy `@scalar/api-reference`'s UMD bundle here so the API page could render
// Scalar's full-page reference without a CDN. That page is gone: it rendered every operation inside
// Scalar's own application chrome, so the site had no per-group pages, no real heading anchors for
// its right-hand outline, and a sidebar listing internal module names. The API pages are now
// generated per group (`generate-api-pages.mjs`) with the request panel built in, so there is
// nothing left for a 4.2MB viewer bundle to do.

// ── 2. The favicon ──────────────────────────────────────────────────────────────────────────────

// Starlight references `/favicon.svg` by default. The product frontend's own icon is the right one to
// serve here — same project, same branding — and copying it from its real source means a change
// there reaches the docs instead of leaving a stale duplicate behind.
const faviconSource = join(repoRoot, 'apps', 'frontend', 'public', 'favicon.svg')
const faviconTarget = join(docsRoot, 'public', 'favicon.svg')
if (!existsSync(faviconSource)) {
  fail(
    `expected the product frontend's own icon at ${faviconSource}; it is this site's favicon's ` +
      `source of truth.`,
  )
} else {
  mkdirSync(dirname(faviconTarget), { recursive: true })
  copyFileSync(faviconSource, faviconTarget)
  report('favicon', faviconTarget)
}

process.exit(failed ? 2 : 0)
