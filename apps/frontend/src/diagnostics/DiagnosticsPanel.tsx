/** The diagnostics panel (specs/018), shown in its own modal from the settings page's left column. */

import "./diagnostics.css"

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { CheckboxField, NumberInput, Select } from "@/components/common-ui/Form"
import { toast } from "@/toast"

import {
  type DiagnosticsSettings,
  type ForwardWindow,
  useCloseWindow,
  useDiagnosticsOverview,
  useOpenWindow,
  useSaveDiagnosticsSettings,
} from "./api"

/** The project's own tracker (see `OFFICIAL_TRACKER_HOST` server-side). */
const OFFICIAL_TRACKER_HOST = "https://sentry.taosaka.com"

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

function officialHost(settings: DiagnosticsSettings): boolean {
  return (
    settings.tracker_official === true ||
    (settings.tracker_host ?? "").replace(/\/$/, "") === OFFICIAL_TRACKER_HOST
  )
}

export function DiagnosticsPanel() {
  const { t } = useTranslation()
  const overview = useDiagnosticsOverview()
  const save = useSaveDiagnosticsSettings()
  const openWindow = useOpenWindow()
  const closeWindow = useCloseWindow()
  const [duration, setDuration] = useState("300")

  const settings = overview.data?.settings
  const usage = overview.data?.usage
  const drops = overview.data?.drops
  const window: ForwardWindow | undefined = overview.data?.window
  // The overview refreshes every 30 s, which is far too coarse for a five-minute countdown. The
  // clock below ticks once a second; before the first tick the server's own `remaining_secs` is
  // shown, and the server stays the authority on when the window actually ends.
  const [now, setNow] = useState(0)
  useEffect(() => {
    if (!window?.open) return
    const tick = () => setNow(Date.now())
    tick()
    const timer = setInterval(tick, 1_000)
    return () => clearInterval(timer)
  }, [window?.open])
  const remaining = window
    ? now > 0
      ? Math.max(0, Math.ceil((window.deadline - now) / 1000))
      : window.remaining_secs
    : 0
  const windowOpen = window?.open === true && remaining > 0

  const onErrorToast = (error: unknown) =>
    toast({ icon: "error", text: error instanceof Error ? error.message : String(error) })
  const update = (body: Record<string, unknown>) => save.mutate(body, { onError: onErrorToast })

  return (
    <div className="diag-modal">
      <p className="diag-tagline">{t("diagnostics.hint")}</p>

      <div className="diag-group">
        <h3 className="diag-group-title">{t("diagnostics.group.capture")}</h3>
        <div className="diag-toggles">
          <div className="diag-toggle-lead">
            <CheckboxField
              checked={settings?.diagnostics_enabled ?? false}
              onCheckedChange={(checked) => update({ diagnostics_enabled: checked })}
            >
              {t("diagnostics.enabled")}
            </CheckboxField>
          </div>
          <div className="diag-toggle-sources">
            <CheckboxField
              checked={settings?.diagnostics_web ?? false}
              onCheckedChange={(checked) => update({ diagnostics_web: checked })}
            >
              {t("diagnostics.sourceWeb")}
            </CheckboxField>
            <CheckboxField
              checked={settings?.diagnostics_server ?? false}
              onCheckedChange={(checked) => update({ diagnostics_server: checked })}
            >
              {t("diagnostics.sourceServer")}
            </CheckboxField>
            <CheckboxField
              checked={settings?.diagnostics_plugins ?? false}
              onCheckedChange={(checked) => update({ diagnostics_plugins: checked })}
            >
              {t("diagnostics.sourcePlugins")}
            </CheckboxField>
          </div>
          <div className="diag-toggle-sources">
            <CheckboxField
              checked={settings?.diagnostics_breadcrumbs ?? false}
              onCheckedChange={(checked) => update({ diagnostics_breadcrumbs: checked })}
            >
              {t("diagnostics.breadcrumbs")}
            </CheckboxField>
          </div>
          <div className="diag-span">
            <CheckboxField
              checked={settings?.diagnostics_include_private_data ?? true}
              onCheckedChange={(checked) =>
                update({ diagnostics_include_private_data: checked })
              }
            >
              {t("diagnostics.includePrivate")}
            </CheckboxField>
            <p className="diag-note">{t("diagnostics.includePrivateHint")}</p>
          </div>
        </div>
      </div>

      <div className="diag-group">
        <h3 className="diag-group-title">{t("diagnostics.group.storage")}</h3>
        <div className="diag-grid">
          <label className="diag-field">
            <span>{t("diagnostics.retentionDays")}</span>
            <NumberInput
              value={settings?.diagnostics_retention_days ?? 7}
              min={1}
              onValueChange={(value) => update({ diagnostics_retention_days: value })}
            />
          </label>
          <label className="diag-field">
            <span>{t("diagnostics.maxEvents")}</span>
            <NumberInput
              value={settings?.diagnostics_max_events ?? 50000}
              min={100}
              onValueChange={(value) => update({ diagnostics_max_events: value })}
            />
          </label>
          <label className="diag-field">
            <span>{t("diagnostics.maxBytes")}</span>
            <NumberInput
              value={settings?.diagnostics_max_bytes ?? 536870912}
              min={1048576}
              onValueChange={(value) => update({ diagnostics_max_bytes: value })}
            />
          </label>
        </div>
        <p className="diag-usage">
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
      </div>

      <div className="diag-group">
        <h3 className="diag-group-title">{t("diagnostics.group.report")}</h3>
        <div className="diag-actions">
          <a className="stdbtn" href="/logs">
            {t("diagnostics.rawLogs")}
          </a>
        </div>
      </div>

      <div className="diag-group">
        <h3 className="diag-group-title">{t("diagnostics.forward.title")}</h3>
        <p className="diag-tagline">{t("diagnostics.forward.hint")}</p>
        <p className="diag-tagline">{t("diagnostics.forward.whatIsSent")}</p>
        <div className="diag-inline">
          <span className="diag-usage">
            {settings?.tracker_configured
              ? settings.tracker_host
                ? t("diagnostics.forward.configuredAt", { host: settings.tracker_host })
                : t("diagnostics.forward.configured")
              : t("diagnostics.forward.notConfigured")}
          </span>
        </div>
        {settings && Object.keys(settings.tracker_routes ?? {}).length > 0 && (
          <p className="diag-tagline">
            {t("diagnostics.forward.routes", {
              routes: Object.entries(settings.tracker_routes)
                .map(([source, host]) => `${source} → ${host}`)
                .join(", "),
            })}
          </p>
        )}
        <p className="diag-note">{t("diagnostics.forward.whereToChange")}</p>
        {settings?.tracker_configured && officialHost(settings) && (
          <div className="diag-official">
            <span className="diag-official-badge">
              <i className="fas fa-shield-halved" aria-hidden="true" />
              <code>{settings.tracker_host}</code>
            </span>
            <span>{t("diagnostics.forward.officialSuffix")}</span>
          </div>
        )}
        {settings?.tracker_configured && (
          <p className="diag-tagline">
            {settings.tracker_host
              ? t("diagnostics.forward.where", { host: settings.tracker_host })
              : t("diagnostics.forward.whereUnknown")}
            {settings.tracker_built_in ? t("diagnostics.forward.builtIn") : ""}
            {" "}
            {t("diagnostics.forward.trusted")}
          </p>
        )}
        <div className="diag-inline">
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
              openWindow.mutate(Number(duration), { onError: onErrorToast })
            }
          >
            {t("diagnostics.forward.start")}
          </button>
          {windowOpen && (
            <>
              <span className="diag-window-open">
                {t("diagnostics.forward.remaining", {
                  remaining: formatRemaining(remaining),
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
          <p className="diag-usage">
            {t("diagnostics.forward.counters", { sent: window.sent, failed: window.failed })}
            {window.last_error ? ` · ${window.last_error}` : ""}
          </p>
        )}
      </div>

    </div>
  )
}
