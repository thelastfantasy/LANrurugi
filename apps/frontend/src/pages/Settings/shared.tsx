import type { ReactNode } from "react"

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
        <input id={id} name={id} className="fa" type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
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
