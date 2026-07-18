import { expect, test } from './fixtures'

// Covers the category `pinned`-field save regression (data-model.md Regression Fixture #1): the
// backend's Form extractor (serde_urlencoded) only accepts the literal strings "true"/"false" for
// a bare Rust bool, not "1"/"0" — sending the latter 422s. Also covers the other per-field
// autosave paths (name, predicate) for real interaction-path coverage per spec User Story 2
// Acceptance Scenario 2, not just the checkbox.
//
// To verify this test actually catches the regression: temporarily revert
// apps/frontend/src/pages/Categories.tsx's `saveDetails` to send `pinned: (next.pinned ?? pinned)
// ? '1' : '0'` instead of `'true'`/`'false'`, confirm this test fails, then restore the fix.
test.describe('categories', { tag: '@categories' }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post('/api/login', { form: { password: 'kamimamita' } })
  })

  async function createStaticCategory(page: import('@playwright/test').Page, name: string) {
    const res = await page.request.put('/api/categories', { form: { name, search: '' } })
    expect(res.ok()).toBe(true)
    const body = (await res.json()) as { category_id: string }
    return body.category_id
  }

  test('pinned-field save persists across reload', async ({ page }) => {
    await createStaticCategory(page, 'Pinned Regression Test')
    await page.goto('/categories')

    await page.selectOption('select.favtag-btn', { label: 'Pinned Regression Test' })
    const pinnedCheckbox = page.locator('#pinned')
    await expect(pinnedCheckbox).not.toBeChecked()

    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/categories/') && r.request().method() === 'PUT'),
      pinnedCheckbox.check(),
    ])
    expect(response.status()).toBe(200)

    await page.reload()
    await page.selectOption('select.favtag-btn', { label: 'Pinned Regression Test' })
    await expect(page.locator('#pinned')).toBeChecked()
  })

  test('name and predicate autosave via real typing and blur', async ({ page }) => {
    const id = await createStaticCategory(page, 'Autosave Field Test')
    await page.goto('/categories')
    await page.selectOption('select.favtag-btn', { label: 'Autosave Field Test' })

    const nameInput = page.locator('tr.tag-options input').first()
    await nameInput.fill('Renamed Category')
    const [nameResponse] = await Promise.all([
      page.waitForResponse((r) => r.url().includes(`/api/categories/${id}`)),
      nameInput.blur(),
    ])
    expect(nameResponse.status()).toBe(200)

    await page.reload()
    await page.selectOption('select.favtag-btn', { label: 'Renamed Category' })
    await expect(page.locator('tr.tag-options input').first()).toHaveValue('Renamed Category')
  })
})
