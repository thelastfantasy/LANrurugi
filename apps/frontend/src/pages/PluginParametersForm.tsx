import { useState } from "react"
import { useTranslation } from "react-i18next"

import { usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { CustomArgValue, PluginSettings } from "@/api/types"

// Per-plugin custom-parameter settings — one text input per `PluginInfo.parameters` entry,
// inside an accordion matching legacy's own "插件设置" panel. Distinct from `PluginOptionsForm`.
export function PluginParametersForm({
  namespace,
  parameters,
}: {
  namespace: string
  parameters: Array<{ name: string; desc: string; type?: string }>
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const settings = usePluginSettings(namespace)

  return (
    <>
      <div
        className={`collapsible-title caret-right${open ? " active" : ""}`}
        style={{ padding: "5px 0 0 5px", cursor: "pointer" }}
        onClick={() => setOpen((o) => !o)}
      >
        <i className="fas fa-sliders-h fa-2x" style={{ marginRight: 4 }} aria-hidden="true"></i>
        <b style={{ verticalAlign: "super" }}>{t("pluginParameters.pluginSettings")}</b>
      </div>
      {open && (
        <div className="collapsible-body" style={{ padding: "5px 0 0 0" }}>
          {settings.isLoading && <p>{t("common.loading")}</p>}
          {settings.data && (
            <PluginParametersFormBody
              namespace={namespace}
              parameters={parameters}
              initial={settings.data}
            />
          )}
        </div>
      )}
    </>
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
                  onChange={(e) => setValue(i, e.target.checked)}
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
        <tr>
          <td colSpan={2}>
            <input
              type="button"
              className="stdbtn"
              disabled={update.isPending}
              value={t("pluginParameters.savePluginSettings") ?? undefined}
              onClick={() => void update.mutateAsync({ customargs: values })}
            />
          </td>
        </tr>
      </tbody>
    </table>
  )
}
