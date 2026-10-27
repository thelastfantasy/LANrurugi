import { Radio as BaseRadio } from "@base-ui/react/radio";
import { RadioGroup as BaseRadioGroup } from "@base-ui/react/radio-group";
import type { ComponentProps } from "react";

/** Site-wide radio group, built on Base UI's `RadioGroup`/`Radio` — follows `Checkbox.tsx`'s
 * visual language rather than legacy's one-off, non-reusable `theme-switch` radio.
 *
 * The group deliberately carries no layout of its own: a group in a menu is a vertical list, one
 * in a form is often a row, so `className` is how a caller lays it out (the same way `Button` and
 * `Input` take one). Without it, callers were hand-rolling inline `style` just to get a gap
 * between the options, or — worse — wrapping the group in their own flex container, which leaves
 * the options themselves with no spacing at all. */
export function RadioGroup<Value extends string>({
  value,
  onValueChange,
  className,
  ...rootProps
}: {
  value: Value
  onValueChange: (value: Value) => void
  className?: string
} & Omit<ComponentProps<typeof BaseRadioGroup>, "value" | "onValueChange" | "className" | "children"> & {
  children: React.ReactNode
}) {
  return (
    <BaseRadioGroup
      className={className}
      value={value}
      onValueChange={(v) => onValueChange(v as Value)}
      {...rootProps}
    />
  )
}

/** One option within a `RadioGroup`. `disabled` is read explicitly (and still forwarded) so this
 * wrapping `<label>` (not a Base UI component, gets no `data-disabled`) gets correct styling.
 *
 * The circle is centred against the label instead of nudged down by a per-size offset: it is a
 * plain box, so `align-items: center` is exact for whatever font/line-height it inherits, while
 * the old `marginTop` table only held for this project's own `FONT_SIZE_*` contexts — measured
 * 2.5px off in the share dialog, whose text is legacy-sized. */
export function RadioItem({
  value,
  disabled,
  children,
  ...rootProps
}: { value: string; children: React.ReactNode } & Omit<
  ComponentProps<typeof BaseRadio.Root>,
  "value" | "className" | "children"
>) {
  return (
    <label
      className={`inline-flex items-center gap-[6px] align-middle ${
        disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"
      }`}
    >
      <BaseRadio.Root
        value={value}
        disabled={disabled}
        className="common-form-control-bg common-radio-bg inline-flex size-[13px] shrink-0 items-center justify-center rounded-full border border-solid border-current p-0 outline-none data-checked:border-[#1a73e8]"
        {...rootProps}
      >
        <BaseRadio.Indicator
          className="size-[7px] rounded-full bg-[#1a73e8]"
          keepMounted={false}
        />
      </BaseRadio.Root>
      {/* A bare text node participating directly in this `<label>`'s `inline-flex` doesn't get a
          real box to align by — wrapping it here is what makes `align-items` apply. */}
      <span>{children}</span>
    </label>
  );
}
