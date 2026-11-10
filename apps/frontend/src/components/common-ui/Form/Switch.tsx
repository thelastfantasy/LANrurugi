import { Switch as BaseSwitch } from "@base-ui/react/switch"
import type { ComponentProps } from "react"
import { FaToggleOff, FaToggleOn } from "react-icons/fa6"

/** Site-wide ON/OFF toggle, built on Base UI's `Switch`. `FaToggleOn`/`FaToggleOff` render real
 * SVGs of legacy's codepoint icons, colored via each theme's `.lrr-switch` rule; legacy also prints
 * "ON"/"OFF" beside the icon, so the same text is rendered here. Both are `shrink-0` because the
 * root is a flex box: squeezed inline it would otherwise collapse the text onto whatever follows. */
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
      {checked ? (
        <FaToggleOn size={20} className="shrink-0" aria-hidden="true" />
      ) : (
        <FaToggleOff size={20} className="shrink-0" aria-hidden="true" />
      )}
      <span className="shrink-0 text-[15px]">{checked ? "ON" : "OFF"}</span>
    </BaseSwitch.Root>
  )
}
