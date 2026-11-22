// Decides which released docs versions are deployed as static files and which are served from R2
// (#305). Cloudflare Pages caps one deployment at 20,000 files and every version adds ~290, so only
// the newest stable release and the newest few prereleases stay static; the rest are read from their
// release zip by the Pages Function.
//
//   node scripts/plan-versions.mjs --current <version> --r2 <true|false> <version>...
//
// Prints `static=<v,…>` and `archived=<v,…>` lines (for `$GITHUB_OUTPUT`). Without R2 every version
// stays static, which is today's layout and still works until the file cap is reached.
import { parseArgs } from 'node:util'

const KEEP_PRERELEASES = 3

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: { current: { type: 'string' }, r2: { type: 'string', default: 'false' } },
})

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
  if (a.pre === b.pre) return 0
  if (a.pre === '') return 1
  if (b.pre === '') return -1
  return a.pre.localeCompare(b.pre, undefined, { numeric: true })
}

const versions = [...new Set(positionals.map((v) => v.replace(/^v/, '')))]
  .filter((v) => v && v !== values.current)
  .sort((a, b) => compare(b, a))

let keep = versions
if (values.r2 === 'true') {
  const stable = versions.find((v) => !v.includes('-'))
  const prereleases = versions.filter((v) => v.includes('-')).slice(0, KEEP_PRERELEASES)
  keep = versions.filter((v) => v === stable || prereleases.includes(v))
}
const archived = versions.filter((v) => !keep.includes(v))

console.log(`static=${keep.join(',')}`)
console.log(`archived=${archived.join(',')}`)
