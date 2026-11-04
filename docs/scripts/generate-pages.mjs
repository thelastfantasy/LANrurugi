// Writes `dist/pages.json`: every page path this build produced, as slugs relative to the build's
// base (`"guide/install/"`, `""` for the index).
//
// The header's version selector reads it before switching versions: a page the target version never
// had must land on that version's index instead of a 404. Generated from the built HTML rather than
// from the markdown sources so it can only ever list pages that actually exist.
import { readdirSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const dist = resolve(here, '../dist')

const pages = []
const walk = (dir) => {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) {
      walk(full)
      continue
    }
    if (entry !== 'index.html') continue
    const slug = relative(dist, dir)
    pages.push(slug ? `${slug}/` : '')
  }
}

walk(dist)
pages.sort()
writeFileSync(join(dist, 'pages.json'), `${JSON.stringify(pages, null, 2)}\n`)
console.log(`pages.json: ${pages.length} page(s)`)
