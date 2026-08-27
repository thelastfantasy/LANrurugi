import { Checkbox as BaseCheckbox } from "@base-ui/react/checkbox"
import type { ComponentProps } from "react"
import { FaCheck } from "react-icons/fa6"

/** Site-wide checkbox, built on Base UI's `Checkbox` rather than a native `<input
 * type="checkbox">` — legacy's own real checkboxes (e.g. Categories.tsx's `pinned` field, `<input
 * type="checkbox" className="fa">`) turned out, once actually measured live in a browser
 * (`getComputedStyle`, 2026-08-27), to render as a completely unstyled native browser checkbox:
 * `appearance: auto`, no border/border-radius/background of its own, 13×13px. `config.css`'s own
 * `input[type='checkbox']` rule (the one that turns `Switch.tsx`'s underlying element into an
 * ON/OFF icon toggle) does NOT apply here — an earlier assumption that this checkbox was secretly
 * already an ON/OFF-style switch was wrong, confirmed and corrected by that same live check. So
 * this component's job is different from `Switch.tsx`'s: reproduce a small square box with a
 * checkmark when ticked, not an icon-toggle. `FaCheck` (react-icons/fa6, a real SVG) for the
 * checkmark — same icon-font-vs-SVG reasoning as every other icon in this directory
 * (`Switch.tsx`/`Select.tsx`'s own docs), not a new precedent.
 *
 * `...rootProps` forwards the rest of `Checkbox.Root`'s own real props (`disabled`, `name`,
 * `required`, `readOnly`, `form`, `value`/`uncheckedValue`, `inputRef`, ...) straight through —
 * same "wrap, don't narrow" shape as this directory's other components. */
export function Checkbox({
  checked,
  onCheckedChange,
  ...rootProps
}: { checked: boolean; onCheckedChange: (checked: boolean) => void } & Omit<
  ComponentProps<typeof BaseCheckbox.Root>,
  "checked" | "onCheckedChange" | "className" | "children"
>) {
  return (
    <BaseCheckbox.Root
      checked={checked}
      onCheckedChange={onCheckedChange}
      className="inline-flex size-[13px] shrink-0 cursor-pointer items-center justify-center border border-solid border-current bg-transparent p-0 align-middle outline-none"
      {...rootProps}
    >
      <BaseCheckbox.Indicator className="flex items-center justify-center text-[11px] leading-none" keepMounted={false}>
        <FaCheck aria-hidden="true" />
      </BaseCheckbox.Indicator>
    </BaseCheckbox.Root>
  )
}
