#!/usr/bin/env node
// Applies the shared LANrurugi palette to the `deno doc --html` output: copies the token and SDK
// theme stylesheets into the output directory and links them from every page. deno doc has no
// option for extra stylesheets, so this runs as the last step of `mise run docs-sdk` — every
// regenerated page picks the theme up without anyone touching the output by hand.
//
// It also swaps the top bar's plain title and moon toggle for the brand and the E-H / EX switch the
// homepage and Swagger UI use, shortens source-path labels (`crates/…/plugin-sdk.ts` → `plugin-sdk.ts`) and fills the
// index page, which deno doc leaves empty for a multi-file build, with a grouped symbol overview
// read from `plugin-sdk.json`.
//
// Usage: node docs/scripts/theme-sdk.mjs <deno-doc-output-dir>
// SDK_DOCS_URL sets where the guide links point (default the public docs site; the server image
// passes `/docs/`).

import { copyFileSync, existsSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { basename, dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const outDir = process.argv[2]
if (!outDir) {
  console.error('usage: theme-sdk.mjs <deno-doc-output-dir>')
  process.exit(2)
}
const docsUrl = (process.env.SDK_DOCS_URL || 'https://docs.lrrg.dev/').replace(/\/?$/, '/')

const docsRoot = join(dirname(fileURLToPath(import.meta.url)), '..')
const themeDir = join(docsRoot, 'theme')
copyFileSync(join(themeDir, 'tokens.css'), join(outDir, 'lanrurugi-tokens.css'))
copyFileSync(join(themeDir, 'sdk.css'), join(outDir, 'lanrurugi-sdk.css'))
copyFileSync(join(themeDir, 'sdk.js'), join(outDir, 'lanrurugi-sdk.js'))
copyFileSync(join(docsRoot, '..', 'apps', 'frontend', 'public', 'favicon.svg'), join(outDir, 'lanrurugi.svg'))

const MARKER = 'lanrurugi-sdk.css'

const escapeHtml = (s) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) {
      yield { path, dir: true }
      yield* walk(path)
    } else yield { path, dir: false }
  }
}

// Page directory per source file, keyed by basename: the Docker build documents copies under
// `sdk-src/`, so the JSON's repository paths are not the output's paths.
const moduleDirs = new Map()
for (const entry of walk(outDir)) {
  if (entry.dir && existsSync(join(entry.path, '~'))) {
    moduleDirs.set(basename(entry.path), relative(outDir, entry.path).split(sep).join('/'))
  }
}

/** First sentence of a doc comment, with `code` and {@link X} rendered as code. */
function summary(doc) {
  if (!doc) return ''
  const para = doc.split(/\n\s*\n/)[0].replace(/\s+/g, ' ').trim()
  let text = (para.match(/^(.+?[.!?])(?=\s+[A-Z]|$)/) || [, para])[1]
  if (text.length > 220) text = `${text.slice(0, 220).replace(/\s+\S*$/, '')} …`
  return escapeHtml(text)
    .replace(/\{@link(?:code|plain)?\s+([^}\s|]+)[^}]*\}/g, '<code>$1</code>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
}

const KINDS = [
  ['class', 'Classes', 'Class', 'c'],
  ['interface', 'Interfaces', 'Interface', 'I'],
  ['typeAlias', 'Type aliases', 'TypeAlias', 'T'],
  ['variable', 'Variables', 'Variable', 'v'],
  ['function', 'Functions', 'Function', 'f'],
]

const FILE_BLURBS = {
  'plugin-sdk.ts': 'What a plugin exports and returns: its info, the result of each entry point, and its options.',
  'legacy-globals.d.ts':
    'Globals the host provides to every plugin, and the arguments each entry point receives.',
}

function indexMain() {
  const json = JSON.parse(readFileSync(join(outDir, 'plugin-sdk.json'), 'utf8'))
  const groups = []
  for (const [file, node] of Object.entries(json.nodes)) {
    const symbols = node.symbols ?? []
    const name = basename(file)
    const dir = moduleDirs.get(name)
    if (!symbols.length || !dir) continue
    const blurb = FILE_BLURBS[name] ? escapeHtml(FILE_BLURBS[name]) : summary(node.module_doc?.doc)
    const kinds = KINDS.map(([kind, heading, cls, letter]) => {
      const items = symbols
        .filter((s) => s.declarations?.[0]?.kind === kind)
        .map((s) => {
          const doc = summary(s.declarations[0].jsDoc?.doc)
          return `<li><div class="docNodeKindIcon"><div class="text-${cls} bg-${cls}/15 dark:text-${cls}Dark dark:bg-${cls}Dark/15" title="${heading}">${letter}</div></div><div><a href="./${dir}/~/${encodeURIComponent(s.name)}.html">${escapeHtml(s.name)}</a>${doc ? `<p>${doc}</p>` : ''}</div></li>`
        })
      return items.length ? `<h3>${heading}</h3><ul class="lr-symbols">${items.join('')}</ul>` : ''
    }).join('')
    groups.push(
      `<section class="lr-group"><h2><a href="./${dir}/index.html">${escapeHtml(name)}</a></h2>${blurb ? `<p>${blurb}</p>` : ''}${kinds}</section>`,
    )
  }
  return `<main class="lr-home">
<header class="lr-hero">
<h1>LANrurugi Plugin SDK</h1>
<p>Every type and global a LANrurugi plugin is written against, generated by <code>deno doc</code> from the same TypeScript the plugin runtime loads — so it cannot drift from what the host actually does.</p>
<nav class="lr-links">
<a class="lr-primary" href="${docsUrl}plugin/getting-started/">插件开发入门</a>
<a href="${docsUrl}en/plugin/getting-started/">Getting started</a>
<a href="./all_symbols.html">All symbols</a>
<a href="./plugin-sdk.json">plugin-sdk.json</a>
</nav>
</header>
${groups.join('\n')}
</main>`
}

const THEME_SWITCH =
  '<div class="lr-theme-switch" role="group" aria-label="Theme">' +
  '<button type="button" data-theme="eh" title="E-Hentai" aria-pressed="true">E-H</button>' +
  '<button type="button" data-theme="ex" title="ExHentai" aria-pressed="false">EX</button></div>'

const GITHUB_LINK =
  '<a class="lr-github" href="https://github.com/thelastfantasy/LANrurugi" title="GitHub" aria-label="GitHub">' +
  '<svg viewBox="0 0 16 16" aria-hidden="true" focusable="false"><path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.4 7.4 0 0 1 2-.27c.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg></a>'

/** The first breadcrumb (the site name deno doc prints from `--name`) becomes the brand. */
const brand = (prefix) =>
  `<img src="${prefix}lanrurugi.svg" alt="" width="30" height="30">` +
  '<span class="lr-brand__name">LANrurugi</span><span class="lr-brand__tag">SDK</span>'

const PATH_LABEL = /(>|<title>|- )((?:[\w.-]+(?:&#x2F;|\/))+)([\w.-]+\.ts)(?=<| -)/g

/** deno doc links page to page as `Foo.html`, while Cloudflare Pages 308-redirects `/Foo.html` to
 * `/Foo` (and serves the file there). Every internal click therefore cost a redirect hop, and
 * Search Console reported the discovered `.html` URLs as "Page with redirect". The files stay —
 * where a site links to `Foo.html` by hand, Pages still redirects it once — but nothing here links
 * to them any more. A canonical tag names the extensionless form for anything that does. */
const sdkSiteUrl = (process.env.SDK_SITE_URL || 'https://sdk.lrrg.dev/').replace(/\/?$/, '/')

function extensionlessLinks(html) {
  // deno doc escapes the slashes inside href values (`&#x2F;`), so decode those first — otherwise
  // the pattern below never sees the path it has to shorten.
  const decoded = html.replace(/href="([^"]*)"/g, (_, value) => `href="${value.replace(/&#x2F;|&#47;/g, '/')}"`)
  // Relative and root-relative hrefs only: an absolute URL contains `:`, a protocol-relative one
  // starts with `//`, and both are left alone.
  return decoded.replace(/href="((?!\/\/)\.{0,2}\/?[^":]*?)\.html(#[^"]*)?"/g, (match, path, hash = '') => {
    const asDirectory = path.replace(/index$/, '')
    if (asDirectory !== path) return `href="${asDirectory || './'}${hash}"`
    return `href="${path}${hash}"`
  })
}

let themed = 0
for (const entry of walk(outDir)) {
  const file = entry.path
  if (entry.dir || !file.endsWith('.html')) continue
  let html = readFileSync(file, 'utf8')
  if (html.includes(MARKER)) continue
  const depth = relative(outDir, dirname(file)).split(sep).filter(Boolean).length
  const prefix = '../'.repeat(depth)
  const relativePath = relative(outDir, file).split(sep).join('/')
  // The page's public address: `index.html` is its directory, and Pages strips the `.html`.
  const pagePath = relativePath === '404.html' ? null : relativePath.replace(/(^|\/)index\.html$/, '$1').replace(/\.html$/, '')
  const canonical = pagePath === null ? '' : `<link rel="canonical" href="${sdkSiteUrl}${pagePath}">`
  const links =
    canonical +
    `<link rel="icon" href="${prefix}lanrurugi.svg" type="image/svg+xml">` +
    `<link rel="stylesheet" href="${prefix}lanrurugi-tokens.css">` +
    `<link rel="stylesheet" href="${prefix}${MARKER}">` +
    `<script src="${prefix}lanrurugi-sdk.js" defer></script>`
  if (!html.includes('</head>')) throw new Error(`theme-sdk: no </head> in ${file}`)
  html = html
    .replace('</head>', `${links}</head>`)
    .replace(/<html(?![^>]*\blang=)/, '<html lang="en"')
    .replace(PATH_LABEL, '$1$3')
  if (html.includes('id="topnav"')) {
    // The brand leaves the breadcrumb list so it never shrinks with it; only the trail is clipped.
    const branded = html.replace(
      /<div class="flex items-center">\s*<ul class="breadcrumbs">\s*<li>\s*<a href="([^"]*)" class="contextLink">LANrurugi Plugin SDK<\/a>\s*<\/li>\s*/,
      (_, href) =>
        `<a href="${href}" class="lr-brand">${brand(prefix)}</a>` +
        '<div class="flex items-center lr-trail"><ul class="breadcrumbs">',
    )
    // deno doc prints the current page's crumb as a bare text node, which CSS cannot select.
    const wrapped = branded.replace(/<ul class="breadcrumbs">[\s\S]*?<\/ul>/, (ul) =>
      ul.replace(/>([^<>]*\S[^<>]*)(?=<(?:span|\/ul)\b)/g, (m, text) =>
        m.startsWith('><') ? m : `><span class="lr-crumb">${text.trim()}</span>`,
      ),
    )
    const switched = wrapped.replace(/(<input[^>]*\bid="searchbar"[^>]*\/>)/, `$1${THEME_SWITCH}${GITHUB_LINK}`)
    if (branded === html || switched === branded) throw new Error(`theme-sdk: top bar markup changed in ${file}`)
    html = switched
  }
  if (depth === 0 && basename(file) === 'index.html') {
    if (!/<main>\s*<\/main>/.test(html)) throw new Error('theme-sdk: index.html main is no longer empty')
    html = html.replace(/<main>\s*<\/main>/, indexMain())
  }
  // Last, so the index page's own generated links and the rewritten ones are covered too.
  writeFileSync(file, extensionlessLinks(html))
  themed++
}

console.log(`theme-sdk: themed ${themed} page(s) in ${outDir}`)
