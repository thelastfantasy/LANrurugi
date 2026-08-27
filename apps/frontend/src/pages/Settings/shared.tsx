import type { ReactNode } from "react"

import { Switch } from "@/components/common-ui/Form/Switch"

export function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <tr>
      <td className="option-td">
        {/* Legacy's `c.lh()` emits raw HTML — a few real labels embed their own `<br>` (e.g.
            "Maximum <br>Cache Size"), so this has to render as HTML, not escaped text. */}
        <h2 className="ih" dangerouslySetInnerHTML={{ __html: ` ${label} ` }} />
      </td>
      <td className="config-td">{children}</td>
    </tr>
  )
}

export function CheckboxRow({
  id,
  checked,
  onChange,
  label,
  disabled,
  indent,
  children,
}: {
  id: string
  checked: boolean
  onChange: (v: boolean) => void
  label: string
  /** issue #97: greys out the switch (via `Switch.tsx`'s own `data-disabled` variant) without
   * hiding the row — used for a sub-option whose parent switch is currently off, so the sub-
   * option's existence/current value both stay visible rather than disappearing. */
  disabled?: boolean
  /** issue #97: left-indents this row's label to visually read as "belongs to the row above" —
   * paired with `disabled` for a parent/child switch pair, but independently usable. */
  indent?: boolean
  children: ReactNode
}) {
  return (
    <tr>
      <td className="option-td" style={indent ? { paddingLeft: 24 } : undefined}>
        <h2 className="ih"> {label} </h2>
      </td>
      <td className="config-td">
        <Switch id={id} checked={checked} onCheckedChange={onChange} disabled={disabled} />
        <label htmlFor={id}>
          <br /> {children}
        </label>
      </td>
    </tr>
  )
}

export function ActionRow({
  id,
  label,
  onClick,
  children,
}: {
  id: string
  label: string
  onClick: () => void
  children: ReactNode
}) {
  return (
    <tr>
      <td className="option-td">
        <input id={id} className="stdbtn" type="button" value={label} onClick={onClick} />
      </td>
      <td className="config-td">{children}</td>
    </tr>
  )
}
