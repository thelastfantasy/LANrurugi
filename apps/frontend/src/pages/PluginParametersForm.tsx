import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { usePluginSettings, useUpdatePluginSettings } from '../api/hooks'
import type { PluginSettings } from '../api/types'

// Per-plugin custom-parameter settings (e.g. E-Hentai login's cookie fields) — one text input per
// `PluginInfo.parameters` entry, dynamically rendered from the plugin's own declared metadata,
// rendered inside a real nested accordion matching legacy's own "插件设置" panel
// (`~/LANraragi/templates/plugins.html.tt2` lines 181-208, `.collapsible-title`/`.collapsible-body`
// with the `fa-sliders-h` icon). Distinct from `PluginOptionsForm` (download-specific
// concurrency/rate-limit/bundling settings).
export default function PluginParametersForm({
  namespace,
  parameters,
}: {
  namespace: string
  parameters: Array<{ name: string; desc: string }>
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const settings = usePluginSettings(namespace)

  return (
    <>
      <div
        className={`collapsible-title${open ? ' active' : ''}`}
        style={{ padding: '5px 0 0 5px', cursor: 'pointer' }}
        onClick={() => setOpen((o) => !o)}
      >
        <i className="fas fa-sliders-h fa-2x" style={{ marginRight: 4 }} aria-hidden="true"></i>
        <b style={{ verticalAlign: 'super' }}>{t('Plugin Settings')}</b>
      </div>
      {open && (
        <div className="collapsible-body" style={{ padding: '5px 0 0 0' }}>
          {settings.isLoading && <p>{t('Loading…')}</p>}
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
  parameters: Array<{ name: string; desc: string }>
  initial: PluginSettings
}) {
  const { t } = useTranslation()
  const update = useUpdatePluginSettings(namespace)
  const [values, setValues] = useState<string[]>(() =>
    parameters.map((_, i) => initial.customargs[i] ?? ''),
  )

  function setValue(index: number, value: string) {
    setValues((v) => v.map((existing, i) => (i === index ? value : existing)))
  }

  return (
    <table>
      <tbody>
        {parameters.map((param, i) => (
          <tr key={param.name}>
            <td style={{ verticalAlign: 'middle' }}>
              <b>{t(param.desc)} :</b>
            </td>
            <td>
              <input
                style={{ maxWidth: 200 }}
                size={20}
                className="stdinput"
                value={values[i]}
                onChange={(e) => setValue(i, e.target.value)}
              />
            </td>
          </tr>
        ))}
        <tr>
          <td colSpan={2}>
            <input
              type="button"
              className="stdbtn"
              disabled={update.isPending}
              value={t('Save Plugin Settings') ?? undefined}
              onClick={() => void update.mutateAsync({ customargs: values })}
            />
          </td>
        </tr>
      </tbody>
    </table>
  )
}
