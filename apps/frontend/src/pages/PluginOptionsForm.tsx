import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ValidationError } from "@/api/client"
import { usePluginOptions, useResetPluginOptions, useUpdatePluginOptions } from "@/api/hooks"
import type { PluginOptions, PluginOptionsUpdate } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"

import { ICON_BUTTON_STYLE } from "./Upload/shared"

interface DomainRuleFormRow {
  pattern: string
  max_concurrent: string
  // Entered/displayed in KB/s for the user's mental model (a raw bytes/sec figure runs to seven
  // digits for even a modest limit); converted to/from the backend's bytes/sec `max_bytes_per_sec`
  // at the form boundary (÷1024 on load, ×1024 on save).
  max_bytes_per_sec: string
  description?: string
}

// Settings form for one download plugin's `pluginOptions()` (specs/005-download-plugin-progress,
// US4) — renders each domain_rules entry and the bundle_as_archive toggle (only shown when the
// plugin's effective settings include it, i.e. only for a multi-resource-capable plugin), with
// each field's current plugin-default-vs-user-override state (T031's `source`) and inline 422
// validation-error display. Rendered as its own visually-separated section directly under the
// plugin card (issue #2: no extra "Download Settings" toggle/flyout layer).
export function PluginOptionsForm({ namespace }: { namespace: string }) {
  const { t } = useTranslation()
  const options = usePluginOptions(namespace)

  if (options.isLoading) return <p>{t("common.loading")}</p>
  if (!options.data) return null

  // Keyed by namespace so switching which plugin's settings are open (unmount + fresh mount)
  // re-seeds the form's local state from that plugin's own current data, without needing an
  // effect to re-sync state that's already available at mount time.
  return <PluginOptionsFormBody key={namespace} namespace={namespace} initial={options.data} />
}

/** Backend `max_bytes_per_sec` (bytes/sec) → KB/s display string. Whole KB values render without a
 * trailing `.0`; a non-divisible byte count keeps its fractional KB so nothing is silently rounded
 * away (a 1000-byte limit would otherwise show as 0 KB). */
function bytesPerSecToKb(bytesPerSec: number): string {
  return (bytesPerSec / 1024).toString()
}

function rowsFromOptions(options: PluginOptions): DomainRuleFormRow[] {
  return options.domain_rules.map((r) => ({
    pattern: r.pattern ?? "",
    max_concurrent: r.max_concurrent?.toString() ?? "",
    max_bytes_per_sec: r.max_bytes_per_sec != null ? bytesPerSecToKb(r.max_bytes_per_sec) : "",
    description: r.description,
  }))
}

function PluginOptionsFormBody({
  namespace,
  initial,
}: {
  namespace: string
  initial: PluginOptions
}) {
  const { t } = useTranslation()
  const update = useUpdatePluginOptions(namespace)
  const reset = useResetPluginOptions(namespace)

  const [rows, setRows] = useState<DomainRuleFormRow[]>(() => rowsFromOptions(initial))
  const [bundleValue, setBundleValue] = useState(initial.bundle_as_archive?.value ?? false)
  const [overwriteValue, setOverwriteValue] = useState(initial.overwrite_on_duplicate?.value ?? false)
  const [fieldError, setFieldError] = useState<{ field: string; message: string } | null>(null)

  function addRow() {
    setRows((r) => [...r, { pattern: "", max_concurrent: "", max_bytes_per_sec: "" }])
  }

  function removeRow(index: number) {
    setRows((r) => r.filter((_, i) => i !== index))
  }

  function updateRow(index: number, patch: Partial<DomainRuleFormRow>) {
    setRows((r) => r.map((row, i) => (i === index ? { ...row, ...patch } : row)))
  }

  async function save() {
    setFieldError(null)
    const body: PluginOptionsUpdate = {
      domain_rules: rows
        .filter((r) => r.pattern.trim() || r.max_concurrent.trim() || r.max_bytes_per_sec.trim())
        .map((r) => ({
          pattern: r.pattern.trim() || undefined,
          max_concurrent: r.max_concurrent.trim() ? Number(r.max_concurrent) : undefined,
          max_bytes_per_sec: r.max_bytes_per_sec.trim() ? Math.round(Number(r.max_bytes_per_sec) * 1024) : undefined,
        })),
    }
    if (initial.bundle_as_archive) body.bundle_as_archive = bundleValue
    if (initial.overwrite_on_duplicate) body.overwrite_on_duplicate = overwriteValue
    try {
      const saved = await update.mutateAsync(body)
      // Re-seed local state from the server's own response (not just leave `rows` as whatever was
      // last typed) — the backend is the source of truth for what was actually persisted, and this
      // is what makes a *second* Save/Reset in the same session see accurate `initial`-equivalent
      // data instead of a stale mount-time snapshot (this component never remounts on its own; only
      // switching which plugin's card is open does, per `PluginOptionsForm`'s `key={namespace}`).
      setRows(rowsFromOptions(saved))
      setBundleValue(saved.bundle_as_archive?.value ?? false)
      setOverwriteValue(saved.overwrite_on_duplicate?.value ?? false)
    } catch (e) {
      if (e instanceof ValidationError) {
        setFieldError({ field: e.field, message: e.message })
      } else {
        throw e
      }
    }
  }

  async function resetToDefaults() {
    const restored = await reset.mutateAsync()
    // Same reasoning as `save()` above — without this, deleting the only rule then clicking Reset
    // visibly did nothing: the mutation succeeded and the React Query cache updated, but this
    // component's own `rows` state was seeded once at mount and never re-derived from new props.
    setRows(rowsFromOptions(restored))
    setBundleValue(restored.bundle_as_archive?.value ?? false)
    setOverwriteValue(restored.overwrite_on_duplicate?.value ?? false)
  }

  return (
    <div
      // Stable anchor for deep-linking from the upload queue's rate-limit tooltip (issue #2):
      // `routes.pluginSettings(namespace)` lands on `/config/plugins?focus=<ns>`, and Plugins.tsx
      // scrolls this section into view + briefly highlights it.
      data-download-settings-namespace={namespace}
      className="option-flyout"
      style={{
        // A visually distinct section, not a flyout: top separator + spacing sets it apart from
        // the plugin's description/parameters above (issue #2).
        borderTop: "1px solid rgba(128,128,128,0.3)",
        marginTop: 8,
        paddingTop: 8,
        // `transition: background-color` lets Plugins.tsx's deep-link highlight (an inline
        // backgroundColor set then cleared 2.5s later, directly on this element) fade smoothly
        // rather than snapping off (issue #2).
        transition: "background-color 0.6s ease",
      }}
    >
      <h3 className="ih" style={{ fontSize: "1.0em", margin: "0 0 6px" }}>
        {t("pluginOptions.downloadRatelimitSettings")}
      </h3>
      <table className="itg" style={{ width: "100%" }}>
        <thead>
          <tr className="jtr0">
            <th>{t("pluginOptions.domainPattern")}</th>
            <th>{t("pluginOptions.maxConcurrent")}</th>
            <th>{t("pluginOptions.maxKbS")}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i} className="gtr1">
              <td>
                <input
                  className="stdinput"
                  value={row.pattern}
                  placeholder={t("pluginOptions.ExampleCom") ?? undefined}
                  onChange={(e) => updateRow(i, { pattern: e.target.value })}
                />
              </td>
              <td>
                <input
                  className="stdinput"
                  type="number"
                  min={1}
                  value={row.max_concurrent}
                  onChange={(e) => updateRow(i, { max_concurrent: e.target.value })}
                />
              </td>
              <td>
                <input
                  className="stdinput"
                  type="number"
                  min={0}
                  step="any"
                  value={row.max_bytes_per_sec}
                  onChange={(e) => updateRow(i, { max_bytes_per_sec: e.target.value })}
                />
              </td>
              <td>
                {/* Icon-only button (matches the upload queue's own row-action buttons, e.g. its
                    "Delete"/"Remove" `fa-times`) instead of a labeled `.stdbtn` — a one-word
                    "Remove" button at `.stdbtn`'s default min-width was far wider than the column
                    needed. */}
                <Tooltip label={t("pluginOptions.remove") ?? ""}>
                  <button type="button" className="stdbtn" style={ICON_BUTTON_STYLE} onClick={() => removeRow(i)}>
                    <i className="fa fa-times" aria-hidden="true"></i>
                  </button>
                </Tooltip>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <input type="button" className="stdbtn" value={t("pluginOptions.addRule") ?? undefined} onClick={addRow} />

      {initial.bundle_as_archive && (
        <p>
          <label>
            {/* `className="fa"` is what makes legacy's `config.css` (linked by this page's own
                `useEffect`) render this as the real ON/OFF switch, matching the "Run Automatically"
                toggle above — a plain unstyled checkbox renders as a bare browser checkbox in this
                theme (a hollow red square with no check glyph), not a rendering failure per se, just
                the wrong class. */}
            <input type="checkbox" className="fa" checked={bundleValue} onChange={(e) => setBundleValue(e.target.checked)} />{" "}
            {t(initial.bundle_as_archive.description)}
          </label>
        </p>
      )}

      {initial.overwrite_on_duplicate && (
        <p>
          <label>
            <input type="checkbox" className="fa" checked={overwriteValue} onChange={(e) => setOverwriteValue(e.target.checked)} />{" "}
            {t(initial.overwrite_on_duplicate.description)}
          </label>
        </p>
      )}

      {fieldError && (
        <p className="error" style={{ color: "red" }}>
          {fieldError.field}: {fieldError.message}
        </p>
      )}

      <div style={{ display: "flex", gap: 8 }}>
        <input
          type="button"
          className="stdbtn"
          disabled={update.isPending}
          value={t("pluginOptions.saveSettings") ?? undefined}
          onClick={() => void save()}
        />
        <input
          type="button"
          className="stdbtn"
          disabled={reset.isPending}
          value={t("pluginOptions.resetToDefaults") ?? undefined}
          onClick={() => void resetToDefaults()}
        />
      </div>
    </div>
  )
}
