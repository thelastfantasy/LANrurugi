import { useState } from "react"
import { useTranslation } from "react-i18next"

import { CheckboxField, Input, Switch } from "@/components/common-ui/Form"
import { useSaveShareSettings, useShareSettings } from "@/share/api"
import { copyWithToast } from "@/share/clipboard"
import type { ShareEndpointStatus, ShareServiceStatus, ShareSettings } from "@/share/types"
import { toast } from "@/toast"

/** Where this instance can be reached for sharing, and which proxy it follows (T054, FR-051). */
export function SharingEndpointStatus() {
  const { t } = useTranslation()
  const settings = useShareSettings()
  const endpoint = settings.data?.endpoint as ShareEndpointStatus | undefined
  const services = (settings.data?.services as ShareServiceStatus[] | undefined) ?? []
  if (!endpoint) return null
  const list = (values: string[] | undefined) =>
    values?.length ? (
      <ul className="share-status-values">
        {values.map((v) => (
          <li key={v}>
            <code>{v}</code>
          </li>
        ))}
      </ul>
    ) : (
      <span>{t("share.status.none")}</span>
    )
  return (
    <div className="share-status-grid">
      {endpoint.running ? (
        <>
          <span>{t("share.status.endpoint")}</span>
          <code>{endpoint.endpoint_id?.slice(0, 16)}…</code>
          <span>{t("share.status.relays")}</span>
          {list(endpoint.relays)}
          <span>{t("share.status.direct")}</span>
          {list(endpoint.direct_addresses)}
          <span>{t("share.panel.proxy")}</span>
          <span>{endpoint.proxy ? t("share.status.followingProxy", { proxy: endpoint.proxy }) : t("share.status.noProxy")}</span>
        </>
      ) : (
        <span className="share-status-wide">{t("share.status.notRunning")}</span>
      )}
      {services.length > 0 && (
        <>
          <span>{t("share.panel.services")}</span>
          <ul className="share-status-values">
            {services.map((s) => (
              <li key={s.url}>
                {s.reachable === null ? (
                  <span className="share-badge">{t("share.status.unknown")}</span>
                ) : (
                  <span className={`share-badge ${s.reachable ? "share-status-active" : "share-status-exhausted"}`}>
                    {s.reachable ? t("share.status.reachable") : t("share.status.unreachable")}
                  </span>
                )}{" "}
                {t(`share.serviceKind.${s.kind}`, { defaultValue: s.kind })} <code>{s.url}</code>
                {s.last_ok_at ? (
                  <span className="share-result-note"> · {new Date(s.last_ok_at * 1000).toLocaleString()}</span>
                ) : null}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  )
}

/** One line on whether the endpoint runs and how it is reachable, for the settings overview. */
export function SharingEndpointSummary() {
  const { t } = useTranslation()
  const settings = useShareSettings()
  const endpoint = settings.data?.endpoint as ShareEndpointStatus | undefined
  if (!endpoint) return null
  if (!endpoint.running) return <>{t("share.status.notRunning")}</>
  return (
    <>
      {t("share.panel.connectivitySummary", {
        direct: endpoint.direct_addresses?.length ?? 0,
        relays: endpoint.relays?.length ?? 0,
      })}
    </>
  )
}

/** This instance's `lrrg-node:` string, for friends to add, when it offers a node role. */
export function SharingNodeString() {
  const { t } = useTranslation()
  const settings = useShareSettings()
  const node = (settings.data?.endpoint as ShareEndpointStatus & { node_string?: string | null } | undefined)?.node_string
  if (!node) return <p className="share-result-note">{t("share.nodes.noString")}</p>
  return (
    <div className="share-result-row">
      <code className="share-result-value">{node}</code>
      <button
        type="button"
        className="stdbtn"
        onClick={() => void copyWithToast(node, t("share.result.copied"), t("share.result.copyFailed"))}
      >
        {t("share.result.copy")}
      </button>
    </div>
  )
}

interface Schedule {
  enabled: boolean
  windows: { from: string; to: string }[]
  days: number[]
}

function parseSchedule(raw: unknown): Schedule {
  try {
    const value = JSON.parse(String(raw ?? "")) as Partial<Schedule>
    return { enabled: Boolean(value.enabled), windows: value.windows ?? [], days: value.days ?? [] }
  } catch {
    return { enabled: false, windows: [], days: [] }
  }
}

/** The alternative-speed schedule and the status line of which set is in effect (FR-042). */
export function AltScheduleEditor() {
  const settings = useShareSettings()
  if (!settings.data) return null
  return <ScheduleDraft key={settings.dataUpdatedAt} saved={settings.data} />
}

function ScheduleDraft({ saved }: { saved: ShareSettings }) {
  const { t } = useTranslation()
  const save = useSaveShareSettings()
  const [schedule, setSchedule] = useState<Schedule>(() => parseSchedule(saved.share_alt_schedule))
  const mode = saved.upload_mode === "alternative" ? t("share.limits.alternative") : t("share.limits.normal")
  const next = typeof saved.next_switch_at === "number" ? new Date(saved.next_switch_at * 1000).toLocaleString() : null
  const days = [1, 2, 3, 4, 5, 6, 7]
  const setWindow = (i: number, key: "from" | "to", value: string) =>
    setSchedule((s) => ({ ...s, windows: s.windows.map((w, j) => (j === i ? { ...w, [key]: value } : w)) }))
  return (
    <div className="share-form">
      <span>{t("share.limits.inEffect")}</span>
      <span>
        {mode}
        {saved.upload_mode_source === "manual" ? ` · ${t("share.limits.manual")}` : ""}
        {next ? ` · ${t("share.limits.nextSwitch", { time: next })}` : ""}
      </span>
      <span>{t("share.limits.schedule")}</span>
      <Switch checked={schedule.enabled} onCheckedChange={(on) => setSchedule((s) => ({ ...s, enabled: on }))} />
      {schedule.windows.map((w, i) => (
        <div key={i} className="share-schedule-window">
          <Input value={w.from} onValueChange={(v) => setWindow(i, "from", v)} placeholder="01:00" />
          –
          <Input value={w.to} onValueChange={(v) => setWindow(i, "to", v)} placeholder="08:00" />
          <button
            type="button"
            className="favtag-btn"
            onClick={() => setSchedule((s) => ({ ...s, windows: s.windows.filter((_, j) => j !== i) }))}
          >
            {t("share.detail.removeItem")}
          </button>
        </div>
      ))}
      <span />
      <div>
        <button
          type="button"
          className="favtag-btn"
          onClick={() => setSchedule((s) => ({ ...s, windows: [...s.windows, { from: "01:00", to: "08:00" }] }))}
        >
          {t("share.limits.addWindow")}
        </button>
      </div>
      <span>{t("share.limits.days")}</span>
      <div className="share-form-checks">
        {days.map((d) => (
          <CheckboxField
            key={d}
            checked={schedule.days.includes(d)}
            onCheckedChange={(on) =>
              setSchedule((s) => ({ ...s, days: on ? [...s.days, d].sort() : s.days.filter((x) => x !== d) }))
            }
          >
            {t(`share.limits.day${d}`)}
          </CheckboxField>
        ))}
      </div>
      <span className="share-form-hint">{t("share.limits.daysHint")}</span>
      <span />
      <div>
        <button
          type="button"
          className="stdbtn"
          disabled={save.isPending}
          onClick={() =>
            save.mutate(
              { share_alt_schedule: JSON.stringify(schedule) },
              {
                onSuccess: () => toast({ heading: t("share.settings.saved"), icon: "success", hideAfter: 3000 }),
                onError: () => toast({ heading: t("share.settings.saveFailed"), icon: "error" }),
              },
            )
          }
        >
          {t("share.settings.save")}
        </button>
      </div>
    </div>
  )
}
