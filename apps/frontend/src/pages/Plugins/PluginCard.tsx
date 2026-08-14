import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useSearchParams } from "react-router-dom"

import { usePluginOptions, usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import type { DragHandleProps } from "@/components/Display"
import { PluginOptionsForm } from "@/pages/PluginOptionsForm"
import { PluginParametersForm } from "@/pages/PluginParametersForm"

/** Width of the drag-handle column — narrow (just enough for the grip glyph plus a little
 * click-target padding), not a content-sized column, which would read as an oversized empty
 * gutter next to `PluginCard`'s own `width: 80%` inset. */
const DRAG_HANDLE_COLUMN_WIDTH = 18

// Per-plugin card, mirroring legacy's exact `pluginlist` markup: an inline-block `<span>` at 80%
// width with a bottom-border separator, name/version/author inline, "Run Automatically"/"depends
// on login plugin" floated right, then description, then the script-trigger table or "Plugin
// Settings" accordion. Plus this app's own additive "Download Settings" for download plugins
// whose `pluginOptions()` resolves — distinctly labeled from "Plugin Settings" so the two aren't
// confused.
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
  // Deep-link target from `/config/plugins?focus=<namespace>` (the upload queue's rate-limit
  // tooltip links here, issue #2). When this card is the focused plugin and its rate-limit section
  // has rendered, scroll it into view and briefly flash it.
  const [searchParams] = useSearchParams()
  const focusNamespace = searchParams.get("focus")
  const didFocusRef = useRef(false)

  const options = usePluginOptions(plugin.type === "download" ? plugin.namespace : "")
  const hasDownloadOptions = plugin.type === "download" && Boolean(options.data)
  const hasParameters = plugin.parameters.length > 0

  // Scrolls + highlights this plugin's rate-limit section once it has rendered (i.e. once
  // `usePluginOptions` resolves and `hasDownloadOptions` flips true) and this is the focused
  // plugin. Pure DOM side-effect (no React state) — the section element carries its own
  // `transition: background-color`, so setting its inline background here flashes amber and the
  // 2.5s `setTimeout` clearing it fades smoothly. `didFocusRef` guards against re-scrolling on
  // later polls. This is "synchronize with an external system (the DOM)" — exactly what an effect
  // is for.
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
  }, [focusNamespace, plugin.namespace, hasDownloadOptions])

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
        // Legacy's own markup uses two trailing `<br />` tags after each card to create the gap
        // before the next card — real vertical space in legacy's non-flex layout. Inside this flex
        // row, a `<br>` is just a zero-width flex item contributing nothing to row height, so
        // `marginBottom: 28` (2x the 14px a `<br>` renders at) reproduces the same visual gap
        // explicitly, applying uniformly across every theme.
        marginBottom: 28,
        // Only ever true inside `SortableList`'s own `DragOverlay` — this element is genuinely
        // detached from the page's normal layout flow at that point, so a lift shadow/scale-up/
        // raised z-index here can't squish or overlap any other row.
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
          // Top-aligned (not vertically centered in the whole, possibly-tall card) so the handle
          // sits level with the plugin's own icon/name row instead of floating in the card's
          // vertical middle. `paddingTop` nudges it down from the very top edge to roughly match
          // the 20px icon's own visual center, rather than sitting flush against the card's border.
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
          // Legacy hardcodes `width: 80%` here, a fixed proportion that leaves its right-floated
          // "Run Automatically" checkbox stopping short of the panel's actual right edge. `flex: 1`
          // instead fills 100% of the row's remaining width, a deliberate improvement over
          // legacy's layout, not a parity target.
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
        {/* `plugin.description` is a third-party string the plugin itself declares (issue #64) —
            rendered as plain text, not HTML, so a malicious/compromised plugin can't inject a
            script via its own metadata. */}
        <span>{plugin.description}</span>
        <br />

        {plugin.type === "script" && (
          <table>
            <tbody>
              {plugin.oneshot_arg && (
                <tr>
                  <td style={{ verticalAlign: "middle" }}>
                    {/* `plugin.oneshot_arg` is also plugin-declared (issue #64) — plain text. */}
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

        {hasDownloadOptions && <PluginOptionsForm namespace={plugin.namespace} />}
        {hasParameters && <PluginParametersForm namespace={plugin.namespace} parameters={plugin.parameters} />}

        <br />
      </span>
    </div>
  )
}
