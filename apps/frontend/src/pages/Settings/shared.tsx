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
  children,
}: {
  id: string
  checked: boolean
  onChange: (v: boolean) => void
  label: string
  children: ReactNode
}) {
  return (
    <tr>
      <td className="option-td">
        <h2 className="ih"> {label} </h2>
      </td>
      <td className="config-td">
        <Switch id={id} checked={checked} onCheckedChange={onChange} />
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
