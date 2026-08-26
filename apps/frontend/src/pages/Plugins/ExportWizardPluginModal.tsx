import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useExportPluginsBatch, usePlugins } from "@/api/hooks"
import { Modal } from "@/components/Display"

/** Checkbox picker for exporting several AI-wizard-generated plugins as one `.zip` — only plugins
 * with `generated_by_wizard: true` are listed, since a hand-written/converted plugin already lives
 * in this repo's own `plugins/` tree and has no real need for this download. Defaults to all
 * selected, since exporting everything is the common case. */
export function ExportWizardPluginModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const plugins = usePlugins("all")
  const wizardPlugins = (plugins.data ?? []).filter((p) => p.generated_by_wizard)
  const [selected, setSelected] = useState<Set<string> | null>(null)
  // Adjust state during render (not an effect) — selects all exactly once, the first time
  // wizardPlugins has real entries; `selected !== null` afterward prevents this from re-running.
  if (selected === null && wizardPlugins.length > 0) {
    setSelected(new Set(wizardPlugins.map((p) => p.namespace)))
  }
  const effectiveSelected = selected ?? new Set<string>()
  const exportBatch = useExportPluginsBatch()

  function toggle(namespace: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(namespace)) next.delete(namespace)
      else next.add(namespace)
      return next
    })
  }

  async function handleExport() {
    const { blob, filename } = await exportBatch.mutateAsync([...effectiveSelected])
    const url = URL.createObjectURL(blob)
    const a = document.createElement("a")
    a.href = url
    a.download = filename ?? "plugins.zip"
    a.click()
    URL.revokeObjectURL(url)
    onClose()
  }

  return (
    <Modal onClose={onClose} width={480}>
      <h2 className="ih">{t("plugins.exportAsZip")}</h2>
      <p>{t("plugins.exportWizardPluginsDescription")}</p>
      {wizardPlugins.length === 0 ? (
        <p>{t("plugins.noWizardPluginsToExport")}</p>
      ) : (
        <>
          <table style={{ width: "100%" }}>
            <tbody>
              {wizardPlugins.map((plugin) => (
                <tr key={plugin.namespace}>
                  <td style={{ verticalAlign: "middle", textAlign: "left" }}>
                    <b>
                      {plugin.name} v.{plugin.version}
                    </b>
                  </td>
                  <td style={{ textAlign: "right" }}>
                    <input
                      type="checkbox"
                      className="fa"
                      checked={effectiveSelected.has(plugin.namespace)}
                      onChange={() => toggle(plugin.namespace)}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <input
            type="button"
            className="stdbtn"
            disabled={effectiveSelected.size === 0 || exportBatch.isPending}
            onClick={() => void handleExport()}
            value={t("plugins.exportAsZip") ?? undefined}
          />
        </>
      )}
    </Modal>
  )
}
