import DOMPurify from "dompurify"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useSearchParams } from "react-router-dom"

import { usePluginOptions, usePluginSettings, useUpdatePluginSettings } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import type { DragHandleProps } from "@/components/common-ui/Display"
import { CollapsibleSection } from "@/components/Display"
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
  // Deep-link target from /config/plugins?focus=<namespace> (the upload queue's rate-limit tooltip,
  // a failed row's "open plugin settings" link): open this card's plugin-type group and its
  // settings panel, then scroll to the download options if it has them, else to the card.
  const [searchParams] = useSearchParams()
  const focused = searchParams.get("focus") === plugin.namespace
  const didFocusRef = useRef(false)
  // The group title is a toggle: StrictMode runs the effect twice before the first click re-renders,
  // so a second click would close the group again.
  const groupOpenedRef = useRef(false)
  const cardRef = useRef<HTMLDivElement>(null)

  const options = usePluginOptions(plugin.type === "download" ? plugin.namespace : "")
  const hasDownloadOptions = plugin.type === "download" && Boolean(options.data)
  const hasParameters = plugin.parameters.length > 0
  // A download plugin's options arrive separately; scrolling before they do would land on the card.
  const optionsSettled = plugin.type !== "download" || !options.isPending

  useEffect(() => {
    if (focused && (hasDownloadOptions || hasParameters) && !didFocusRef.current) setSettingsOpen(true)
  }, [focused, hasDownloadOptions, hasParameters])

  useEffect(() => {
    const card = cardRef.current
    if (!focused || !optionsSettled || !card || didFocusRef.current) return
    didFocusRef.current = true
    const groupTitle = card.parentElement?.closest("[data-section-id]")?.querySelector<HTMLElement>(":scope > .collapsible-title")
    if (groupTitle && !groupOpenedRef.current && !groupTitle.classList.contains("active")) groupTitle.click()
    groupOpenedRef.current = true
    let flashTimer = 0
    let scrolled = false
    // One frame for the group to stop being `display: none`, or there is nothing to scroll to.
    const frame = window.requestAnimationFrame(() => {
      scrolled = true
      const el =
        card.querySelector<HTMLElement>(`[data-download-settings-namespace="${CSS.escape(plugin.namespace)}"]`) ?? card
      el.scrollIntoView({ block: "center", behavior: "smooth" })
      el.style.backgroundColor = "rgba(230, 126, 34, 0.18)"
      flashTimer = window.setTimeout(() => {
        el.style.backgroundColor = ""
      }, 2500)
    })
    return () => {
      // Torn down before the frame ran (StrictMode's double effect): let the next run try again.
      if (!scrolled) {
        window.cancelAnimationFrame(frame)
        didFocusRef.current = false
      }
      window.clearTimeout(flashTimer)
    }
  }, [focused, optionsSettled, plugin.namespace])

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

  const { handleRef, isDragging } = dragHandleProps

  return (
    <div
      ref={cardRef}
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
        ref={handleRef}
        role="button"
        tabIndex={0}
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
        {/* Legacy plugin descriptions are HTML — they carry `<br>` for paragraphs, `<code>` around
            regexes and named entities (`&rarr;`, `&lt;`), which is how LANraragi rendered them. As
            text they showed the markup itself. Sanitised rather than trusted: a description travels
            inside the plugin file, and this page runs in the admin's own session.
            `plugin-description` also lets long unbreakable regexes wrap instead of widening the card. */}
        <span
          className="plugin-description"
          dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(plugin.description ?? "") }}
        />
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
            <CollapsibleSection
              as="div"
              icon="fa-sliders"
              iconClassName="fas fa-sliders-h"
              open={settingsOpen}
              onOpenChange={setSettingsOpen}
              // 4px top / 8px bottom: the line box's own content sits 4px low (font ascent vs the
              // icon's baseline), so equal padding still read as bottom-heavy.
              titleStyle={{ padding: "4px 12px 8px" }}
              bodyStyle={{ padding: "5px 0 0 0" }}
              title={<b>{t("pluginParameters.pluginSettings")}</b>}
            >
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
            </CollapsibleSection>
          </>
        )}

        <br />
      </span>
    </div>
  )
}
