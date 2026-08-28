import { Radio as BaseRadio } from "@base-ui/react/radio";
import { RadioGroup as BaseRadioGroup } from "@base-ui/react/radio-group";
import type { ComponentProps } from "react";

/** Site-wide radio group, built on Base UI's `RadioGroup`/`Radio` rather than legacy's own real
 * mechanism — legacy has exactly one radio-button usage anywhere (`config_theme.html.tt2`'s theme
 * picker, ported as `SettingsPage.tsx`'s own raw `<input type="radio" className="theme-switch">`),
 * and that one is a special case: the native radio input itself is visually hidden, with
 * `.theme-switch`'s real CSS styling the *card* around it (a theme preview thumbnail) rather than
 * a generic small circular indicator — not a reusable pattern for an ordinary "pick one of these
 * N options" control like the reader's own `kBehavior` setting needs. This component instead
 * follows `Checkbox.tsx`'s own visual language (a small bordered shape using `currentColor`, no
 * new colors introduced) — a filled circle instead of a checkmark-in-a-square, the standard visual
 * distinction between "pick one" and "toggle this on/off" controls.
 *
 * `RadioGroup`'s own `value`/`onValueChange` are generic over the value type (`RadioGroup<Value>`)
 * the same way `Select.tsx`'s `value extends string` type param is — narrowed to `string` here
 * since every call site so far uses string-literal option values, matching `Select.tsx`'s own
 * choice not to plumb through Base UI's fuller generic for a case with no current use. */
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

/** One option within a `RadioGroup` — pairs `Radio.Root` (the circular indicator) with its own
 * label text, following the exact `<label>`-wraps-`Radio.Root`-plus-text structure base-ui.com's
 * own documented example uses (Base UI's `Radio.Root` renders its own hidden `<input>` internally
 * and associates it via the wrapping `<label>`'s native click-through behavior, so no explicit
 * `id`/`htmlFor` pair is needed the way the legacy `theme-switch` input above required one).
 *
 * `disabled` is read explicitly (not left inside `...rootProps` alone) purely for this wrapping
 * `<label>`'s own cursor/opacity styling — a plain `<label>` isn't a Base UI component and never
 * receives Base UI's own `data-disabled` attribute the way `Radio.Root` itself does, so without
 * reading it here the label's cursor would stay `pointer` and its text full-opacity even while the
 * radio it wraps is genuinely disabled. Still forwarded on to `Radio.Root` via `...rootProps` too
 * (not consumed/stripped) — this is purely an *additional* read, not a narrowing, matching every
 * other prop in `...rootProps` passing through unchanged.
 *
 * Note this only covers a `disabled` prop passed directly to *this* `RadioItem` — `RadioGroup`'s
 * own `disabled` (which base-ui.com's docs confirm does propagate down into every `Radio.Root`'s
 * own Base-UI-internal state/`data-disabled`) does *not* reach this label-level styling, since
 * that propagation happens inside Base UI's own context, invisible to this wrapper. No current
 * call site sets `disabled` at the `RadioGroup` level (only per-item, if at all), so this gap is
 * not exercised in practice — flagged here rather than silently assumed correct. */
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
        className="common-form-control-bg inline-flex size-[13px] shrink-0 items-center justify-center rounded-full border border-solid border-current p-0 outline-none"
        {...rootProps}
      >
        <BaseRadio.Indicator
          className="size-[7px] rounded-full bg-current"
          keepMounted={false}
        />
      </BaseRadio.Root>
      {children}
    </label>
  );
}
