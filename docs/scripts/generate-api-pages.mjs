#!/usr/bin/env node
// Turn the generated OpenAPI document into one Starlight content page per API group.
//
// Why generate pages rather than render the whole spec with a single embedded viewer: the layout
// this repo is reproducing renders each operation as *page content* — a real `<h3 id="get-search">`
// heading, then its method/path, description, parameters and response inline — with the site's own
// left sidebar and right-hand "on this page" outline around it, and a small request panel for
// trying the call. An embedded viewer renders all of that inside its own application chrome
// instead, which is a visibly different product: no per-group pages to navigate, no site outline,
// and headings that exist only after client-side JS runs.
//
// Real headings are the point, not a detail: Starlight builds the right-hand outline from the
// markdown heading tree at build time, so an operation title rendered by a component is invisible
// to it. Each operation therefore gets a literal `## Title {#anchor}` and everything else hangs off
// a component below it.
//
// Runs before `astro build` (see the docs package's `build` script), so CI, local builds and the
// Docker image all generate from the same document at the same revision.

import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  hasOperationDescription,
  hasParameterEntry,
  hasResponseEntry,
  localizeOperation,
  localizeParameter,
  localizeResponse,
} from '../src/lib/i18n-core.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const docsRoot = join(here, '..')
const repoRoot = join(docsRoot, '..')

const specPath = join(docsRoot, 'public', 'openapi.json')
const tagNamesPath = join(docsRoot, 'api', 'tag-names.json')
const generatedDir = join(docsRoot, 'src', 'generated')
const contentDir = join(docsRoot, 'src', 'content', 'docs')

// Locales the site is built for. `root` is the default-locale content directory that Starlight
// serves without a path prefix; every other key is both a `locales` entry in astro.config.mjs and
// a subdirectory here.
const LOCALES = [
  { key: 'zh', dir: '', label: '简体中文' },
  { key: 'en', dir: 'en', label: 'English' },
]

// The document's own language. Every other locale gets its chrome from `labels.mjs` and its body
// text from an overlay under `docs/api/i18n/`.
const SOURCE_LOCALE = 'en'

if (!existsSync(specPath)) {
  console.error(
    `generate-api-pages: ${relative(repoRoot, specPath)} not found — run \`mise run docs-api\` first.`,
  )
  process.exit(2)
}

const spec = JSON.parse(readFileSync(specPath, 'utf8'))

// Translation overlays, one file per non-source locale. Read (and validated) here rather than
// imported by the components, so a malformed or missing overlay fails the build naming the file —
// rather than silently rendering English on a Chinese page, which is what a missing key used to do.
const overlays = {}
for (const locale of LOCALES) {
  if (locale.key === SOURCE_LOCALE) continue
  const path = join(docsRoot, 'api', 'i18n', `${locale.key}.json`)
  if (!existsSync(path)) {
    console.error(
      `generate-api-pages: locale "${locale.key}" has no translation overlay at ` +
        `${relative(repoRoot, path)}. Add one, or drop the locale from LOCALES.`,
    )
    process.exit(2)
  }
  try {
    overlays[locale.key] = JSON.parse(readFileSync(path, 'utf8'))
  } catch (error) {
    console.error(`generate-api-pages: ${relative(repoRoot, path)} is not valid JSON — ${error}`)
    process.exit(2)
  }
}

const tagNames = JSON.parse(readFileSync(tagNamesPath, 'utf8'))
const names = new Map(tagNames.map((entry) => [entry.key, entry]))

/**
 * Make a document's prose safe to place in MDX without losing its markdown.
 *
 * Three hazards, all measured against the real document rather than guessed at:
 *
 * - **A bare `---` line.** 83 descriptions carry one (the generator's own "not yet described"
 *   separator). In markdown a `---` immediately under a text line turns *that line* into a setext
 *   H2 — which would silently restructure the page and corrupt the right-hand outline. `***` is
 *   the same thematic break without the ambiguity.
 * - **`<` and `{`/`}`.** MDX reads both as syntax (JSX and expressions). Escaping applies only
 *   *outside* inline code, because backslash escapes are inert inside a code span — blanket
 *   escaping would render `\{` literally in the 136 descriptions that quote an identifier.
 * - **A leading `#`.** Would become a heading; escaped for the same reason as the rest.
 */
function mdxSafe(text) {
  if (!text) return ''
  // Odd indices are inside backticks, which MDX already treats as literal.
  const parts = String(text).split(/(`[^`]*`)/)
  return parts
    .map((part, index) => {
      if (index % 2 === 1) return part
      return part
        .replace(/^---+$/gm, '***')
        .replace(/^#{1,6} /gm, (match) => `\\${match}`)
        .replace(/</g, '\\<')
        .replace(/\{/g, '\\{')
        .replace(/\}/g, '\\}')
    })
    .join('')
}

/** One operation's own fields, for inlining its description as page prose. */
function findOperationFields(operationId) {
  for (const item of Object.values(spec.paths ?? {})) {
    for (const method of METHODS) {
      const op = item[method]
      if (op?.operationId === operationId) return op
    }
  }
  return {}
}

/** `api_tokens` -> `api-tokens`, for a URL-friendly page slug. */
const slugify = (key) => key.replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '').toLowerCase()

/** `GET /api/search` -> `get-search`; also the operation's stable `operationId`-independent key. */
const anchorFor = (method, path) =>
  `${method}-${path.replace(/^\/+/, '').replace(/[^a-z0-9]+/gi, '-').replace(/-+$/, '')}`.toLowerCase()

const METHODS = ['get', 'post', 'put', 'delete', 'patch', 'head', 'options']

// Group by each operation's first tag, and collect any tag an operation carries that the naming
// file does not know about — an unnamed group would render as a raw module name, which is the
// thing this file exists to avoid.
const groups = new Map()
const unknownTags = new Set()
for (const [path, item] of Object.entries(spec.paths ?? {})) {
  for (const method of METHODS) {
    const op = item[method]
    if (!op) continue
    const tag = (op.tags ?? [])[0] ?? 'misc'
    if (!names.has(tag)) unknownTags.add(tag)
    if (!groups.has(tag)) groups.set(tag, [])
    groups.get(tag).push({
      method: method.toUpperCase(),
      path,
      anchor: anchorFor(method, path),
      operationId: op.operationId,
      summary: op.summary ?? `${method.toUpperCase()} ${path}`,
    })
  }
}

if (unknownTags.size > 0) {
  console.error(
    `generate-api-pages: ${[...unknownTags].sort().join(', ')} appear as API groups but have no ` +
      `entry in ${relative(repoRoot, tagNamesPath)}. Add one (a raw module name as a sidebar ` +
      `heading is exactly what that file exists to prevent).`,
  )
  process.exit(2)
}

// Operation titles double as the page's headings, and Starlight derives each heading's anchor from
// its text — so two operations in one group sharing a title would give the outline two entries
// pointing at the same place, silently. Checked here because it is a data condition that only
// appears when the document changes.
for (const [key, operations] of groups) {
  const seen = new Set()
  for (const op of operations) {
    if (seen.has(op.summary)) {
      console.error(
        `generate-api-pages: two operations in group "${key}" both have the title ` +
          `${JSON.stringify(op.summary)}. Their headings would share an anchor; give one of them a ` +
          `distinct summary (a sidecar entry is the place).`,
      )
      process.exit(2)
    }
    seen.add(op.summary)
  }
}

for (const operations of groups.values()) {
  operations.sort((a, b) => a.path.localeCompare(b.path) || a.method.localeCompare(b.method))
}

// Display order: the naming file's own order first (it mirrors the reference site's sidebar, which
// in turn mirrors the upstream document's tag order), then anything else alphabetically. A tag with
// no operations is dropped rather than rendered as an empty page.
const orderedKeys = [
  ...tagNames.map((entry) => entry.key).filter((key) => groups.has(key)),
  ...[...groups.keys()].filter((key) => !names.has(key)).sort(),
]

// Stale pages from a previous run must go, or a renamed group leaves its old page behind and
// Starlight happily builds both.
for (const locale of LOCALES) {
  const dir = join(contentDir, locale.dir, 'api')
  if (existsSync(dir)) rmSync(dir, { recursive: true, force: true })
  mkdirSync(dir, { recursive: true })
}
mkdirSync(generatedDir, { recursive: true })

const nav = []
for (const key of orderedKeys) {
  const entry = names.get(key)
  const slug = slugify(key)
  const operations = groups.get(key)
  nav.push({
    key,
    slug,
    zh: entry.zh,
    en: entry.en,
    operationCount: operations.length,
    operations: operations.map((op) => ({ anchor: op.anchor, summary: op.summary, method: op.method, path: op.path })),
  })

  for (const locale of LOCALES) {
    const title = locale.key === 'zh' ? entry.zh : entry.en
    const description =
      (locale.key === 'zh' ? entry.description_zh : entry.description_en) ??
      `${operations.length} endpoint${operations.length === 1 ? '' : 's'}.`

    // Relative path from the page back to `src/components/`. The page lives at
    // `src/content/docs[/<locale>]/api/`, so the root locale needs three levels and a locale
    // directory one more.
    const depth = locale.dir ? 4 : 3
    const components = `${'../'.repeat(depth)}components`

    const body = operations
      .map((op) => {
        const full = findOperationFields(op.operationId)
        // Body text is localised here, at render time, from an overlay keyed by the English source.
        // The OpenAPI document itself is never translated: it is the machine-readable contract.
        const overlay = locale.key === SOURCE_LOCALE ? null : overlays[locale.key]
        const text = localizeOperation(overlay, full)
        const description = mdxSafe(text.description)
        // No `{#custom-id}` here: MDX parses `{...}` as a JSX expression, so that syntax is a
        // build error rather than a heading id. Starlight slugs the heading text instead, which is
        // also what the right-hand outline links to — and operation titles are unique within a
        // group (checked below), so nothing collides.
        return `## ${mdxSafe(text.summary)}

<ApiEndpoint op="${op.operationId}" />

${description}

<ApiOperation op="${op.operationId}" />
`
      })
      .join('\n')

    const page = `---
title: ${JSON.stringify(title)}
description: ${JSON.stringify(description)}
---

import ApiEndpoint from '${components}/ApiEndpoint.astro'\nimport ApiOperation from '${components}/ApiOperation.astro'

${description}

${body}`
    writeFileSync(join(contentDir, locale.dir, 'api', `${slug}.mdx`), page)
  }
}

// An index page for the API section.
//
// Two jobs: it is the section's own landing page (the reference site has a "Getting started" entry
// in the same place), and it is the target for every "HTTP API 参考" link in the hand-written pages
// — those used to point at the whole-page Scalar viewer this site no longer renders.
for (const locale of LOCALES) {
  const isZh = locale.key === 'zh'
  const localeDir = locale.dir ? `${locale.dir}/` : ''
  const totalOps = nav.reduce((sum, group) => sum + group.operationCount, 0)
  const title = isZh ? 'API 参考' : 'API Reference'
  const intro = isZh
    ? `本参考由 \`crates/lanrurugi-api-docgen\` 从服务端自己的路由注册生成：共 **${totalOps}** 个端点，` +
      `分为 **${nav.length}** 组。每个端点都给出方法、完整路径、参数与响应示例，并带一个请求面板——` +
      `填入**你自己的实例地址**即可直接发送，请求由浏览器直接发往该地址。`
    : `Generated by \`crates/lanrurugi-api-docgen\` from the server's own route registrations: ` +
      `**${totalOps}** endpoints in **${nav.length}** groups. Every endpoint lists its method, full ` +
      `path, parameters and a response example, and carries a request panel — point it at **your own ` +
      `instance** and the request goes straight there from this browser.`
  const list = nav
    .map((group) => `- [${isZh ? group.zh : group.en}](./${group.slug}/) — ${group.operationCount}`)
    .join('\n')
  const download = isZh
    ? `<p><a class="sdk-launch" href={\`${'${import.meta.env.BASE_URL}'}openapi.json\`}>下载 OpenAPI 文档（JSON）</a></p>`
    : `<p><a class="sdk-launch" href={\`${'${import.meta.env.BASE_URL}'}openapi.json\`}>Download the OpenAPI document (JSON)</a></p>`
  const more = isZh
    ? `\n\n怎么让浏览器能直接请求你自己的实例（跨域、HTTPS、令牌），见[用文档站测试 API](../guide/api-testing/)。`
    : `\n\nWhat the browser needs before it can call your own instance (cross-origin access, HTTPS, a ` +
      `token) is in [Testing the API from these docs](../guide/api-testing/).`
  writeFileSync(
    join(contentDir, locale.dir, 'api', 'index.mdx'),
    `---\ntitle: ${JSON.stringify(title)}\ndescription: ${JSON.stringify(intro.slice(0, 120))}\n---\n\n${intro}\n\n${list}${more}\n\n${download}\n`,
  )
}

writeFileSync(join(generatedDir, 'api-nav.json'), `${JSON.stringify(nav, null, 2)}\n`)
// The components read this to localise parameter and response descriptions, which are rendered at
// request time from the document rather than inlined into the generated pages.
writeFileSync(join(generatedDir, 'api-i18n.json'), `${JSON.stringify(overlays, null, 2)}\n`)
// The components read the document themselves rather than receiving operation objects as props:
// the pages stay small and reviewable, and there is exactly one copy of the data in the build.
writeFileSync(join(generatedDir, 'openapi.json'), readFileSync(specPath))

// Coverage, not a pass/fail gate: an untranslated string renders as its English source, which is a
// legitimate intermediate state for a translation that is written by hand. Reporting the count is
// what keeps "how much is left" a measured number instead of an impression — the same reason the
// document's own undocumented-endpoint backlog is reported rather than hidden.
for (const [key, overlay] of Object.entries(overlays)) {
  const counters = { summary: 0, description: 0, parameter: 0, response: 0, quote: 0 }
  const totals = { summary: 0, description: 0, parameter: 0, response: 0, quote: 0 }
  const seenParams = new Map()
  const seenResponses = new Map()
  for (const item of Object.values(spec.paths ?? {})) {
    for (const method of METHODS) {
      const op = item[method]
      if (!op) continue
      const entry = overlay.operations?.[op.operationId] ?? {}
      totals.summary += 1
      totals.description += 1
      if (entry.summary) counters.summary += 1
      if (hasOperationDescription(overlay, op)) counters.description += 1
      // The quoted handler comments are a separate count from the preamble: localising the preamble
      // still leaves English prose in the middle of the page for these endpoints.
      const quoted = (op.description ?? '').includes('\n\n---\n\n')
      if (quoted) {
        totals.quote += 1
        if (overlay.quotedComments?.[op.operationId]) counters.quote += 1
      }
      for (const parameter of op.parameters ?? []) {
        if (parameter.description) seenParams.set(parameter.description, true)
      }
      for (const response of Object.values(op.responses ?? {})) {
        if (response.description) seenResponses.set(response.description, true)
      }
    }
  }
  for (const source of seenParams.keys()) {
    totals.parameter += 1
    if (hasParameterEntry(overlay, source)) counters.parameter += 1
  }
  for (const source of seenResponses.keys()) {
    totals.response += 1
    if (hasResponseEntry(overlay, source)) counters.response += 1
  }
  const pct = (done, all) => (all === 0 ? '100%' : `${Math.round((done / all) * 100)}%`)
  console.log(
    `generate-api-pages: ${key} translation — ` +
      `摘要 ${counters.summary}/${totals.summary} (${pct(counters.summary, totals.summary)}), ` +
      `描述 ${counters.description}/${totals.description} (${pct(counters.description, totals.description)}), ` +
      `参数 ${counters.parameter}/${totals.parameter} (${pct(counters.parameter, totals.parameter)}), ` +
      `响应 ${counters.response}/${totals.response} (${pct(counters.response, totals.response)}), ` +
      `引用注释 ${counters.quote}/${totals.quote} (${pct(counters.quote, totals.quote)})`,
  )
}

const total = nav.reduce((sum, group) => sum + group.operationCount, 0)
console.log(
  `generate-api-pages: ${nav.length} group page(s), ${total} operation(s), ` +
    `${LOCALES.length} locale(s)`,
)
