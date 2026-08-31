import { useQueryClient } from "@tanstack/react-query"
import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { usePlugins, useSettings, useUpdateSettings } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { CollapsibleSection } from "@/components/Display"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { useSectionDeepLink } from "@/hooks/useSectionDeepLink"
import { routes } from "@/lib/routes"
import { useApplyTheme, useLegacyConfigCss } from "@/theme"

import { ExportWizardPluginModal } from "./ExportWizardPluginModal"
import { SortablePluginGroup } from "./SortablePluginGroup"

// Legacy's own left/right split: left is Login/Downloaders/Scripts, right is Metadata Plugins.
const LEFT_GROUPS: Array<{ type: PluginInfo["type"]; icon: string; label: string }> = [
  { type: "login", icon: "fa-plug", label: "Login Plugins" },
  { type: "download", icon: "fa-cloud-download-alt", label: "Downloaders" },
  { type: "script", icon: "fa-scroll", label: "Scripts" },
]
const RIGHT_GROUPS: Array<{ type: PluginInfo["type"]; icon: string; label: string }> = [
  { type: "metadata", icon: "fa-digital-tachograph", label: "Metadata Plugins" },
]

// Mirrors legacy's plugins.html.tt2 — plugins grouped into flyouts by type, each a card. No
// "run this plugin against an archive" affordance — that's Edit.tsx's job, matching legacy.
export function Plugins() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins("all")
  const settings = useSettings()
  const updateSettings = useUpdateSettings()
  const queryClient = useQueryClient()
  const [running, setRunning] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [uploadStatus, setUploadStatus] = useState<string | null>(null)
  const [exportModalOpen, setExportModalOpen] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  useApplyTheme()
  useLegacyConfigCss()
  useDocumentTitle(t("common.pluginConfiguration") ?? undefined)
  useSectionDeepLink()

  async function runScript(path: string, params?: Record<string, string>) {
    setRunning(path)
    setResult(null)
    try {
      const query = params ? `?${new URLSearchParams(params)}` : ""
      const response = await fetch(`/api/database/scripts/${path}${query}`, { method: "POST" })
      const data = (await response.json()) as Record<string, unknown>
      setResult(JSON.stringify(data, null, 2))
      await queryClient.invalidateQueries({ queryKey: ["archives"] })
      await queryClient.invalidateQueries({ queryKey: ["categories"] })
    } finally {
      setRunning(null)
    }
  }

  async function uploadPlugin(file: File) {
    setUploadStatus(null)
    const body = new FormData()
    body.set("file", file)
    try {
      const response = await fetch("/api/plugins/upload", { method: "POST", body })
      const data = (await response.json()) as { success: number; name?: string; error?: string }
      setUploadStatus(
        data.success ? t("plugins.pluginUploadedName", { name: data.name }) : (data.error ?? t("plugins.uploadFailed") ?? ""),
      )
      if (data.success) await queryClient.invalidateQueries({ queryKey: ["plugins"] })
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  function renderGroupFlyout(group: { type: PluginInfo["type"]; icon: string; label: string }) {
    const groupPlugins = plugins.data?.filter((p) => p.type === group.type) ?? []
    return (
      <CollapsibleSection id={group.type} icon={group.icon} title={t(group.label)} key={group.type}>
        {/* Gates whether a metadata plugin's returned title is applied; tags/summary aren't gated. */}
        {group.type === "metadata" && (
          <div style={{ padding: "4px 0 8px 0" }}>
            <h1 className="ih" style={{ display: "inline" }}>
              {t("plugins.allowPluginsToReplaceArchive")}{" "}
            </h1>
            <input
              id="replacetitles"
              className="fa"
              type="checkbox"
              checked={settings.data?.replacetitles ?? true}
              onChange={(e) => updateSettings.mutate({ replacetitles: e.target.checked })}
            />
            <label htmlFor="replacetitles">
              <br />
              {t(
                "plugins.ifEnabledMetadataPluginsWill",
              )}
            </label>
          </div>
        )}
        {groupPlugins.length === 0 ? (
          <p>{t("plugins.noPluginsInstalled")}</p>
        ) : (
          <SortablePluginGroup type={group.type} plugins={groupPlugins} />
        )}
      </CollapsibleSection>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{t("plugins.plugins")}</h1>
      <p style={{ textAlign: "center" }}>
        <a href="/docs/" target="_blank" rel="noopener noreferrer">
          <i className="fa fa-book"></i> {t("plugins.pluginSdkDocumentation")}
        </a>
        {" · "}
        <a href="#" onClick={(e) => { e.preventDefault(); setExportModalOpen(true) }}>
          <i className="fa fa-download"></i> {t("plugins.exportAsZip")}
        </a>
      </p>
      {exportModalOpen && <ExportWizardPluginModal onClose={() => setExportModalOpen(false)} />}

      <div className="left-column" style={{ width: "49%" }}>
        <ul className="collapsible extensible with-right-caret">
          {LEFT_GROUPS.map(renderGroupFlyout)}

          <CollapsibleSection id="maintenance-scripts" icon="fa-scroll" title={t("plugins.maintenanceScripts")}>
              <p>{t("plugins.librarywideMaintenanceScriptsOperateOn")}</p>

              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, padding: "4px 0" }}>
                <span>
                  <b>{t("plugins.subfoldersToCategories")}</b>
                  <br />
                  {t("plugins.scanYourContentFolderAnd")}
                </span>
                <input
                  type="button"
                  className="stdbtn"
                  disabled={running === "subfolders-to-categories"}
                  onClick={() => void runScript("subfolders-to-categories")}
                  value={t("plugins.run") ?? undefined}
                />
              </div>
          </CollapsibleSection>
        </ul>
      </div>

      <div className="right-column" style={{ width: "50%" }}>
        <ul className="collapsible extensible with-right-caret">{RIGHT_GROUPS.map(renderGroupFlyout)}</ul>
      </div>

      {result && (
        <table className="itg">
          <tbody>
            <tr className="gtr1">
              <td>
                <pre className="log-panel">{result}</pre>
              </td>
            </tr>
          </tbody>
        </table>
      )}

      {uploadStatus && <p style={{ textAlign: "center" }}>{uploadStatus}</p>}

      {/* flex instead of text-align: center — a span and an input[type=button] baseline-align
          differently inline, offsetting the buttons. */}
      <h1 style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 8 }}>
        <span className="stdbtn fileinput-button" style={{ display: "inline-flex", alignItems: "center", justifyContent: "center" }}>
          <span style={{ fontWeight: "normal" }}>{t("plugins.uploadPlugin")}</span>
          <input
            ref={fileInputRef}
            type="file"
            accept=".ts"
            style={{ display: "none" }}
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) void uploadPlugin(file)
            }}
          />
        </span>
        <input
          type="button"
          className="stdbtn"
          value={t("pluginWizard.pageTitle") ?? undefined}
          title={
            settings.data && !settings.data.llm_api_key_set
              ? (t("pluginWizard.llmKeyNotConfiguredButtonHint") ?? undefined)
              : undefined
          }
          onClick={() => navigate(routes.pluginWizard())}
        />
        <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />
      </h1>
    </div>
  )
}
