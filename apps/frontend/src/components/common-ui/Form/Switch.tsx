import { Switch as BaseSwitch } from "@base-ui/react/switch"
import type { ComponentProps } from "react"
import { FaToggleOff, FaToggleOn } from "react-icons/fa6"

/** Site-wide ON/OFF toggle, built on Base UI's `Switch` rather than legacy's own real mechanism
 * (`public/legacy/config.css`'s `input[type='checkbox']` rule, which uses `appearance: none` plus
 * `::before`/`::after` content to turn a native checkbox into a Font Awesome toggle-icon glyph +
 * "ON"/"OFF" text — a CSS hack, not a real switch element, and the same kind of native-form-
 * control-vs-CSS-override gap that made `common-ui/Form/Select.tsx`'s plain `<select>` render
 * inconsistently). `FaToggleOn`/`FaToggleOff` (`react-icons/fa6`) render real SVGs of the exact
 * same Font Awesome 6 glyphs `config.css` referenced by codepoint (`\f204`/`\f205`) — same
 * icon-font-vs-SVG centering/crispness reasoning as `BookmarkHoverGrid.tsx`'s own `FaTrashCan`
 * precedent, not a different icon language. Colors reused verbatim from each theme's own real
 * `input[type='checkbox']`/`:checked` rule via the `.lrr-switch`/`.lrr-switch[data-checked]`
 * classes (see `public/legacy/themes/*.css`, one rule per file) — no new colors introduced.
 *
 * `...rootProps` forwards the rest of `Switch.Root`'s own real props (`disabled`, `name`,
 * `required`, `readOnly`, `form`, `value`/`uncheckedValue`, `inputRef`, ...) straight through —
 * same "wrap, don't narrow" shape as `Button.tsx`/`Select.tsx`'s own `Omit<ComponentProps<...>>`
 * pattern, so a caller never hits a missing prop this wrapper didn't happen to anticipate. */
export function Switch({
  checked,
  onCheckedChange,
  ...rootProps
}: { checked: boolean; onCheckedChange: (checked: boolean) => void } & Omit<
  ComponentProps<typeof BaseSwitch.Root>,
  "checked" | "onCheckedChange" | "className" | "children"
>) {
  return (
    <BaseSwitch.Root
      checked={checked}
      onCheckedChange={onCheckedChange}
      className="lrr-switch inline-flex cursor-pointer items-center gap-1 border-none bg-transparent p-0 font-bold outline-none data-disabled:cursor-not-allowed data-disabled:opacity-50"
      {...rootProps}
    >
      {checked ? <FaToggleOn size={20} aria-hidden="true" /> : <FaToggleOff size={20} aria-hidden="true" />}
      <span className="text-[15px]">{checked ? "ON" : "OFF"}</span>
    </BaseSwitch.Root>
  )
}
