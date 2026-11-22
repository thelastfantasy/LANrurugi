// `node --test "apps/docs-site/tests/*.test.mjs"` — the R2 zip reader against a real archive
// (built with Python's zipfile, deflated and stored entries), through a fake bucket that honours the
// same ranged `get`/`head` calls R2 does.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { markArchived } from '../functions/[[path]].js'
import { centralDirectory, clearCache, readEntry } from '../functions/_lib/zip.js'

function makeZip() {
  const dir = mkdtempSync(join(tmpdir(), 'docs-zip-'))
  const out = join(dir, 'docs.zip')
  execFileSync('python3', [
    '-c',
    `import zipfile, sys
with zipfile.ZipFile(sys.argv[1], 'w') as z:
    z.writestr('index.html', '<html><head><link rel="canonical" href="https://x/v1/"></head><body>root ' + 'x'*2000 + '</body></html>', compress_type=zipfile.ZIP_DEFLATED)
    z.writestr('guide/sharing/index.html', '<html><head></head><body>sharing</body></html>', compress_type=zipfile.ZIP_DEFLATED)
    z.writestr('favicon.svg', '<svg/>', compress_type=zipfile.ZIP_STORED)
    z.writestr('empty.txt', '', compress_type=zipfile.ZIP_DEFLATED)
    z.comment = b'release zip'`,
    out,
  ])
  return readFileSync(out)
}

function fakeBucket(bytes) {
  const reads = []
  return {
    reads,
    async head(key) {
      return key === 'v.zip' ? { size: bytes.length } : null
    },
    async get(key, { range }) {
      if (key !== 'v.zip') return null
      reads.push(range)
      const slice = bytes.subarray(range.offset, range.offset + range.length)
      return { arrayBuffer: async () => slice.buffer.slice(slice.byteOffset, slice.byteOffset + slice.byteLength) }
    },
  }
}

test('reads deflated and stored entries with ranged reads only', async () => {
  clearCache()
  const zip = makeZip()
  const bucket = fakeBucket(zip)
  const entries = await centralDirectory(bucket, 'v.zip')
  assert.deepEqual([...entries.keys()].sort(), ['empty.txt', 'favicon.svg', 'guide/sharing/index.html', 'index.html'])

  const page = new TextDecoder().decode(await readEntry(bucket, 'v.zip', 'guide/sharing/index.html'))
  assert.equal(page, '<html><head></head><body>sharing</body></html>')
  assert.equal(new TextDecoder().decode(await readEntry(bucket, 'v.zip', 'favicon.svg')), '<svg/>')
  assert.equal((await readEntry(bucket, 'v.zip', 'empty.txt')).length, 0)
  assert.ok(new TextDecoder().decode(await readEntry(bucket, 'v.zip', 'index.html')).includes('root xxx'))
  // After the end record and the directory (one tail read covers a zip this small), every file read
  // is a slice of the archive, not the archive.
  assert.ok(bucket.reads.slice(2).every((r) => r.length < zip.length / 2))
})

test('missing archive, missing entry and directory names are null', async () => {
  clearCache()
  const bucket = fakeBucket(makeZip())
  assert.equal(await readEntry(bucket, 'missing.zip', 'index.html'), null)
  assert.equal(await readEntry(bucket, 'v.zip', 'nope.html'), null)
  assert.equal(await readEntry(bucket, 'v.zip', 'guide/'), null)
})

test('an archived page points search engines at the latest copy', () => {
  const html = '<html><head><link rel="canonical" href="https://docs.lrrg.dev/v1.0.0/guide/a/"></head><body></body></html>'
  const out = markArchived(html, 'https://docs.lrrg.dev', '/guide/a/')
  assert.ok(out.includes('<meta name="robots" content="noindex">'))
  assert.ok(out.includes('<link rel="canonical" href="https://docs.lrrg.dev/guide/a/">'))
  assert.equal(out.match(/rel="canonical"/g).length, 1)
})
