import { Checkbox as BaseCheckbox } from "@base-ui/react/checkbox";
import type { ComponentProps, ReactNode } from "react";
import { FaCheck } from "react-icons/fa6";

/** Site-wide checkbox, built on Base UI's `Checkbox` — a small square box with a `FaCheck`
 * checkmark, unlike `Switch.tsx`'s icon-toggle. `common-form-control-bg` avoids a transparent box. */
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
      className="common-form-control-bg relative inline-flex size-[13px] shrink-0 -translate-y-[2px] cursor-pointer items-center justify-center border border-solid border-current p-0 align-middle outline-none data-checked:border-[#1a73e8] data-checked:bg-[#1a73e8] data-checked:text-white"
      {...rootProps}
    >
      <BaseCheckbox.Indicator
        className="flex items-center justify-center text-[11px] leading-none"
        keepMounted={false}
      >
        <FaCheck aria-hidden="true" />
      </BaseCheckbox.Indicator>
    </BaseCheckbox.Root>
  );
}

/** Per-`size` vertical nudge on the control so its visual center lines up with the label text's
 * — a browser renders a native-styled control's own box slightly off from a same-line text
 * baseline, and by how much depends on the text's line-height (which depends on font-size), not
 * something `align-items: center`/`align-middle` alone corrects for. Measured live via
 * `getBoundingClientRect` against each of this project's own `FONT_SIZE_*` constants (not
 * calculated/assumed) — see the `CheckboxField`/`RadioField` doc comment for how to add a size. */
export const FIELD_CONTROL_OFFSET = {
  xs: 0,
  sm: 1,
  md: 1,
  inherit: 1,
} as const;

export type FieldSize = keyof typeof FIELD_CONTROL_OFFSET;

/** `Checkbox` pre-wrapped with a label, using the same `<label>` + `inline-flex items-start` +
 * per-control `marginTop` pattern `RadioField` (`Radio.tsx`) already uses — the two exist
 * together so a checkbox row and a radio row in the same menu/list use one shared alignment
 * mechanism instead of each call site improvising its own (`align-items: center` alone doesn't
 * correct for a native control's own rendered offset from the label text's baseline, which is
 * what caused this pair to exist — see git history/PR discussion around 2026-08-31 if this
 * comment ever goes stale). `size` picks a `marginTop` calibrated for that font-size; pass
 * `"inherit"` (default) when the surrounding text isn't at one of this project's own
 * `FONT_SIZE_*` sizes.
 *
 * `checked`/`onCheckedChange` is already the shape a form library's controlled-input adapter
 * expects (e.g. react-hook-form's `Controller`: `render={({ field }) => <CheckboxField
 * checked={field.value} onCheckedChange={field.onChange} />}`) — this isn't a native `<input>`,
 * so `register()`'s own uncontrolled-ref mode doesn't apply; go through `Controller` instead. */
export function CheckboxField({
  checked,
  onCheckedChange,
  disabled,
  size = "inherit",
  children,
  ...rootProps
}: {
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  disabled?: boolean
  size?: FieldSize
  children: ReactNode
} & Omit<ComponentProps<typeof BaseCheckbox.Root>, "checked" | "onCheckedChange" | "className" | "children" | "disabled">) {
  return (
    <label
      className={`inline-flex items-start gap-[6px] align-middle ${
        disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"
      }`}
    >
      <BaseCheckbox.Root
        checked={checked}
        onCheckedChange={onCheckedChange}
        disabled={disabled}
        className="common-form-control-bg inline-flex size-[13px] shrink-0 items-center justify-center border border-solid border-current p-0 outline-none data-checked:border-[#1a73e8] data-checked:bg-[#1a73e8] data-checked:text-white"
        style={{ marginTop: FIELD_CONTROL_OFFSET[size] }}
        {...rootProps}
      >
        <BaseCheckbox.Indicator className="flex items-center justify-center text-[11px] leading-none" keepMounted={false}>
          <FaCheck aria-hidden="true" />
        </BaseCheckbox.Indicator>
      </BaseCheckbox.Root>
      {/* A bare text node participating directly in this `<label>`'s `inline-flex` doesn't get a
          real box to align by (no line-height/geometry of its own to match against the checkbox's
          `size-[13px]` box) — wrapping it here is what actually makes `items-start` (or any
          `align-items`) apply consistently, not the `marginTop` nudge alone. */}
      <span>{children}</span>
    </label>
  );
}
