#!/usr/bin/env node
// Build the second of the three deliverables: a standalone Swagger UI page over the same
// `openapi.json` the docs site renders.
//
// Why this is a separate artifact and not a page in the docs site — the question that led to it not
// existing at first: the docs site renders the *prose* reference (one page per API group, like the
// GitBook reference), which reads well but cannot answer "what shape is this response?" or "show me
// the schemas". A Swagger UI page can. Different job, so different page — and publishing it as its
// own Cloudflare Pages project keeps the docs site from growing a 1.4MB JS bundle nobody browsing the
// install guide asked for.
//
// Two locales, matching the docs site's own routing: `/` is Chinese (the project's primary locale)
// and `/en/` is English. Swagger UI has no localisation mechanism at all — verified against the
// bundle rather than assumed: `i18n`/`intl`/`messages:`/`currentLocale` all occur zero times, the
// six `locale` hits are `localeCompare`/`KeyboardEvent.locale`, and every label is a hardcoded
// literal. What *can* be localised is the document it renders, because that text comes from the
// document. So each locale gets its own generated copy of the spec and the widget's own chrome stays
// English on both pages.

import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  localizeAccessNotes,
  localizeOperation,
  localizeParameter,
  localizeResponse,
} from '../src/lib/i18n-core.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const docsRoot = join(here, '..')
const repoRoot = join(docsRoot, '..')

const specPath = join(docsRoot, 'public', 'openapi.json')
const overlayPath = join(docsRoot, 'api', 'i18n', 'zh.json')
const outDir = join(docsRoot, 'dist-swagger')

if (!existsSync(specPath)) {
  console.error(
    `build-swagger: ${relative(repoRoot, specPath)} not found — run \`mise run docs-api\` first.`,
  )
  process.exit(2)
}

// Resolve through Node rather than assuming a path, so a pnpm layout change breaks here with a
// readable error instead of copying nothing.
const require = createRequire(import.meta.url)
let assetDir
try {
  assetDir = dirname(require.resolve('swagger-ui-dist/package.json'))
} catch {
  console.error(
    'build-swagger: `swagger-ui-dist` is not installed. Run `pnpm install` from the repo root.',
  )
  process.exit(2)
}

const SWAGGER_DIR = join(docsRoot, 'swagger')

const ASSETS = [
  'swagger-ui.css',
  'swagger-ui-bundle.js',
  'swagger-ui-standalone-preset.js',
  'oauth2-redirect.html',
]

rmSync(outDir, { recursive: true, force: true })
mkdirSync(outDir, { recursive: true })

let copied = 0
for (const asset of ASSETS) {
  const from = join(assetDir, asset)
  if (!existsSync(from)) {
    console.error(
      `build-swagger: swagger-ui-dist no longer ships ${asset}. Its file list changed — update ` +
        `ASSETS in this script rather than shipping a page that 404s one of them.`,
    )
    process.exit(2)
  }
  copyFileSync(from, join(outDir, asset))
  copied += 1
}

copyFileSync(specPath, join(outDir, 'openapi.json'))
copyFileSync(join(SWAGGER_DIR, 'swagger-init.js'), join(outDir, 'swagger-init.js'))
copyFileSync(join(SWAGGER_DIR, 'swagger-custom.css'), join(outDir, 'swagger-custom.css'))
// Shared LANrurugi palette plus Swagger UI's mapping onto it (see swagger/swagger-theme.css).
copyFileSync(join(docsRoot, 'theme', 'tokens.css'), join(outDir, 'lanrurugi-tokens.css'))
copyFileSync(join(SWAGGER_DIR, 'swagger-theme.css'), join(outDir, 'swagger-theme.css'))
// The project icon, for the tab and for the top-bar logo swagger-init.js puts in Swagger's place.
copyFileSync(join(repoRoot, 'apps', 'frontend', 'public', 'favicon.svg'), join(outDir, 'lanrurugi.svg'))

// ── The Chinese copy of the document ────────────────────────────────────────────────────────────
//
// Built here, from the same overlay module the docs site uses, so the two surfaces cannot disagree
// about what an endpoint says. Only the fields the overlay covers are touched: `summary`,
// `description`, parameter and response descriptions, and the access notes. `tags`, schema names,
// property names, paths and enum values stay as they are — they are identifiers, not prose.
function localizeDocument(document, overlay) {
  const METHODS = ['get', 'post', 'put', 'delete', 'patch', 'head', 'options']
  const localized = structuredClone(document)

  for (const [path, item] of Object.entries(localized.paths ?? {})) {
    for (const method of METHODS) {
      const operation = item[method]
      if (!operation) continue

      const text = localizeOperation(overlay, operation)
      operation.summary = text.summary
      // `localizeOperation` deliberately strips the appended access notes from the description: the
      // documentation site renders them as its own callout. This page has no callout, so the prose
      // copy is re-attached below from the localised notes.
      operation.description = text.description

      for (const parameter of operation.parameters ?? []) {
        if (parameter.description) {
          parameter.description = localizeParameter(overlay, parameter.description)
        }
      }

      for (const response of Object.values(operation.responses ?? {})) {
        if (response.description) {
          response.description = localizeResponse(overlay, response.description)
        }
      }

      const notes = localizeAccessNotes(overlay, operation)
      if (notes.length > 0) {
        operation.description = [
          operation.description,
          '<!-- lanrurugi:access-notes -->',
          '',
          '**权限差异**',
          '',
          ...notes.map((note) => `- ${note}`),
        ]
          .filter((part) => part !== '')
          .join('\n\n')
        // Keep the structured field in step with the prose, so a consumer reading either form of the
        // Chinese document sees the same thing.
        operation['x-lanrurugi-access-notes'] = notes
      }
    }
  }

  return localized
}

const overlay = JSON.parse(
  // Read through the same file the docs site does; a missing overlay is a broken build, not a page
  // that quietly falls back to English.
  (() => {
    if (!existsSync(overlayPath)) {
      console.error(
        `build-swagger: ${relative(repoRoot, overlayPath)} not found — the Chinese page would render ` +
          `English content. Run the generator, or delete the zh locale from this script on purpose.`,
      )
      process.exit(2)
    }
    return require('node:fs').readFileSync(overlayPath, 'utf8')
  })(),
)

writeFileSync(
  join(outDir, 'openapi.zh.json'),
  `${JSON.stringify(localizeDocument(JSON.parse(require('node:fs').readFileSync(specPath, 'utf8')), overlay), null, 2)}\n`,
)

// ── The two pages ───────────────────────────────────────────────────────────────────────────────

const LOCALES = [
  {
    dir: '',
    lang: 'zh-CN',
    title: 'LANrurugi API — Swagger UI',
    themeLabel: '配色',
    // Asset prefix: the root page sits next to the assets, the English page one level down.
    prefix: './',
    spec: 'openapi.zh.json',
    switcher: { label: 'English', href: './en/' },
  },
  {
    dir: 'en',
    lang: 'en',
    title: 'LANrurugi API — Swagger UI',
    themeLabel: 'Color theme',
    prefix: '../',
    // Relative to the *page*, not the site root: this page lives at `/en/`, so a bare
    // `openapi.json` would resolve to `/en/openapi.json`, which does not exist.
    spec: '../openapi.json',
    switcher: { label: '简体中文', href: '../' },
  },
]

function page(locale) {
  return `<!DOCTYPE html>
<html lang="${locale.lang}">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${locale.title}</title>
    <link rel="icon" type="image/svg+xml" href="${locale.prefix}lanrurugi.svg" />
    <link rel="stylesheet" href="${locale.prefix}swagger-ui.css" />
    <link rel="stylesheet" href="${locale.prefix}swagger-custom.css" />
    <link rel="stylesheet" href="${locale.prefix}lanrurugi-tokens.css" />
    <link rel="stylesheet" href="${locale.prefix}swagger-theme.css" />
    <script>
      // E-H / EX palette before first paint: the stored choice, else the OS preference.
      try {
        const t = localStorage.getItem('lanrurugi-swagger:theme')
        const dark = t ? t === 'ex' : matchMedia('(prefers-color-scheme: dark)').matches
        if (dark) document.documentElement.classList.add('dark-mode')
      } catch {}
    </script>
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script>
      // The spec this page renders, and where the other locale lives. Set here rather than inside
      // swagger-init.js because that file is copied verbatim to both locales and is a real file
      // precisely so it needs no escaping.
      window.__LRR_SWAGGER__ = ${JSON.stringify({
        spec: locale.spec,
        switcher: locale.switcher,
        logo: `${locale.prefix}lanrurugi.svg`,
        themeLabel: locale.themeLabel,
      })}
    </script>
    <script src="${locale.prefix}swagger-ui-bundle.js" crossorigin></script>
    <script src="${locale.prefix}swagger-ui-standalone-preset.js" crossorigin></script>
    <script src="${locale.prefix}swagger-init.js"></script>
  </body>
</html>
`
}

for (const locale of LOCALES) {
  const dir = locale.dir ? join(outDir, locale.dir) : outDir
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'index.html'), page(locale))
}

// Cloudflare Pages serves this for any unmatched path. Without it the project is treated as a
// single-page app and unmatched paths (e.g. a stale link to `/swagger/`) get `index.html` with a
// 200, which hides broken links instead of showing them.
writeFileSync(
  join(outDir, '404.html'),
  `<!DOCTYPE html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <title>404 — LANrurugi API</title>
    <link rel="stylesheet" href="/swagger-custom.css" />
    <link rel="stylesheet" href="/lanrurugi-tokens.css" />
    <link rel="stylesheet" href="/swagger-theme.css" />
  </head>
  <body style="font-family: system-ui, sans-serif; padding: 4rem 2rem; text-align: center">
    <h1>404</h1>
    <p>没有这个页面。<a href="/">回到 API 参考</a>，或 <a href="/en/">看英文版</a>。</p>
  </body>
</html>
`,
)

const walk = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = join(dir, entry.name)
    return entry.isDirectory() ? walk(full) : [full]
  })
const files = walk(outDir)
const total = files.reduce((sum, path) => sum + statSync(path).size, 0)
console.log(
  `build-swagger: ${copied} asset(s) + openapi.json (+ zh) + 2 page(s) + 404 -> ` +
    `${relative(repoRoot, outDir)} (${(total / 1048576).toFixed(1)} MB, ${files.length} file(s))`,
)
