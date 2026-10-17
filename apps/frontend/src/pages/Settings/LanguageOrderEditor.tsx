import { useTranslation } from "react-i18next"

import { Tooltip } from "@/components/common-ui/Display"
import { SUPPORTED_LANGUAGES } from "@/i18n"

import { ICON_BUTTON_STYLE } from "../Upload/shared"

/** Edits the ordered language preference, stored comma-separated in the one `language` setting.
 *
 * An order rather than a single choice because one answer rarely covers both jobs it is asked to do:
 * the interface needs a language this build has translations for, while a work's title needs one its
 * source happened to supply. A list lets both fall through to the next acceptable answer instead of
 * jumping straight to English. */
export function LanguageOrderEditor({
  value,
  onChange,
}: {
  /** Comma-separated codes, or `"auto"` to defer to the browser. */
  value: string
  onChange: (next: string) => void
}) {
  const { t } = useTranslation()
  const isAuto = value === "auto" || value.trim() === ""
  const order = isAuto
    ? []
    : value
        .split(",")
        .map((c) => c.trim())
        .filter(Boolean)

  const nameOf = (code: string) =>
    SUPPORTED_LANGUAGES.find((l) => l.code === code)?.nativeName ?? code
  const unused = SUPPORTED_LANGUAGES.filter((l) => !order.includes(l.code))

  // Empty means nothing is preferred, which is what `"auto"` says — writing an empty string instead
  // would read as a preference for nothing at all.
  const commit = (next: string[]) => onChange(next.length > 0 ? next.join(",") : "auto")

  const move = (i: number, by: number) => {
    const next = [...order]
    const j = i + by
    if (j < 0 || j >= next.length) return
    ;[next[i], next[j]] = [next[j], next[i]]
    commit(next)
  }

  return (
    <div className="lang-order">
      <label style={{ display: "block", marginBottom: 6 }}>
        <input
          type="checkbox"
          className="fa"
          checked={isAuto}
          onChange={(e) => commit(e.target.checked ? [] : [SUPPORTED_LANGUAGES[0].code])}
        />{" "}
        {t("settings.automaticBrowserDefault")}
      </label>

      {!isAuto && (
        <>
          <ol className="lang-order-list">
            {order.map((code, i) => (
              <li key={code} className="lang-order-item">
                <span className="lang-order-name">{nameOf(code)}</span>
                <Tooltip label={t("settings.moveUp") ?? ""}>
                  <button
                    type="button"
                    className="stdbtn"
                    style={ICON_BUTTON_STYLE}
                    aria-label={t("settings.moveUp") ?? "Move up"}
                    disabled={i === 0}
                    onClick={() => move(i, -1)}
                  >
                    <i className="fa fa-arrow-up" aria-hidden="true"></i>
                  </button>
                </Tooltip>
                <Tooltip label={t("settings.moveDown") ?? ""}>
                  <button
                    type="button"
                    className="stdbtn"
                    style={ICON_BUTTON_STYLE}
                    aria-label={t("settings.moveDown") ?? "Move down"}
                    disabled={i === order.length - 1}
                    onClick={() => move(i, 1)}
                  >
                    <i className="fa fa-arrow-down" aria-hidden="true"></i>
                  </button>
                </Tooltip>
                <Tooltip label={t("settings.removeLanguage") ?? ""}>
                  <button
                    type="button"
                    className="stdbtn"
                    style={ICON_BUTTON_STYLE}
                    aria-label={t("settings.removeLanguage") ?? "Remove"}
                    onClick={() => commit(order.filter((_, j) => j !== i))}
                  >
                    <i className="fa fa-times" aria-hidden="true"></i>
                  </button>
                </Tooltip>
              </li>
            ))}
          </ol>

          {unused.length > 0 && (
            <select
              className="stdinput"
              value=""
              aria-label={t("settings.addLanguage") ?? "Add a language"}
              onChange={(e) => e.target.value && commit([...order, e.target.value])}
            >
              <option value="">{t("settings.addLanguage")}</option>
              {unused.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.nativeName}
                </option>
              ))}
            </select>
          )}

          <p className="sub-form-hint">{t("settings.languageOrderHint")}</p>
        </>
      )}
    </div>
  )
}
