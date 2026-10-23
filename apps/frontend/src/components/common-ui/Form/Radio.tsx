import { Radio as BaseRadio } from "@base-ui/react/radio";
import { RadioGroup as BaseRadioGroup } from "@base-ui/react/radio-group";
import type { ComponentProps } from "react";

import type { FieldSize } from "./Checkbox";
import { FIELD_CONTROL_OFFSET } from "./Checkbox";

/** Site-wide radio group, built on Base UI's `RadioGroup`/`Radio` — follows `Checkbox.tsx`'s
 * visual language rather than legacy's one-off, non-reusable `theme-switch` radio. */
export function RadioGroup<Value extends string>({
  value,
  onValueChange,
  ...rootProps
}: { value: Value; onValueChange: (value: Value) => void } & Omit<
  ComponentProps<typeof BaseRadioGroup>,
  "value" | "onValueChange" | "className" | "children"
> & { children: React.ReactNode }) {
  return (
    <BaseRadioGroup
      value={value}
      onValueChange={(v) => onValueChange(v as Value)}
      {...rootProps}
    />
  );
}

/** One option within a `RadioGroup`. `disabled` is read explicitly (and still forwarded) so this
 * wrapping `<label>` (not a Base UI component, gets no `data-disabled`) gets correct styling.
 * `size` picks a `marginTop` calibrated for that font-size, shared with `CheckboxField` (see that
 * component's own doc comment) so a radio row and a checkbox row in the same menu/list align the
 * same way. Pass `"inherit"` (default) when the surrounding text isn't at one of this project's
 * own `FONT_SIZE_*` sizes. */
export function RadioItem({
  value,
  disabled,
  size = "inherit",
  children,
  ...rootProps
}: { value: string; size?: FieldSize; children: React.ReactNode } & Omit<
  ComponentProps<typeof BaseRadio.Root>,
  "value" | "className" | "children"
>) {
  return (
    <label
      className={`inline-flex items-start gap-[6px] align-middle ${
        disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"
      }`}
    >
      <BaseRadio.Root
        value={value}
        disabled={disabled}
        className="common-form-control-bg common-radio-bg inline-flex size-[13px] shrink-0 items-center justify-center rounded-full border border-solid border-current p-0 outline-none data-checked:border-[#1a73e8]"
        style={{ marginTop: FIELD_CONTROL_OFFSET[size] }}
        {...rootProps}
      >
        <BaseRadio.Indicator
          className="size-[7px] rounded-full bg-[#1a73e8]"
          keepMounted={false}
        />
      </BaseRadio.Root>
      {/* A bare text node participating directly in this `<label>`'s `inline-flex` doesn't get a
          real box to align by (no line-height/geometry of its own to match against the radio
          circle's `size-[13px]` box) — wrapping it here is what actually makes `items-start` (or
          any `align-items`) apply consistently, not the `marginTop` nudge alone. */}
      <span>{children}</span>
    </label>
  );
}
