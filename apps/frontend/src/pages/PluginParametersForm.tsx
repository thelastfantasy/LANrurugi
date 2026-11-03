import { useState } from "react"
import { useTranslation } from "react-i18next"

import { usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { CustomArgValue, PluginSettings } from "@/api/types"
import { NullableNumberInput } from "@/components/common-ui/Form/NumberInput"
import { cookieNameForParameter, hasCookieParameters, parseCookieHeader } from "@/lib/cookies"
import { FONT_SIZE_XS } from "@/theme"
import { toast } from "@/toast"

// Per-plugin custom-parameter settings — one input per `PluginInfo.parameters` entry. Rendered
// inside `PluginCard`'s own "插件设置" accordion, which it shares with `PluginOptionsForm`; the
// collapsible wrapper used to live here, back when this was the only collapsible section.
export function PluginParametersForm({
  namespace,
  parameters,
}: {
  namespace: string
  parameters: Array<{ name: string; desc: string; type?: string }>
}) {
  const { t } = useTranslation()
  const settings = usePluginSettings(namespace)

  if (settings.isLoading) return <p>{t("common.loading")}</p>
  if (!settings.data) return null
  return (
    <PluginParametersFormBody
      namespace={namespace}
      parameters={parameters}
      initial={settings.data}
    />
  )
}

function PluginParametersFormBody({
  namespace,
  parameters,
  initial,
}: {
  namespace: string
  parameters: Array<{ name: string; desc: string; type?: string }>
  initial: PluginSettings
}) {
  const { t } = useTranslation()
  const update = useUpdatePluginSettings(namespace)
  const [values, setValues] = useState<CustomArgValue[]>(() =>
    parameters.map((param, i) => {
      const saved = initial.customargs[i]
      if (param.type === "bool") return saved === true
      return saved ?? ""
    }),
  )
  const [cookieImportOpen, setCookieImportOpen] = useState(false)
  const [cookieText, setCookieText] = useState("")

  function setValue(index: number, value: CustomArgValue) {
    setValues((v) => v.map((existing, i) => (i === index ? value : existing)))
  }

  /** A toggle has no "still typing" state, so it persists immediately — matching the
   * "Run Automatically" switch on this same card, which has never had a save button. The whole
   * `customargs` array goes along, since that's what the endpoint replaces; `values` is stale
   * inside this closure, so the changed entry is applied here rather than read back after
   * `setValue`. */
  function setBoolAndSave(index: number, checked: boolean) {
    setValue(index, checked)
    const next = values.map((existing, i) => (i === index ? checked : existing))
    void persist(next)
  }

  /** Fills the cookie-shaped fields from a pasted `document.cookie` string, ignoring cookies the
   * form has no field for (and leaving non-cookie fields, and fields whose cookie is absent,
   * untouched). It deliberately does not save: the values still go through the form's own Save
   * button, so the user can review what was picked up first. */
  function importCookies() {
    const cookies = parseCookieHeader(cookieText)
    const matched = new Set<string>()
    let filled = 0
    const next = parameters.map((param, i) => {
      const cookieName = cookieNameForParameter(param)
      if (!cookieName) return values[i]
      const value = cookies.get(cookieName)
      if (value === undefined) return values[i]
      matched.add(cookieName)
      filled += 1
      return value
    })
    const ignored = [...cookies.keys()].filter((name) => !matched.has(name)).length
    setValues(next)
    setCookieText("")
    toast({
      text: t("pluginParameters.importCookiesResult", { filled, ignored }) ?? undefined,
      icon: filled > 0 ? "success" : "info",
    })
  }

  /** Saves and reports the outcome either way. A bare `.then(toast)` would leave a failed save's
   * rejection unhandled *and* silently unreported — worst of both, since this form's only feedback
   * is the toast. */
  async function persist(customargs: CustomArgValue[]) {
    try {
      await update.mutateAsync({ customargs })
      toast({ text: t("pluginParameters.parametersSaved") ?? undefined, icon: "success" })
    } catch {
      toast({ text: t("pluginParameters.parametersSaveFailed") ?? undefined, icon: "error" })
    }
  }

  // Only text/number parameters need an explicit Save — firing a request per keystroke isn't an
  // option, and debouncing would leave "is it saved yet?" ambiguous.
  const hasNonBoolParameters = parameters.some((param) => param.type !== "bool")

  return (
    <>
      {hasCookieParameters(parameters) && (
        <div style={{ marginBottom: 8 }}>
          <button
            type="button"
            className="stdbtn"
            onClick={() => setCookieImportOpen((open) => !open)}
          >
            <i className="fa fa-cookie-bite" aria-hidden="true"></i>{" "}
            {t("pluginParameters.importCookies")}
          </button>
          {cookieImportOpen && (
            <div style={{ marginTop: 6 }}>
              <textarea
                className="stdinput"
                style={{
                  width: "100%",
                  minHeight: 64,
                  boxSizing: "border-box",
                  fontFamily: "monospace",
                }}
                value={cookieText}
                placeholder={t("pluginParameters.importCookiesPlaceholder") ?? undefined}
                onChange={(e) => setCookieText(e.target.value)}
              />
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  marginTop: 6,
                  flexWrap: "wrap",
                }}
              >
                <button
                  type="button"
                  className="stdbtn"
                  disabled={!cookieText.trim()}
                  onClick={importCookies}
                >
                  {t("pluginParameters.importCookiesApply")}
                </button>
                <span style={{ fontSize: FONT_SIZE_XS, opacity: 0.75 }}>
                  {t("pluginParameters.importCookiesHint")}
                </span>
              </div>
            </div>
          )}
        </div>
      )}
      <table>
        <tbody>
        {parameters.map((param, i) =>
          param.type === "bool" ? (
            // class="fa" supplies the Font Awesome glyphs config.css's ON/OFF switch look needs.
            <tr key={param.name}>
              <td style={{ verticalAlign: "middle" }}>
                <b>{t(param.desc)} :</b>
              </td>
              <td>
                <input
                  type="checkbox"
                  className="fa"
                  checked={values[i] === true}
                  disabled={update.isPending}
                  onChange={(e) => setBoolAndSave(i, e.target.checked)}
                />
              </td>
            </tr>
          ) : param.type === "int" ? (
            // Legacy's own `SWITCH param.type` renders an `int` parameter as a number input
            // (`~/LANraragi/templates/plugins.html.tt2`); it was falling through to the plain-text
            // branch below along with `string`. The saved customarg stays a string — the host hands
            // the array to the plugin as-is.
            <tr key={param.name}>
              <td style={{ verticalAlign: "middle" }}>
                <b>{t(param.desc)} :</b>
              </td>
              <td>
                <NullableNumberInput
                  className="stdinput"
                  style={{ maxWidth: 200 }}
                  step={1}
                  value={values[i] === "" || values[i] === undefined ? "" : Number(values[i])}
                  onValueChange={(v) => setValue(i, v === "" ? "" : String(v))}
                  disabled={update.isPending}
                />
              </td>
            </tr>
          ) : (
            <tr key={param.name}>
              <td style={{ verticalAlign: "middle" }}>
                <b>{t(param.desc)} :</b>
              </td>
              <td>
                <input
                  style={{ maxWidth: 200 }}
                  size={20}
                  className="stdinput"
                  value={String(values[i])}
                  onChange={(e) => setValue(i, e.target.value)}
                />
              </td>
            </tr>
          ),
        )}
        {hasNonBoolParameters && (
          <tr>
            <td colSpan={2}>
              <input
                type="button"
                className="stdbtn"
                disabled={update.isPending}
                value={t("pluginParameters.savePluginSettings") ?? undefined}
                onClick={() => void persist(values)}
              />
            </td>
          </tr>
        )}
        </tbody>
      </table>
    </>
  )
}
