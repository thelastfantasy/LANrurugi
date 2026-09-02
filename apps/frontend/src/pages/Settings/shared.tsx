import type { ReactNode } from "react"

import { Switch } from "@/components/common-ui/Form/Switch"

export function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings-row">
      <div className="option-td">
        {/* Legacy's `c.lh()` emits raw HTML — a few real labels embed their own `<br>` (e.g.
            "Maximum <br>Cache Size"), so this has to render as HTML, not escaped text. */}
        <h2 className="ih" dangerouslySetInnerHTML={{ __html: ` ${label} ` }} />
      </div>
      <div className="config-td">{children}</div>
    </div>
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
  /** Greys out the switch without hiding the row — for a sub-option whose parent is off. */
  disabled?: boolean
  /** Left-indents this row's label to read as "belongs to the row above". */
  indent?: boolean
  children: ReactNode
}) {
  return (
    <div className="settings-row">
      <div className="option-td" style={indent ? { paddingLeft: 24 } : undefined}>
        <h2 className="ih"> {label} </h2>
      </div>
      <div className="config-td">
        <Switch id={id} checked={checked} onCheckedChange={onChange} disabled={disabled} />
        <label htmlFor={id}>
          <br /> {children}
        </label>
      </div>
    </div>
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
    <div className="settings-row">
      <div className="option-td">
        <input id={id} className="stdbtn" type="button" value={label} onClick={onClick} />
      </div>
      <div className="config-td">{children}</div>
    </div>
  )
}
