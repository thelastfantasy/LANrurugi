import { useTranslation } from "react-i18next"

import { usePlugins } from "@/api/hooks"
import { PluginOptionsForm } from "@/pages/PluginOptionsForm"
import { PluginParametersForm } from "@/pages/PluginParametersForm"

/** Shows and edits the associated login plugin's real, already-installed settings, saved straight
 * back to that plugin's own Redis entry — distinct from this draft's own trial-run parameters. */
export function AssociatedLoginPluginSettings({ filePathNamespace }: { filePathNamespace: string | undefined }) {
  const { t } = useTranslation()
  const plugins = usePlugins("login")
  const plugin = plugins.data?.find((p) => p.namespace === filePathNamespace)
  if (!plugin) return null

  return (
    <div className="ptbox" style={{ padding: 8, marginBottom: 8 }}>
      <h3 className="ih">{t("pluginWizard.associatedLoginPluginSettingsHeading", { name: plugin.name })}</h3>
      {plugin.parameters.length > 0 && (
        <PluginParametersForm namespace={plugin.namespace} parameters={plugin.parameters} />
      )}
      {plugin.type === "download" && <PluginOptionsForm namespace={plugin.namespace} />}
    </div>
  )
}
