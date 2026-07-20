import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { ValidationError } from '../api/client'
import { usePluginOptions, useResetPluginOptions, useUpdatePluginOptions } from '../api/hooks'
import type { PluginOptions, PluginOptionsUpdate } from '../api/types'

interface DomainRuleFormRow {
  pattern: string
  max_concurrent: string
  max_bytes_per_sec: string
  description?: string
}

// Settings form for one download plugin's `pluginOptions()` (specs/005-download-plugin-progress,
// US4) — renders each domain_rules entry and the bundle_as_archive toggle (only shown when the
// plugin's effective settings include it, i.e. only for a multi-resource-capable plugin), with
// each field's current plugin-default-vs-user-override state (T031's `source`) and inline 422
// validation-error display.
export default function PluginOptionsForm({ namespace, onClose }: { namespace: string; onClose: () => void }) {
  const { t } = useTranslation()
  const options = usePluginOptions(namespace)

  if (options.isLoading) return <p>{t('Loading…')}</p>
  if (!options.data) return null

  // Keyed by namespace so switching which plugin's settings are open (unmount + fresh mount)
  // re-seeds the form's local state from that plugin's own current data, without needing an
  // effect to re-sync state that's already available at mount time.
  return <PluginOptionsFormBody key={namespace} namespace={namespace} initial={options.data} onClose={onClose} />
}

function PluginOptionsFormBody({
  namespace,
  initial,
  onClose,
}: {
  namespace: string
  initial: PluginOptions
  onClose: () => void
}) {
  const { t } = useTranslation()
  const update = useUpdatePluginOptions(namespace)
  const reset = useResetPluginOptions(namespace)

  const [rows, setRows] = useState<DomainRuleFormRow[]>(() =>
    initial.domain_rules.map((r) => ({
      pattern: r.pattern ?? '',
      max_concurrent: r.max_concurrent?.toString() ?? '',
      max_bytes_per_sec: r.max_bytes_per_sec?.toString() ?? '',
      description: r.description,
    })),
  )
  const [bundleValue, setBundleValue] = useState(initial.bundle_as_archive?.value ?? false)
  const [overwriteValue, setOverwriteValue] = useState(initial.overwrite_on_duplicate?.value ?? false)
  const [fieldError, setFieldError] = useState<{ field: string; message: string } | null>(null)

  function addRow() {
    setRows((r) => [...r, { pattern: '', max_concurrent: '', max_bytes_per_sec: '' }])
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
          max_bytes_per_sec: r.max_bytes_per_sec.trim() ? Number(r.max_bytes_per_sec) : undefined,
        })),
    }
    if (initial.bundle_as_archive) body.bundle_as_archive = bundleValue
    if (initial.overwrite_on_duplicate) body.overwrite_on_duplicate = overwriteValue
    try {
      await update.mutateAsync(body)
    } catch (e) {
      if (e instanceof ValidationError) {
        setFieldError({ field: e.field, message: e.message })
      } else {
        throw e
      }
    }
  }

  return (
    <div className="option-flyout" style={{ padding: '8px 0' }}>
      <table className="itg" style={{ width: '100%' }}>
        <thead>
          <tr className="jtr0">
            <th>{t('Domain Pattern')}</th>
            <th>{t('Max Concurrent')}</th>
            <th>{t('Max Bytes/sec')}</th>
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
                  placeholder={t('*.example.com') ?? undefined}
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
                  min={1}
                  value={row.max_bytes_per_sec}
                  onChange={(e) => updateRow(i, { max_bytes_per_sec: e.target.value })}
                />
              </td>
              <td>
                <input type="button" className="stdbtn" value={t('Remove') ?? undefined} onClick={() => removeRow(i)} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <input type="button" className="stdbtn" value={t('Add Rule') ?? undefined} onClick={addRow} />

      {initial.bundle_as_archive && (
        <p>
          <label>
            <input type="checkbox" checked={bundleValue} onChange={(e) => setBundleValue(e.target.checked)} />{' '}
            {t(initial.bundle_as_archive.description)}
          </label>
        </p>
      )}

      {initial.overwrite_on_duplicate && (
        <p>
          <label>
            <input type="checkbox" checked={overwriteValue} onChange={(e) => setOverwriteValue(e.target.checked)} />{' '}
            {t(initial.overwrite_on_duplicate.description)}
          </label>
        </p>
      )}

      {fieldError && (
        <p className="error" style={{ color: 'red' }}>
          {fieldError.field}: {fieldError.message}
        </p>
      )}

      <div style={{ display: 'flex', gap: 8 }}>
        <input
          type="button"
          className="stdbtn"
          disabled={update.isPending}
          value={t('Save Settings') ?? undefined}
          onClick={() => void save()}
        />
        <input
          type="button"
          className="stdbtn"
          disabled={reset.isPending}
          value={t('Reset to Defaults') ?? undefined}
          onClick={() => void reset.mutateAsync()}
        />
        <input type="button" className="stdbtn" value={t('Close') ?? undefined} onClick={onClose} />
      </div>
    </div>
  )
}
