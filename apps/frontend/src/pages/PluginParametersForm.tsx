import { useState } from "react"
import { useTranslation } from "react-i18next"

import { usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { CustomArgValue, PluginSettings } from "@/api/types"
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
  )
}
