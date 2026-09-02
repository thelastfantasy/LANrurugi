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
  // Displayed in KB/s for the user's mental model; converted to/from backend bytes/sec at the
  // form boundary (÷1024 on load, ×1024 on save).
  max_bytes_per_sec: string
  description?: string
}

/** Settings form for one download plugin's `pluginOptions()` — renders `domain_rules` entries and
 * the `bundle_as_archive` toggle (multi-resource-capable plugins only), with inline 422 errors. */
export function PluginOptionsForm({ namespace }: { namespace: string }) {
  const { t } = useTranslation()
  const options = usePluginOptions(namespace)

  if (options.isLoading) return <p>{t("common.loading")}</p>
  if (!options.data) return null

  // Keyed by namespace so switching plugins remounts with fresh local state.
  return <PluginOptionsFormBody key={namespace} namespace={namespace} initial={options.data} />
}

/** Backend `max_bytes_per_sec` → KB/s display string; keeps fractional KB rather than rounding
 * away a non-divisible byte count (a 1000-byte limit would otherwise show as 0 KB). */
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
      // Re-seeds from the server's response, not last-typed rows — this component never remounts
      // between saves, so a second Save/Reset needs accurate data, not a stale mount snapshot.
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
    setRows(rowsFromOptions(restored))
    setBundleValue(restored.bundle_as_archive?.value ?? false)
    setOverwriteValue(restored.overwrite_on_duplicate?.value ?? false)
  }

  return (
    <div
      data-download-settings-namespace={namespace}
      className="option-flyout"
      style={{
        borderTop: "1px solid rgba(128,128,128,0.3)",
        marginTop: 8,
        paddingTop: 8,
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
