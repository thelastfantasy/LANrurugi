import { useTranslation } from "react-i18next"

import { usePlugins } from "@/api/hooks"
import { PluginOptionsForm } from "@/pages/PluginOptionsForm"
import { PluginParametersForm } from "@/pages/PluginParametersForm"

/** A metadata/download draft's own associated login plugin's *real, currently-installed* settings
 * — both its declared `pluginInfo().parameters` (customargs, e.g. an API key) and, if it's itself
 * a download plugin somehow (unusual but not impossible), its `pluginOptions()`. Editable directly
 * here and saved straight back to that OTHER plugin's own `LRR_PLUGIN_<NS>` Redis entry — this
 * session's own confirmed requirement ("可以直接在这里编辑并保存回登录插件自己的设置", 2026-08-25),
 * distinct from the current draft's own trial-run parameter panel (`TrialRunResult.tsx`'s own
 * `declaredParameters`), which saves to *this* draft's future namespace only once confirm-save
 * runs. Renders nothing until `usePlugins("login")` actually finds `filePathNamespace` in the real
 * installed list — a namespace that resolves to nothing yet (e.g. this session generated a login
 * plugin but hasn't saved it) has no real settings to show. */
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
