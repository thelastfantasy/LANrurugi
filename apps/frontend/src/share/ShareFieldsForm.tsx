import { useTranslation } from "react-i18next"

import { CheckboxField, Input, NullableNumberInput, Switch } from "@/components/common-ui/Form"

import type { ShareFields, SharePermissions } from "./types"

/** Editable share settings as the form holds them. */
export interface ShareFormState {
  title: string
  permissions: SharePermissions
  downloadLimit: number | ""
  ipAllow: string
  ipFirstN: number | ""
  /** Unix seconds; `""` never expires. Held as the instant itself, not a day count, so opening and
   * saving a share leaves its expiry exactly where it was. */
  expiresAt: number | ""
  embed: boolean
  allowHelpers: boolean
}

const DAY = 86_400

const nowSecs = () => Math.floor(Date.now() / 1000)

/** Whole days left until `expiresAt`, rounded up, as the days field shows it. */
export function daysUntil(expiresAt: number, now = nowSecs()): number {
  return Math.max(1, Math.ceil((expiresAt - now) / DAY))
}

/** `YYYY-MM-DD` of the local day `expiresAt` falls on, as a date input wants it. */
export function localDateOf(expiresAt: number): string {
  const d = new Date(expiresAt * 1000)
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** A picked date means "through that day": the last second of it, local time. */
export function endOfLocalDay(date: string): number | "" {
  const [y, m, d] = date.split("-").map(Number)
  if (!y || !m || !d) return ""
  return Math.floor(new Date(y, m - 1, d, 23, 59, 59).getTime() / 1000)
}

/** The expiry `days` from now, the way a days count has always been applied. */
export function expiryInDays(days: number | ""): number | "" {
  return days === "" ? "" : nowSecs() + days * DAY
}

export function formFromShare(s: {
  title: string
  permissions: SharePermissions
  download_limit: number | null
  ip_allow?: string[]
  ip_first_n?: number | null
  expires_at: number | null
  embed: boolean
  allow_helpers: boolean
}): ShareFormState {
  return {
    title: s.title,
    permissions: s.permissions,
    downloadLimit: s.download_limit ?? "",
    ipAllow: (s.ip_allow ?? []).join("\n"),
    ipFirstN: s.ip_first_n ?? "",
    expiresAt: s.expires_at ?? "",
    embed: s.embed,
    allowHelpers: s.allow_helpers,
  }
}

export function fieldsFromForm(f: ShareFormState): ShareFields {
  return {
    title: f.title.trim() || null,
    permissions: f.permissions,
    download_limit: f.downloadLimit === "" ? null : f.downloadLimit,
    ip_allow: f.ipAllow
      .split(/[\n,]/)
      .map((s) => s.trim())
      .filter(Boolean),
    ip_first_n: f.ipFirstN === "" ? null : f.ipFirstN,
    expires_at: f.expiresAt === "" ? null : f.expiresAt,
    embed: f.embed,
    allow_helpers: f.allowHelpers,
  }
}

export function permissionsChosen(f: ShareFormState): boolean {
  return f.permissions.import || f.permissions.read || f.permissions.download
}

/** The share settings editor used by the share dialog and the Settings → Sharing detail panel. */
export function ShareFieldsForm({
  value,
  onChange,
  embedAvailable,
}: {
  value: ShareFormState
  onChange: (next: ShareFormState) => void
  /** Embedding needs a public URL, since the card is served by this instance (FR-030). */
  embedAvailable: boolean
}) {
  const { t } = useTranslation()
  const set = (patch: Partial<ShareFormState>) => onChange({ ...value, ...patch })
  const setPermission = (key: keyof SharePermissions, on: boolean) =>
    set({ permissions: { ...value.permissions, [key]: on } })
  return (
    <div className="share-form">
      <label htmlFor="share-title">{t("share.form.title")}</label>
      <Input id="share-title" value={value.title} onValueChange={(v) => set({ title: v })} />

      <span>{t("share.form.permissions")}</span>
      <div className="share-form-checks">
        {(["import", "read", "download"] as const).map((key) => (
          <CheckboxField key={key} checked={value.permissions[key]} onCheckedChange={(on) => setPermission(key, on)}>
            {t(`share.permission.${key}`)}
          </CheckboxField>
        ))}
      </div>
      {!permissionsChosen(value) && <span className="share-form-hint">{t("share.form.permissionsRequired")}</span>}

      <span>{t("share.form.downloadLimit")}</span>
      <NullableNumberInput
        value={value.downloadLimit}
        min={1}
        onValueChange={(v) => set({ downloadLimit: v })}
        placeholder={t("share.form.unlimited")}
        style={{ width: "8em" }}
      />
      <span className="share-form-hint">{t("share.form.downloadLimitHint")}</span>

      <span>{t("share.form.ipAllow")}</span>
      <Input
        rows={3}
        value={value.ipAllow}
        onValueChange={(v) => set({ ipAllow: v })}
        placeholder="192.168.1.0/24"
      />
      <span className="share-form-hint">{t("share.form.ipHint")}</span>

      <span>{t("share.form.firstN")}</span>
      <NullableNumberInput
        value={value.ipFirstN}
        min={1}
        onValueChange={(v) => set({ ipFirstN: v })}
        placeholder={t("share.form.off")}
        style={{ width: "8em" }}
      />

      <span>{t("share.form.expiry")}</span>
      <div className="share-expiry">
        <NullableNumberInput
          value={value.expiresAt === "" ? "" : daysUntil(value.expiresAt)}
          min={1}
          onValueChange={(v) => set({ expiresAt: expiryInDays(v) })}
          placeholder={t("share.form.never")}
          style={{ width: "8em" }}
          aria-label={t("share.form.expiryDays")}
        />
        <span>{t("share.form.daysOrUntil")}</span>
        <Input
          type="date"
          value={value.expiresAt === "" ? "" : localDateOf(value.expiresAt)}
          min={localDateOf(nowSecs())}
          onValueChange={(v) => set({ expiresAt: v ? endOfLocalDay(v) : "" })}
          aria-label={t("share.form.expiryDate")}
          style={{ width: "auto" }}
        />
      </div>
      <span className="share-form-hint">{t("share.form.expiryHint")}</span>

      <span>{t("share.form.embed")}</span>
      <Switch checked={value.embed && embedAvailable} disabled={!embedAvailable} onCheckedChange={(on) => set({ embed: on })} />
      {!embedAvailable && <span className="share-form-hint">{t("share.form.embedNeedsPublicUrl")}</span>}

      <span>{t("share.form.allowHelpers")}</span>
      <Switch checked={value.allowHelpers} onCheckedChange={(on) => set({ allowHelpers: on })} />
      <span className="share-form-hint">{t("share.form.helpersIpNote")}</span>
    </div>
  )
}
