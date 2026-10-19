#!/usr/bin/env node
// Applies the shared LANrurugi palette to the `deno doc --html` output: copies the token and SDK
// theme stylesheets into the output directory and links them from every page. deno doc has no
// option for extra stylesheets, so this runs as the last step of `mise run docs-sdk` — every
// regenerated page picks the theme up without anyone touching the output by hand.
//
// Usage: node docs/scripts/theme-sdk.mjs <deno-doc-output-dir>

import { copyFileSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const outDir = process.argv[2]
if (!outDir) {
  console.error('usage: theme-sdk.mjs <deno-doc-output-dir>')
  process.exit(2)
}

const themeDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'theme')
copyFileSync(join(themeDir, 'tokens.css'), join(outDir, 'lanrurugi-tokens.css'))
copyFileSync(join(themeDir, 'sdk.css'), join(outDir, 'lanrurugi-sdk.css'))

const MARKER = 'lanrurugi-sdk.css'

function* htmlFiles(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) yield* htmlFiles(path)
    else if (name.endsWith('.html')) yield path
  }
}

let themed = 0
for (const file of htmlFiles(outDir)) {
  const html = readFileSync(file, 'utf8')
  if (html.includes(MARKER)) continue
  const depth = relative(outDir, dirname(file)).split(sep).filter(Boolean).length
  const prefix = '../'.repeat(depth)
  const links =
    `<link rel="stylesheet" href="${prefix}lanrurugi-tokens.css">` +
    `<link rel="stylesheet" href="${prefix}${MARKER}">`
  if (!html.includes('</head>')) throw new Error(`theme-sdk: no </head> in ${file}`)
  writeFileSync(file, html.replace('</head>', `${links}</head>`))
  themed++
}

console.log(`theme-sdk: themed ${themed} page(s) in ${outDir}`)
