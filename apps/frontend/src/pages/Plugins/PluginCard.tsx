import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useSearchParams } from "react-router-dom"

import { usePluginOptions, usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import type { DragHandleProps } from "@/components/common-ui/Display"
import { PluginOptionsForm } from "@/pages/PluginOptionsForm"
import { PluginParametersForm } from "@/pages/PluginParametersForm"

/** Width of the drag-handle column — narrow (just enough for the grip glyph plus a little
 * click-target padding), not a content-sized column that would read as an oversized empty gutter. */
const DRAG_HANDLE_COLUMN_WIDTH = 18

/** Per-plugin card, mirroring legacy's `pluginlist` markup, plus this app's own additive Download
 * Settings section for download plugins whose `pluginOptions()` resolves. */
export function PluginCard({
  plugin,
  dragHandleProps,
}: {
  plugin: PluginInfo
  dragHandleProps: DragHandleProps
}) {
  const { t } = useTranslation()
  const [scriptArg, setScriptArg] = useState("")
  const [scriptRunning, setScriptRunning] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  // Deep-link target from /config/plugins?focus=<namespace> (upload queue's rate-limit tooltip) —
  // when this card is focused and its rate-limit section has rendered, scroll + flash it.
  const [searchParams] = useSearchParams()
  const focusNamespace = searchParams.get("focus")
  const didFocusRef = useRef(false)

  const options = usePluginOptions(plugin.type === "download" ? plugin.namespace : "")
  const hasDownloadOptions = plugin.type === "download" && Boolean(options.data)
  const hasParameters = plugin.parameters.length > 0

  // The deep-link target now lives inside the collapsed settings panel, so open it before trying
  // to scroll — otherwise the element doesn't exist and the flash silently no-ops.
  useEffect(() => {
    if (focusNamespace === plugin.namespace && hasDownloadOptions && !didFocusRef.current) {
      setSettingsOpen(true)
    }
  }, [focusNamespace, plugin.namespace, hasDownloadOptions])

  useEffect(() => {
    if (!focusNamespace || focusNamespace !== plugin.namespace || !hasDownloadOptions || didFocusRef.current) return
    const el = document.querySelector(`[data-download-settings-namespace="${CSS.escape(plugin.namespace)}"]`)
    if (!el) return
    didFocusRef.current = true
    el.scrollIntoView({ block: "center", behavior: "smooth" })
    const htmlEl = el as HTMLElement
    htmlEl.style.backgroundColor = "rgba(230, 126, 34, 0.18)"
    const timer = window.setTimeout(() => {
      htmlEl.style.backgroundColor = ""
    }, 2500)
    return () => window.clearTimeout(timer)
    // `settingsOpen` is a real dependency: the target only exists once the panel above has opened.
  }, [focusNamespace, plugin.namespace, hasDownloadOptions, settingsOpen])

  const settings = usePluginSettings(plugin.type === "metadata" ? plugin.namespace : "")
  const updateSettings = useUpdatePluginSettings(plugin.namespace)

  async function triggerScript() {
    setScriptRunning(true)
    try {
      const query = new URLSearchParams({ plugin: plugin.namespace })
      if (scriptArg) query.set("arg", scriptArg)
      await fetch(`/api/plugins/queue?${query}`, { method: "POST" })
    } finally {
      setScriptRunning(false)
    }
  }

  const { attributes, listeners, isDragging } = dragHandleProps

  return (
    <div
      style={{
        display: "flex",
        alignItems: "stretch",
        marginBottom: 28,
        ...(isDragging && {
          zIndex: 1,
          position: "relative",
          boxShadow: "0 8px 16px rgba(0, 0, 0, 0.35)",
          scale: "1.02",
        }),
      }}
    >
      <span
        {...attributes}
        {...listeners}
        style={{
          width: DRAG_HANDLE_COLUMN_WIDTH,
          flexShrink: 0,
          display: "flex",
          alignItems: "flex-start",
          justifyContent: "center",
          paddingTop: 6,
          cursor: isDragging ? "grabbing" : "grab",
          touchAction: "none",
          fontSize: "0.9em",
          opacity: 0.5,
        }}
      >
        <i className="fa fa-grip-vertical" aria-hidden="true"></i>
      </span>
      <span
        style={{
          display: "inline-block",
          textAlign: "left",
          flex: 1,
          minWidth: 0,
          borderBottomWidth: 1,
          borderBottomStyle: "solid",
        }}
      >
        {plugin.icon ? (
          <img height={20} width={20} src={plugin.icon} alt="" />
        ) : (
          <i className="fa fa-puzzle-piece" style={{ fontSize: 20 }}></i>
        )}
        <h2 className="ih" style={{ display: "inline" }}>
          {" "}
          {plugin.name} v.{plugin.version}
        </h2>
        <h1 className="ih" style={{ display: "inline" }}>
          {" "}
          by {plugin.author}{" "}
        </h1>
        {plugin.generated_by_wizard && (
          <i
            className="fa fa-magic"
            aria-hidden="true"
            title={t("plugins.generatedByWizard") ?? undefined}
            style={{ marginLeft: 4, opacity: 0.7 }}
          ></i>
        )}

        <div style={{ float: "right", textAlign: "right" }}>
          {plugin.type === "metadata" && settings.data && (
            <>
              <h1 className="ih" style={{ display: "inline" }}>
                {" "}
                {t("plugins.runAutomatically")}:{" "}
              </h1>
              <input
                type="checkbox"
                className="fa"
                checked={settings.data.enabled}
                onChange={(e) => void updateSettings.mutateAsync({ enabled: e.target.checked })}
              />
              <br />
            </>
          )}
          {plugin.login_from && (
            <>
              <i className="fa fa-plug" aria-hidden="true"></i>{" "}
              {t("plugins.thisPluginDependsOnThe")} "{plugin.login_from}".
            </>
          )}
        </div>

        <br />
        <span>{plugin.description}</span>
        <br />

        {plugin.type === "script" && (
          <table>
            <tbody>
              {plugin.oneshot_arg && (
                <tr>
                  <td style={{ verticalAlign: "middle" }}>
                    <b>{t(plugin.oneshot_arg)} :</b>
                  </td>
                  <td>
                    <input style={{ maxWidth: 200 }} size={20} value={scriptArg} onChange={(e) => setScriptArg(e.target.value)} />
                  </td>
                </tr>
              )}
              <tr>
                <td colSpan={2}>
                  <input
                    type="button"
                    className="stdbtn"
                    disabled={scriptRunning}
                    onClick={() => void triggerScript()}
                    value={t("plugins.triggerScript") ?? undefined}
                  />
                </td>
              </tr>
            </tbody>
          </table>
        )}

        {/* One "插件设置" panel for everything configurable about this plugin — the download
            options (rate limits, overwrite, revision policies) and the plugin's own declared
            parameters both live here, rather than the options sitting loose above the panel while
            only the parameters were collapsible. */}
        {(hasDownloadOptions || hasParameters) && (
          <>
            <div
              className={`collapsible-title caret-right${settingsOpen ? " active" : ""}`}
              style={{ padding: "5px 0 0 5px", cursor: "pointer" }}
              onClick={() => setSettingsOpen((o) => !o)}
            >
              <i className="fas fa-sliders-h fa-2x" style={{ marginRight: 4 }} aria-hidden="true"></i>
              <b style={{ verticalAlign: "super" }}>{t("pluginParameters.pluginSettings")}</b>
            </div>
            {settingsOpen && (
              <div className="collapsible-body" style={{ padding: "5px 0 0 0" }}>
                {hasDownloadOptions && <PluginOptionsForm namespace={plugin.namespace} />}
                {/* Deliberately its own box, outside the options form: everything in that form
                    persists through one `/plugins/options` record that Save/Reset act on as a
                    unit, while these parameters go to `/plugins/settings` and are untouched by
                    that Reset. Putting them under the Reset button would misrepresent its reach. */}
                {hasParameters && (
                  <div className="option-flyout">
                    <PluginParametersForm namespace={plugin.namespace} parameters={plugin.parameters} />
                  </div>
                )}
              </div>
            )}
          </>
        )}

        <br />
      </span>
    </div>
  )
}
