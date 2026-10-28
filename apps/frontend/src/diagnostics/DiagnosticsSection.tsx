/** The diagnostics settings panel and its two modals (specs/018 US1–US3). */

import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Modal } from "@/components/common-ui/Display"
import { CheckboxField, NumberInput, Select } from "@/components/common-ui/Form"
import { toast } from "@/toast"

import {
  type ForwardWindow,
  useCloseWindow,
  useDiagnosticsOverview,
  useOpenWindow,
  useSaveDiagnosticsSettings,
} from "./api"
import { BundleModal } from "./BundleModal"
import { IssuesModal } from "./IssuesModal"

const WINDOW_CHOICES = [
  { value: "300", labelKey: "diagnostics.window.minutes5" },
  { value: "3600", labelKey: "diagnostics.window.hour1" },
  { value: "86400", labelKey: "diagnostics.window.day1" },
]

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ["KB", "MB", "GB"]
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(1)} ${units[unit]}`
}

function formatRemaining(seconds: number): string {
  if (seconds <= 0) return "0s"
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const rest = seconds % 60
  if (hours > 0) return `${hours}h ${minutes}m`
  if (minutes > 0) return `${minutes}m ${rest}s`
  return `${rest}s`
}

export function DiagnosticsSection() {
  const { t } = useTranslation()
  const overview = useDiagnosticsOverview()
  const save = useSaveDiagnosticsSettings()
  const openWindow = useOpenWindow()
  const closeWindow = useCloseWindow()
  const [issuesOpen, setIssuesOpen] = useState(false)
  const [bundleOpen, setBundleOpen] = useState(false)
  const [dsn, setDsn] = useState("")
  const [duration, setDuration] = useState("300")

  const settings = overview.data?.settings
  const usage = overview.data?.usage
  const drops = overview.data?.drops
  const window: ForwardWindow | undefined = overview.data?.window

  const onErrorToast = (error: unknown) =>
    toast({ icon: "error", text: error instanceof Error ? error.message : String(error) })
  const update = (body: Record<string, unknown>) => save.mutate(body, { onError: onErrorToast })

  return (
    <div className="settings-section diagnostics-section">
      <h3 className="ih">{t("diagnostics.title")}</h3>
      <p className="settings-hint">{t("diagnostics.hint")}</p>

      <div className="diagnostics-row">
        <CheckboxField
          checked={settings?.enabled ?? false}
          onCheckedChange={(checked) => update({ diagnostics_enabled: checked })}
        >
          {t("diagnostics.enabled")}
        </CheckboxField>
      </div>

      <div className="diagnostics-row diagnostics-sources">
        <CheckboxField
          checked={settings?.sources.web ?? false}
          onCheckedChange={(checked) => update({ diagnostics_web: checked })}
        >
          {t("diagnostics.sourceWeb")}
        </CheckboxField>
        <CheckboxField
          checked={settings?.sources.server ?? false}
          onCheckedChange={(checked) => update({ diagnostics_server: checked })}
        >
          {t("diagnostics.sourceServer")}
        </CheckboxField>
        <CheckboxField
          checked={settings?.sources.plugins ?? false}
          onCheckedChange={(checked) => update({ diagnostics_plugins: checked })}
        >
          {t("diagnostics.sourcePlugins")}
        </CheckboxField>
        <CheckboxField
          checked={settings?.breadcrumbs ?? false}
          onCheckedChange={(checked) => update({ diagnostics_breadcrumbs: checked })}
        >
          {t("diagnostics.breadcrumbs")}
        </CheckboxField>
        <CheckboxField
          checked={settings?.include_archive_names ?? false}
          onCheckedChange={(checked) => update({ diagnostics_include_archive_names: checked })}
        >
          {t("diagnostics.includeArchives")}
        </CheckboxField>
      </div>

      <div className="diagnostics-row diagnostics-limits">
        <label className="diagnostics-field">
          <span>{t("diagnostics.retentionDays")}</span>
          <NumberInput
            value={settings?.retention_days ?? 7}
            min={1}
            onValueChange={(value) => update({ diagnostics_retention_days: value })}
          />
        </label>
        <label className="diagnostics-field">
          <span>{t("diagnostics.maxEvents")}</span>
          <NumberInput
            value={settings?.max_events ?? 50000}
            min={100}
            onValueChange={(value) => update({ diagnostics_max_events: value })}
          />
        </label>
        <label className="diagnostics-field">
          <span>{t("diagnostics.maxBytes")}</span>
          <NumberInput
            value={settings?.max_bytes ?? 536870912}
            min={1048576}
            onValueChange={(value) => update({ diagnostics_max_bytes: value })}
          />
        </label>
      </div>

      <p className="settings-hint">
        {t("diagnostics.usage", {
          events: usage?.events ?? 0,
          issues: usage?.issues ?? 0,
          bytes: formatBytes(usage?.bytes ?? 0),
        })}
        {drops && (drops.rate_limit > 0 || drops.retention > 0 || drops.too_large > 0) && (
          <>
            {" · "}
            {t("diagnostics.drops", {
              retention: drops.retention,
              rate: drops.rate_limit,
              large: drops.too_large,
            })}
          </>
        )}
      </p>

      <div className="diagnostics-actions">
        <button type="button" className="stdbtn" onClick={() => setIssuesOpen(true)}>
          {t("diagnostics.openIssues")}
        </button>
        <button type="button" className="stdbtn" onClick={() => setBundleOpen(true)}>
          {t("diagnostics.openBundle")}
        </button>
        <a className="stdbtn" href="/logs">
          {t("diagnostics.rawLogs")}
        </a>
      </div>

      <h4 className="ih">{t("diagnostics.forward.title")}</h4>
      <p className="settings-hint">{t("diagnostics.forward.hint")}</p>
      <div className="diagnostics-row">
        <input
          className="stdinput diagnostics-dsn"
          type="url"
          placeholder="https://key@tracker.example.com/1"
          value={dsn}
          onChange={(event) => setDsn(event.target.value)}
        />
        <button
          type="button"
          className="stdbtn"
          disabled={save.isPending}
          onClick={() => save.mutate({ tracker_dsn: dsn }, { onError: onErrorToast })}
        >
          {t("diagnostics.forward.save")}
        </button>
        <span className="settings-hint">
          {settings?.tracker_configured
            ? t("diagnostics.forward.configured")
            : t("diagnostics.forward.notConfigured")}
        </span>
      </div>

      <div className="diagnostics-row">
        <Select
          value={duration}
          onValueChange={setDuration}
          align="start"
          items={WINDOW_CHOICES.map((choice) => ({
            value: choice.value,
            label: t(choice.labelKey),
          }))}
        />
        <button
          type="button"
          className="stdbtn"
          disabled={openWindow.isPending || !settings?.tracker_configured}
          onClick={() =>
            openWindow.mutate(Number(duration), {
              onError: onErrorToast,
            })
          }
        >
          {t("diagnostics.forward.start")}
        </button>
        {window?.open && (
          <>
            <span className="diagnostics-window-open">
              {t("diagnostics.forward.remaining", {
                remaining: formatRemaining(window.remaining_secs),
              })}
            </span>
            <button
              type="button"
              className="stdbtn"
              disabled={closeWindow.isPending}
              onClick={() => closeWindow.mutate()}
            >
              {t("diagnostics.forward.stop")}
            </button>
          </>
        )}
      </div>
      {window && (window.sent > 0 || window.failed > 0 || window.last_error) && (
        <p className="settings-hint">
          {t("diagnostics.forward.counters", { sent: window.sent, failed: window.failed })}
          {window.last_error ? ` · ${window.last_error}` : ""}
        </p>
      )}

      {issuesOpen && (
        <Modal onClose={() => setIssuesOpen(false)} width={900} textAlign="left">
          <h2 className="share-modal-title">{t("diagnostics.issues.title")}</h2>
          <IssuesModal />
        </Modal>
      )}
      {bundleOpen && (
        <Modal onClose={() => setBundleOpen(false)} width={760} textAlign="left">
          <h2 className="share-modal-title">{t("diagnostics.bundle.title")}</h2>
          <BundleModal />
        </Modal>
      )}
    </div>
  )
}
