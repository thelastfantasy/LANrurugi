import fs from 'node:fs'
import path from 'node:path'

import { expect, test } from './fixtures'
import { fixturePath } from './fixturePath'

// Several sample fixtures were originally byte-identical copies under different extensions
// (T009's `sample.zip`/`.cbz`/`.epub`; T010's `sample.7z`/`.cb7`) — the archive id is a content
// hash, so uploading two unmodified in the same run collides with a 409 "already exists".
// Appending a unique trailing marker is harmless for those container formats (each reader seeks
// its own end-of-archive structure, not a fixed file-length offset) — verified this actually
// holds per-format rather than assumed: zip/cbz/epub/rar/cbr/7z/cb7/tar/gz/bz2 all tolerate it.
//
// `sample.lzh`/`sample.lha` and `sample.xz` do NOT tolerate trailing padding (verified directly:
// `7z l`/`tar -tJf` against a padded copy both reported real parse errors, not just a benign extra
// byte, since neither format has a fixed end-of-archive marker the way zip/7z do) — those three
// fixtures are pre-generated with genuinely distinct embedded content instead (see T012/T011's own
// regeneration), so they need no padding here at all.
const NO_TRAILING_PADDING = new Set(['sample.lzh', 'sample.lha', 'sample.xz'])

async function uploadArchive(page: import('@playwright/test').Page, filePath: string, filename: string) {
  const original = fs.readFileSync(filePath)
  const buffer = NO_TRAILING_PADDING.has(path.basename(filePath))
    ? original
    : Buffer.concat([original, Buffer.from(`unique-${Date.now()}-${Math.random()}`)])
  const res = await page.request.put('/api/archives/upload', {
    multipart: { file: { name: filename, mimeType: 'application/octet-stream', buffer } },
  })
  return res
}

test.describe('archive formats', { tag: '@archive-formats' }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post('/api/login', { form: { password: 'kamimamita' } })
  })

  // Covers the CJK archive-entry filename mojibake regression (data-model.md Regression Fixture
  // #6): a legacy zip entry with a raw Shift-JIS-encoded filename and the UTF-8 general-purpose
  // flag left unset must decode correctly, not render as mojibake or error — plus the separate
  // UTF-8-locale failure this project's own fix uncovered (`archive_read_next_header` rejecting
  // valid UTF-8-flagged non-ASCII names entirely under a bare C/POSIX process locale).
  //
  // To verify: temporarily revert crates/lanrurugi-scanner/src/archive_format.rs's
  // `Utf8LocaleGuard` to use `newlocale`'s empty-string argument instead of the hardcoded
  // "C.utf8", confirm this test fails under this project's own dev container (no LANG env var
  // set), then restore it.
  test('CJK filename without UTF-8 flag decodes correctly, not as mojibake', async ({ page }) => {
    const res = await uploadArchive(page, fixturePath('cjk-shiftjis-noflag.zip'), 'cjk-shiftjis-noflag.zip')
    expect(res.ok()).toBe(true)
    const body = (await res.json()) as { success: number; id: string }
    expect(body.success).toBe(1)

    await page.goto(`/reader/${body.id}`)
    const image = page.locator('#i3 img.reader-image').first()
    await image.waitFor({ state: 'visible' })
    const src = await image.getAttribute('src')
    // `getAttribute` returns the raw attribute value (browsers do not re-encode it for this API),
    // so the correctly-decoded CJK text appears literally, not percent-encoded.
    expect(src).toContain('空白 テスト.jpg')
  })

  // Multi-volume 7z (spec User Story 4 Acceptance Scenarios 2-3): this scenario documents and
  // locks in whatever this project's *current* behavior actually is for a split archive — not a
  // prescriptive "should support" assertion. See data-model.md's Fixture Archive
  // `expected_behavior: current-behavior-locked` field: a future deliberate change to
  // multi-volume handling should update this test's assertion as a reviewed change, not have it
  // silently pass either way.
  //
  // Actual current behavior (verified by hand against a real running backend, not assumed):
  // uploading a lone `.001` first-volume file *succeeds* at the upload step (the backend accepts
  // and stores the raw bytes), but the archive is then unreadable — `GET /files` fails with
  // `"unsupported archive extension: \"001\""`, since extension-based format detection sees `.001`
  // rather than `.7z`. This is a real gap: no multi-volume assembly happens at all today.
  test('multi-volume 7z: current behavior is recorded (upload of first volume)', async ({ page }) => {
    const uploadRes = await uploadArchive(page, fixturePath('multivolume.7z.001'), 'multivolume.7z.001')
    expect(uploadRes.ok()).toBe(true)
    const uploadBody = (await uploadRes.json()) as { success: number; id: string }
    expect(uploadBody.success).toBe(1)

    const filesRes = await page.request.get(`/api/archives/${uploadBody.id}/files`)
    const filesBody = (await filesRes.json()) as { success: number; error?: string }
    expect(filesBody.success).toBe(0)
    expect(filesBody.error).toContain('unsupported archive extension')
  })

  // Encrypted 7z (spec User Story 4 Acceptance Scenarios 2-3): same current-behavior-locked
  // approach as the multi-volume case above — no password-entry UI exists yet in this project
  // (per the user's own disclosed-but-not-yet-implemented encrypted-archive roadmap).
  //
  // Actual current behavior (verified by hand): upload succeeds (raw bytes are accepted and
  // stored), but `GET /files` fails with a clear libarchive error — "The archive header is
  // encrypted, but currently not supported" — rather than a silent empty page list or a crash.
  test('encrypted 7z: current behavior is recorded (upload without a password)', async ({ page }) => {
    const uploadRes = await uploadArchive(page, fixturePath('encrypted.7z'), 'encrypted.7z')
    expect(uploadRes.ok()).toBe(true)
    const uploadBody = (await uploadRes.json()) as { success: number; id: string }
    expect(uploadBody.success).toBe(1)

    const filesRes = await page.request.get(`/api/archives/${uploadBody.id}/files`)
    const filesBody = (await filesRes.json()) as { success: number; error?: string }
    expect(filesBody.success).toBe(0)
    expect(filesBody.error).toContain('encrypted')
  })

  // Spec FR-009/User Story 4: every plain (non-encrypted, non-split, ASCII-filename) format
  // `lanrurugi-scanner` supports must ingest and be fully readable — one scenario per remaining
  // fixture (the CJK zip test above and the two current-behavior-locked tests above already cover
  // zip-family and 7z; this covers the rest of `ARCHIVE_EXTENSIONS`).
  const plainFormats: { file: string; mime: string }[] = [
    { file: 'sample.zip', mime: 'application/zip' },
    { file: 'sample.cbz', mime: 'application/zip' },
    { file: 'sample.epub', mime: 'application/epub+zip' },
    { file: 'sample.rar', mime: 'application/x-rar-compressed' },
    { file: 'sample.cbr', mime: 'application/x-rar-compressed' },
    { file: 'sample.7z', mime: 'application/x-7z-compressed' },
    { file: 'sample.cb7', mime: 'application/x-7z-compressed' },
    { file: 'sample.lzh', mime: 'application/octet-stream' },
    { file: 'sample.lha', mime: 'application/octet-stream' },
    { file: 'sample.tar', mime: 'application/x-tar' },
    { file: 'sample.gz', mime: 'application/gzip' },
    { file: 'sample.bz2', mime: 'application/x-bzip2' },
    { file: 'sample.xz', mime: 'application/x-xz' },
  ]

  for (const { file, mime } of plainFormats) {
    test(`${file}: uploads and every page is viewable`, async ({ page }) => {
      const res = await uploadArchive(page, fixturePath(file), file)
      expect(res.ok(), `${file} upload should succeed`).toBe(true)
      const body = (await res.json()) as { success: number; id: string }
      expect(body.success).toBe(1)

      const filesRes = await page.request.get(`/api/archives/${body.id}/files`)
      const filesBody = (await filesRes.json()) as { success?: number; pages?: string[] }
      expect(filesBody.pages?.length ?? 0).toBeGreaterThan(0)

      await page.goto(`/reader/${body.id}`)
      const image = page.locator('#i3 img.reader-image').first()
      await expect(image).toBeVisible({ timeout: 10_000 })
      void mime // documents the real content-type a browser/client would send; not asserted on
    })
  }

  // Extends non-ASCII-filename coverage (spec FR-010) to a second container format (RAR), per
  // User Story 4 Acceptance Scenario 4's "not only the format the original bug happened to be
  // found in" requirement. Uses the `unrar` crate's own bundled `unicode.rar` fixture (research.md
  // §6) — its content is Latin/symbol/emoji text, not genuine CJK (a project-built CJK-content RAR
  // fixture would need a personally-licensed RAR-creation tool used as a one-time maintainer task,
  // out of scope for this automated suite — see research.md §6 and this file's own git history for
  // that decision), so this scenario covers general non-ASCII decoding, not the CJK-mojibake
  // regression specifically.
  test('RAR: non-ASCII filename decodes correctly', async ({ page }) => {
    const res = await uploadArchive(page, fixturePath('rar/unicode.rar'), 'unicode-rar-test.rar')
    expect(res.ok()).toBe(true)
    const body = (await res.json()) as { success: number; id: string }
    expect(body.success).toBe(1)

    const filesRes = await page.request.get(`/api/archives/${body.id}/files`)
    const filesBody = (await filesRes.json()) as { pages?: string[] }
    // unicode.rar's one entry isn't image-named, so it won't appear in the page list — this
    // asserts the archive opened and was scanned without error/mojibake-induced garbage, which is
    // what `list_pages` succeeding (returning an empty, not error, array) demonstrates here.
    expect(filesBody.pages).toEqual([])
  })
})
