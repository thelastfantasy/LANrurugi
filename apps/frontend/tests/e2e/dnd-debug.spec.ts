import fs from 'node:fs'
import http from 'node:http'

import { expect, test } from './fixtures'
import { fixturePath } from './fixturePath'

// TEMPORARY debug spec — reproduces a real drag-and-drop bug in TemplateInput's rename popover
// (apps/frontend/src/pages/Upload.tsx) via a genuine Playwright-driven mouse gesture, since a
// synthetic-DragEvent script (dispatched directly via devtools) failed to reproduce the real
// failure the user reported. Uses `plugins/download/e2e-dnd-test.ts` (a temporary test-only
// plugin) plus a local static-file server this spec starts itself, so a real
// `pending_filename_conflict` queue item can be produced without depending on a real external
// download plugin's real network target. DELETE this file (and the test plugin) once the bug is
// confirmed fixed.
test.describe('dnd debug', () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post('/api/login', { form: { password: 'kamimamita' } })
  })

  test('real mouse drag of a template-variable button into the rename popover', async ({ page, baseURL }) => {
    page.on('console', (msg) => {
      if (msg.text().includes('[DND]')) console.log('BROWSER:', msg.text())
    })
    const original = fs.readFileSync(fixturePath('sample.zip'))
    // Two distinct byte sequences (real content difference, not just a different URL) — a real
    // filename collision (PendingFilenameConflict) only arises when the resolved filenames match
    // AND the content genuinely differs; identical bytes under different query strings instead hit
    // DuplicateArchive's content-hash path and get rejected outright, never reaching a conflict at
    // all (confirmed live — this is exactly what happened before this fix).
    const fileBuf1 = Buffer.concat([original, Buffer.from('e2e-dnd-test-variant-1')])
    const fileBuf2 = Buffer.concat([original, Buffer.from('e2e-dnd-test-variant-2')])
    const server = http.createServer((req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/zip' })
      res.end(req.url?.includes('e2e-dnd-test=2') ? fileBuf2 : fileBuf1)
    })
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
    const addr = server.address()
    if (!addr || typeof addr === 'string') throw new Error('server did not bind to a port')
    const fileUrl = `http://127.0.0.1:${addr.port}/dnd-test.zip`

    try {
      // First download: succeeds, catalogs a real archive named dnd-test.zip.
      const add1 = await page.request.post('/api/download_queue', {
        data: {
          items: [
            {
              url: `${fileUrl}?e2e-dnd-test=1`,
              plugin_namespace: 'download/e2e-dnd-test',
              category: null,
              auto_fetch_metadata: false,
              overwrite_on_duplicate: false,
            },
          ],
        },
      })
      expect(add1.ok(), await add1.text()).toBe(true)
      const body1 = (await add1.json()) as { added: { id: string }[]; rejected: unknown[] }
      console.log('ADD1 RESPONSE:', JSON.stringify(body1))
      const item1Id = body1.added[0].id
      const start1 = await page.request.post(`/api/download_queue/${item1Id}/start`)
      expect(start1.ok(), await start1.text()).toBe(true)

      // Poll until the first item is done (archive cataloged) before starting the second, so the
      // second one deterministically collides with the first's now-cataloged filename.
      await expect
        .poll(
          async () => {
            const res = await page.request.get('/api/download_queue')
            const list = (await res.json()) as { items: { id: string; state: string }[] }
            return list.items.find((i) => i.id === item1Id)?.state
          },
          { timeout: 30_000 },
        )
        .toBe('done')

      // Second download: same filename_hint (dnd-test.zip) but different content (query string
      // makes the URL distinct so the plugin/queue treat it as a separate download) — collides on
      // filename, not content hash, producing a real PendingFilenameConflict (not a rejected
      // DuplicateArchive).
      const add2 = await page.request.post('/api/download_queue', {
        data: {
          items: [
            {
              url: `${fileUrl}?e2e-dnd-test=2`,
              plugin_namespace: 'download/e2e-dnd-test',
              category: null,
              auto_fetch_metadata: false,
              overwrite_on_duplicate: false,
            },
          ],
        },
      })
      expect(add2.ok(), await add2.text()).toBe(true)
      const body2 = (await add2.json()) as { added: { id: string }[] }
      const item2Id = body2.added[0].id
      const start2 = await page.request.post(`/api/download_queue/${item2Id}/start`)
      expect(start2.ok(), await start2.text()).toBe(true)

      await expect
        .poll(
          async () => {
            const res = await page.request.get('/api/download_queue')
            const list = (await res.json()) as {
              items: { id: string; state: string; pending_filename_conflict: unknown; error: unknown }[]
            }
            const item = list.items.find((i) => i.id === item2Id)
            if (item?.state === 'error') console.log('ITEM2 ERROR:', JSON.stringify(item.error))
            return item?.pending_filename_conflict ? 'conflict' : item?.state
          },
          { timeout: 30_000 },
        )
        .toBe('conflict')

      // Real UI from here on — open the Upload page, resolve the conflict row's dropdown, open
      // the rename popover, and attempt a real mouse-driven drag of a template-variable button
      // into the contentEditable input.
      await page.goto('/upload')
      await page.reload() // ensure the queue (already populated via API) is loaded fresh

      const conflictRow = page.locator('div', { hasText: 'dnd-test.zip' }).last()
      await conflictRow.scrollIntoViewIfNeeded()

      const resolveBtn = page.locator('button:has(i.fa-clone)').last()
      await resolveBtn.click()

      const renameMenuItem = page.getByText('Rename and Catalog', { exact: false })
      await renameMenuItem.click()

      const editor = page.locator('[contenteditable="true"][role="textbox"]')
      await expect(editor).toBeVisible()

      const beforeText = await editor.textContent()
      console.log('BEFORE DRAG:', JSON.stringify(beforeText))

      const titleButton = page.locator('button[draggable="true"]', { hasText: '{title}' })
      await expect(titleButton).toBeVisible()

      // Playwright's own high-level `dragTo` — a hand-rolled mouse.down/move/up sequence
      // (tried first) never got Chromium to recognize it as a real HTML5 drag at all (confirmed:
      // dragstart fired, then dragend fired immediately after with dropEffect "none", with zero
      // dragover/drop events in between — a fundamentally different, Playwright-simulation-only
      // failure mode from the one being investigated, not the real bug). `dragTo` handles the
      // internal timing HTML5 DnD needs that raw mouse events alone don't reliably reproduce.
      const editorBox = await editor.boundingBox()
      if (!editorBox) throw new Error('editor bounding box missing')
      await titleButton.dragTo(editor, {
        targetPosition: { x: editorBox.width - 5, y: editorBox.height / 2 },
      })
      await page.waitForTimeout(200)

      const afterText = await editor.textContent()
      console.log('AFTER DRAG:', JSON.stringify(afterText))

      expect(afterText).not.toBe(beforeText)
      expect(afterText).toContain('{title}')
    } finally {
      server.close()
    }
  })
})
