import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { Switch } from "@/components/common-ui/Form/Switch"
import type { ImageFormatSupport } from "@/lib/utils/imageFormatSupport"

export function Row({
  label,
  children,
  noHelp,
}: {
  label: string
  children: ReactNode
  /** This row's `.config-td` is only the control itself, with no helper text below it — vertically
   * centers the label against it instead of top-aligning (the default, which assumes every row is
   * tall enough to align against). */
  noHelp?: boolean
}) {
  return (
    <div className={noHelp ? "settings-row settings-row--no-help" : "settings-row"}>
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
  disabled,
  children,
}: {
  id: string
  label: string
  onClick: () => void
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <div className="settings-row">
      <div className="option-td">
        <input id={id} className="stdbtn" type="button" disabled={disabled} value={label} onClick={onClick} />
      </div>
      <div className="config-td">{children}</div>
    </div>
  )
}

/** Small high-contrast status badge for the one-time browser image-format detection result. */
export function ImageSupportBadge({ support }: { support: ImageFormatSupport }) {
  const { t } = useTranslation()
  const isJxl = support === "jxl"
  const isSource = support === "source"
  const label = isJxl
    ? t("settings.browserFormatJxl")
    : support === "webp"
      ? t("settings.browserFormatWebp")
      : t("settings.browserFormatSource")
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        padding: "2px 7px",
        borderRadius: 999,
        background: isJxl
          ? "rgba(26, 115, 232, 0.12)"
          : isSource
            ? "rgba(220, 38, 38, 0.10)"
            : "rgba(245, 158, 11, 0.16)",
        color: isJxl ? "#1a73e8" : isSource ? "#dc2626" : "#b45309",
        fontWeight: 700,
        lineHeight: 1.4,
      }}
    >
      <i
        className={`fas ${
          isJxl ? "fa-bolt" : isSource ? "fa-triangle-exclamation" : "fa-image"
        }`}
        style={{ fontSize: 9 }}
      />
      {label}
    </span>
  )
}
