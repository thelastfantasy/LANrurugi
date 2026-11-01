# UI Contract: Recycle Bin and Safety Password

New surface only; no existing screen changes layout. The delete affordances that already exist gain a
confirmation word and an undo action, which is the only behavioural change to an existing page.

## Routes and navigation

| Piece | Detail |
|---|---|
| Route | `/recycle-bin`, inside the authenticated route block next to `/bookmarks` (`App.tsx`) |
| Nav | One link in `Layout.tsx`'s authenticated `links` array and therefore in `MobileNav`'s drawer automatically |
| Page files | `pages/RecycleBin/index.tsx` (the page component only — Principle VII), `RecycleRow.tsx`, `RecycleBinToolbar.tsx`, `SafetyPasswordDialog.tsx` |
| Data | `api/recycle.ts` (types + fetchers) and TanStack Query hooks in `api/hooks.ts` (`useRecycleBin`, `useRestoreArchive`, `usePurgeEntry`, `useEmptyBin`, `useSafetyPasswordStatus`, `useSetSafetyPassword`, `useResetSafetyPassword`) |

## Recycle bin page

- Header: title, usage summary (`N entries · X GiB of 10 GiB`), retention text ("kept 30 days" / "kept
  forever"), empty-bin button (disabled when empty).
- Filter: one text input filtering title / filename / original path, client-side over the fetched list (the bin is
  bounded; no server pagination).
- Rows: thumbnail (or a placeholder when `has_thumbnail` is false), title, filename, size, page count,
  deleted-at, remaining time ("29 days left" / "kept forever"), original path (truncated, full value in a tooltip), and actions:
  `Restore`, `Delete permanently`.
- Un-restorable rows (`restorable: false`) render the reason inline (`file missing`, `an identical archive
  exists`, `unsupported entry format`) with `Restore` disabled and `Delete permanently` still available.
- Empty state: "The recycle bin is empty" plus a link back to the library.
- A `leftovers` badge appears only when the reconcile pass reports orphaned payloads, with an explanation that
  they will be cleaned automatically after the grace period.
- Every destructive control opens `SafetyPasswordDialog`; restore does not.

## Safety password dialog

- Title depends on the action (`Empty the recycle bin?`, `Delete this archive permanently?`).
- If `configured: false`: shows a "set a safety password" form (two fields, 4-digit numeric, confirm) and the
  destructive button stays disabled until the two match. Saving sets it and immediately proceeds with the action
  the user originally requested (FR-020).
- If configured: the shared `OTPField` (below) with four slots; the confirm button stays disabled until all four
  ASCII digits are present, and completion only moves focus to it — nothing auto-submits.
- Wrong PIN: inline error, field cleared, focus returned to the field, `attempts_remaining` shown.
- Locked: field and confirm disabled, countdown ("try again in 4:12") from `locked_until`, refreshed from
  `GET /api/safety-password` on reopen.
- Errors that are not PIN-related (`in progress`, `not found`, `identical archive exists`) close the dialog and
  surface through the existing toast.
- Keyboard/a11y: focus trap, Escape cancels, focus returns to the triggering button (FR-035). All strings through
  i18next.

## Shared component: `common-ui/Form/OTPField`

A single reusable segmented code field for the whole app, built on Base UI's `OTPField` rather than four styled
`<input>`s. It lives in `apps/frontend/src/components/common-ui/Form/OTPField.tsx` and is exported from
`Form/index.ts`; the safety-password dialog and the Settings section both use this one component.

| Aspect | Contract |
|---|---|
| Anatomy | `OTPField.Root` + one `OTPField.Input` per digit (+ optional `OTPField.Separator` for grouped codes); one native `<label>` binds the root's `id` |
| Props | `length` (default 4), controlled `value` / `onValueChange`, `onComplete`, `mask` (default off — the digits stay readable while typing), `disabled`, `invalid`, `autoFocus`, `label`, `slotLabel(index, total)`, `describedById` |
| Behaviour | `validationType="numeric"`; paste of the whole code fills all slots; Backspace/arrow navigation follows Base UI's own handling; completion fires `onComplete` once and moves focus to the caller's confirm button; no auto-submit |
| Accessibility | first slot carries the label, every later slot an `aria-label` from `slotLabel` (e.g. "数字 2/4"); `data-invalid` on the root plus an `aria-describedby` error text; full keyboard operability |
| Theming | every colour, size and the mobile step live in five real CSS classes appended to all five theme files (`g`, `ex`, `modern`, `modern_red`, `modern_clear`) — `.otp-field`, `.otp-field-slot`, `[data-filled]`, `:focus-visible`, `[data-invalid] .otp-field-slot`, `.otp-field-separator` — each using that theme's own accent/input/error colours. No inline `style`, no `--theme-*` transitional variables |
| Responsive | slot size and gap step down inside `@media (max-width: 560px)` in the theme CSS (a layout rule, never an inline style — Principle VII) |
| Motion | any invalid-state shake is CSS-only and suppressed under `prefers-reduced-motion: reduce` |
| Tests | Vitest/RTL: four slots, typing/paste/backspace, completion callback, disabled/invalid propagation, slot aria-labels |

## Visual and interaction quality bar

- Reuse the existing primitives (`Modal`/dialog, `Button`, `Input`, `CollapsibleSection`, the settings table
  classes) rather than bespoke markup; the bin page and Settings section must look native to the current theme.
- Every new colour-bearing class exists in all five theme files — a value that only looks right on the theme it
  was developed against is a defect, not a detail.
- Focus rings, hover and disabled states are visible in every theme; nothing relies on colour alone (the expiry
  and lock states also carry text).
- All strings go through i18next; no hardcoded English in components (English source keys with translations
  where they exist).
- Loading, empty, error and partial-failure states are designed, not left as blank rows (see States and errors).

## Delete flow on existing pages

Delete affordances in the Library context menu, the Batch page and the Edit page keep their current confirmation
dialogs, with the body text extended to say the archive moves to the recycle bin and can be restored. After a
successful delete the toast says "Moved to the recycle bin" and carries an **Undo** action that calls
`POST /api/recycle-bin/{id}/restore` for the single-archive case (batch deletes show the count and a link to the
bin instead, since one undo cannot be unambiguous). Undo failures surface the restore error verbatim (e.g. the
identical-archive conflict).

## Settings section

`pages/Settings/RecycleBinSection.tsx`, rendered as a `CollapsibleSection` (`id="recycle"`,
`icon="fa-recycle"`) on the Settings page, containing:

- `Retention` — number input in days, `0 = keep forever`, saved through the existing `PUT /settings`.
- `Capacity` — number input in MiB, `0 = unlimited`, saved the same way.
- Usage line: current entries/bytes and the computed effective limit.
- Safety password block: `Set` / `Change` (current + new + confirm) and `Reset with login password`
  (login password + confirm). Both use `SafetyPasswordDialog`'s input component, not a second implementation.
- A note that the bin never counts toward library statistics and that resetting leaves the bin unprotected until a
  new password is set.

## States and errors

| Condition | Rendering |
|---|---|
| Query loading | skeleton rows (same shape as the other admin tables) |
| Query error | inline retry row, not a blank page |
| Empty bin | empty state, no toolbar filter |
| Entry restored elsewhere (stale list) | restore/purge returns `404`; row is removed and a toast explains it was already handled |
| Lock in progress | dialog countdown; page itself unaffected |
| Any mutation | optimistic row removal only after the mutation resolves, so a failed restore leaves the row in place |
