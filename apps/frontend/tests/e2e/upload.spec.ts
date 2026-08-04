import fs from "node:fs"
import os from "node:os"
import path from "node:path"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

// Covers the large-archive upload regression (data-model.md Regression Fixture #2): axum's
// `Multipart` extractor enforces a 2 MB `DefaultBodyLimit` by default; without the explicit
// `DefaultBodyLimit::max(...)` layer (crates/lanrurugi-api/src/upload.rs), an upload past 2 MB
// fails with "Error parsing `multipart/form-data` request" instead of succeeding.
//
// To verify this test actually catches the regression: temporarily remove the
// `.layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))` call in upload.rs's route registration,
// confirm this test fails, then restore it.
test.describe("upload", { tag: "@upload" }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  test("an archive larger than the 2MB default body limit uploads successfully", async ({ page }) => {
    // Pad a real, valid zip fixture past axum's 2MB default so a regression in the override is
    // actually exercised (over 2MB, not merely "a normal-sized archive"). The filename carries a
    // timestamp specifically so it's unique against whatever the download queue panel's
    // worker-scoped backend already has queued from other tests sharing the same worker/backend
    // instance (queue history persists in Redis for the worker's whole lifetime, not per-test).
    const original = fs.readFileSync(fixturePath("sample.zip"))
    const padded = Buffer.concat([original, Buffer.alloc(3 * 1024 * 1024)])
    const uniqueName = `oversized-${Date.now()}`
    const tmpPath = path.join(os.tmpdir(), `${uniqueName}.zip`)
    fs.writeFileSync(tmpPath, padded)

    try {
      await page.goto("/upload")
      const fileInput = page.locator('input[type="file"]')
      await fileInput.setInputFiles(tmpPath)

      // The local-upload result UI was rewritten (issue #2: uploads now share the download
      // queue's own persisted-state panel, not a standalone <table>) — a `done` row renders as a
      // green bar whose whole title is a link straight to the reader, not a separate "Click here
      // to edit metadata." cell. Match the current UI's real success signal instead, scoped to
      // this test's own uniquely-named upload — the panel accumulates rows from every prior test
      // that shared this worker's backend, so an unscoped `a[href^="/reader/"]` locator matches
      // several unrelated rows (strict-mode violation) rather than resolving unambiguously.
      await expect(page.getByRole("link", { name: new RegExp(uniqueName) })).toBeVisible({
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
  test("repeated first-time uploads all succeed (no upload-vs-watcher race)", async ({ page }) => {
    const original = fs.readFileSync(fixturePath("sample.zip"))
    for (let i = 0; i < 5; i++) {
      // `wait_until_stable` (lanrurugi-scanner's watcher.rs, mirroring legacy Shinobu.pm's
      // add_to_filemap) treats any file under its 512000-byte hashing-sample threshold as
      // "possibly still being written" and polls up to 5 times at a 1s interval before giving up
      // and proceeding anyway — a deliberate, unit-tested behavior, not a bug. `sample.zip` itself
      // is 416 bytes, so an unpadded upload pays that full ~5s tax on every single ingest. This
      // loop's whole point is 5 uploads in quick succession to exercise a race, not to exercise
      // that stability wait — padding each buffer past the threshold lets the first size check
      // pass immediately, keeping the test's real 30s budget for the thing it actually tests.
      const unique = Buffer.concat([
        original,
        Buffer.from(`race-check-${Date.now()}-${i}-${Math.random()}`),
        Buffer.alloc(520_000),
      ])
      const res = await page.request.put("/api/archives/upload", {
        multipart: { file: { name: `race-check-${i}.zip`, mimeType: "application/zip", buffer: unique } },
      })
      expect(res.status(), `upload #${i} should succeed, not report a spurious duplicate`).toBe(200)
      const body = (await res.json()) as { success: number }
      expect(body.success).toBe(1)
    }
  })
})
