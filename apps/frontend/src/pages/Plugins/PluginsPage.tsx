import { useQueryClient } from "@tanstack/react-query"
import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { usePlugins, useSettings, useUpdateSettings } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { CollapsibleSection } from "@/components/Display/CollapsibleSection"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { useApplyTheme, useLegacyConfigCss } from "@/theme"

import { SortablePluginGroup } from "./SortablePluginGroup"

// Legacy's own left/right split (`~/LANraragi/templates/plugins.html.tt2`): left column is
// Login Plugins → Downloaders → Scripts (script-*type* plugins, not this app's own maintenance
// scripts below), right column is just Metadata Plugins.
const LEFT_GROUPS: Array<{ type: PluginInfo["type"]; icon: string; label: string }> = [
  { type: "login", icon: "fa-plug", label: "Login Plugins" },
  { type: "download", icon: "fa-cloud-download-alt", label: "Downloaders" },
  { type: "script", icon: "fa-scroll", label: "Scripts" },
]
const RIGHT_GROUPS: Array<{ type: PluginInfo["type"]; icon: string; label: string }> = [
  { type: "metadata", icon: "fa-digital-tachograph", label: "Metadata Plugins" },
]

// Mirrors legacy's `~/LANraragi/templates/plugins.html.tt2` — plugins grouped into
// `.collapsible.extensible.with-right-caret` > `.option-flyout` flyouts by type, each plugin a
// card with icon/name/version/author/description. There is deliberately no "run this plugin
// against an archive" affordance here — legacy has none either: a metadata plugin's "Run
// Automatically" checkbox controls whether it fires on new archives, a script-type plugin has its
// own "Trigger Script" button, and a download plugin is only invoked via the upload page's
// URL-download form. Manually running one plugin against one existing archive is `Edit.tsx`'s job
// — script-type "Library-wide maintenance scripts" below have no legacy equivalent at all.
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
  const fileInputRef = useRef<HTMLInputElement>(null)
  useApplyTheme()
  useLegacyConfigCss()
  useDocumentTitle(t("Plugin Configuration") ?? undefined)

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
        data.success ? t("Plugin uploaded: {{name}}", { name: data.name }) : (data.error ?? t("Upload failed.") ?? ""),
      )
      if (data.success) await queryClient.invalidateQueries({ queryKey: ["plugins"] })
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  function renderGroupFlyout(group: { type: PluginInfo["type"]; icon: string; label: string }) {
    const groupPlugins = plugins.data?.filter((p) => p.type === group.type) ?? []
    return (
      <CollapsibleSection icon={group.icon} title={t(group.label)} key={group.type}>
        {/* Legacy's own `plugins.html.tt2:84-91` — sits at the top of the Metadata Plugins flyout
            body, above the plugin list itself. Gates whether a metadata plugin's returned `title`
            is actually applied to an archive (`Edit.tsx`'s own `handleSave`/`runPlugin`); tags and
            summary are never gated. */}
        {group.type === "metadata" && (
          <div style={{ padding: "4px 0 8px 0" }}>
            <h1 className="ih" style={{ display: "inline" }}>
              {t("Allow Plugins to replace archive titles:")}{" "}
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
                "If enabled, metadata plugins will be able to change the title of your archives alongside adding tags to them.",
              )}
            </label>
          </div>
        )}
        {groupPlugins.length === 0 ? (
          <p>{t("No plugins installed.")}</p>
        ) : (
          <SortablePluginGroup type={group.type} plugins={groupPlugins} />
        )}
      </CollapsibleSection>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{t("Plugins")}</h1>
      <p style={{ textAlign: "center" }}>
        <a href="/docs/" target="_blank" rel="noopener noreferrer">
          <i className="fa fa-book"></i> {t("Plugin SDK Documentation")}
        </a>
      </p>

      <div className="left-column" style={{ width: "49%" }}>
        <ul className="collapsible extensible with-right-caret">
          {LEFT_GROUPS.map(renderGroupFlyout)}

          {/* This app's own library-wide maintenance scripts — a genuinely new feature with no
              legacy template, labeled distinctly from legacy's real "Scripts" flyout above
              (per-plugin script execution) so the two aren't confused. Only "Subfolders to
              Categories" lives here — it walks the entire archive directory tree, I/O-heavy enough
              to warrant a native Rust endpoint rather than a Deno-subprocess round trip. Source
              Finder / nHentai Source Converter are real `script`-type plugins and render as
              ordinary cards in the "Scripts" flyout above. */}
          <CollapsibleSection icon="fa-scroll" title={t("Maintenance Scripts")}>
              <p>{t("Library-wide maintenance scripts (operate on the whole database, not one archive).")}</p>

              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, padding: "4px 0" }}>
                <span>
                  <b>{t("Subfolders to Categories")}</b>
                  <br />
                  {t("Scan your Content Folder and automatically create Static Categories for each subfolder.")}
                </span>
                <input
                  type="button"
                  className="stdbtn"
                  disabled={running === "subfolders-to-categories"}
                  onClick={() => void runScript("subfolders-to-categories")}
                  value={t("Run") ?? undefined}
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

      {/* `justifyContent`/`alignItems: center` (flex) instead of plain `text-align: center` —
          a `<span class="fileinput-button">` (text baseline) and an `<input type="button">`
          (replaced-ish element) align differently under `vertical-align: baseline` when sitting
          side by side inline, offsetting the Upload Plugin button above the Return to Library
          button. Flex sidesteps the baseline-vs-replaced-element quirk entirely. */}
      <h1 style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 8 }}>
        {/* Also flex: an `<input type="button">`'s value text is vertically centered in its
            content box natively, but a `<span>` wrapping plain text just follows normal inline
            flow, leaving the label sitting near the top of the box instead of centered.
            `fontWeight: 'normal'` on the inner label: this `<h1>`'s own bold inherits into a plain
            `<span>` child, since form controls don't inherit a heading's bold by browser UA
            default the way ordinary text elements do. */}
        <span className="stdbtn fileinput-button" style={{ display: "inline-flex", alignItems: "center", justifyContent: "center" }}>
          <span style={{ fontWeight: "normal" }}>{t("Upload Plugin")}</span>
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
        <input type="button" id="return" className="stdbtn" value={t("Return to Library") ?? undefined} onClick={() => navigate(routes.library())} />
      </h1>
    </div>
  )
}
