import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

import { expect, test } from './fixtures'
import { fixturePath } from './fixturePath'

// Covers the large-archive upload regression (data-model.md Regression Fixture #2): axum's
// `Multipart` extractor enforces a 2 MB `DefaultBodyLimit` by default; without the explicit
// `DefaultBodyLimit::max(...)` layer (crates/lanrurugi-api/src/upload.rs), an upload past 2 MB
// fails with "Error parsing `multipart/form-data` request" instead of succeeding.
//
// To verify this test actually catches the regression: temporarily remove the
// `.layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))` call in upload.rs's route registration,
// confirm this test fails, then restore it.
test.describe('upload', { tag: '@upload' }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post('/api/login', { form: { password: 'kamimamita' } })
  })

  test('an archive larger than the 2MB default body limit uploads successfully', async ({ page }) => {
    // Pad a real, valid zip fixture past axum's 2MB default so a regression in the override is
    // actually exercised (over 2MB, not merely "a normal-sized archive").
    const original = fs.readFileSync(fixturePath('sample.zip'))
    const padded = Buffer.concat([original, Buffer.alloc(3 * 1024 * 1024)])
    const tmpPath = path.join(os.tmpdir(), `oversized-${Date.now()}.zip`)
    fs.writeFileSync(tmpPath, padded)

    try {
      await page.goto('/upload')
      const fileInput = page.locator('input[type="file"]')
      await fileInput.setInputFiles(tmpPath)

      await expect(page.locator('td', { hasText: 'Click here to edit metadata.' })).toBeVisible({
        timeout: 30_000,
      })
    } finally {
      fs.unlinkSync(tmpPath)
    }
  })

  // Covers the upload-vs-watcher ingestion race regression (data-model.md Regression Fixture #7,
  // found while implementing this feature's own e2e tests, not a previously-known bug): the
  // upload handler used to write bytes directly into the watched `archive_dir` before calling
  // `ingest_file` itself, racing the file watcher's own independent `ingest_file` call on the same
  // newly-appeared file — whichever won the race first "catalogued" it, and the upload handler's
  // own call then often saw the ID as already tracked, returning a spurious 409 for a genuinely
  // first-time upload. The race is intermittent, not deterministic, so this test uploads several
  // distinct never-before-seen archives in quick succession rather than just once.
  test('repeated first-time uploads all succeed (no upload-vs-watcher race)', async ({ page }) => {
    const original = fs.readFileSync(fixturePath('sample.zip'))
    for (let i = 0; i < 5; i++) {
      const unique = Buffer.concat([original, Buffer.from(`race-check-${Date.now()}-${i}-${Math.random()}`)])
      const res = await page.request.put('/api/archives/upload', {
        multipart: { file: { name: `race-check-${i}.zip`, mimeType: 'application/zip', buffer: unique } },
      })
      expect(res.status(), `upload #${i} should succeed, not report a spurious duplicate`).toBe(200)
      const body = (await res.json()) as { success: number }
      expect(body.success).toBe(1)
    }
  })
})
