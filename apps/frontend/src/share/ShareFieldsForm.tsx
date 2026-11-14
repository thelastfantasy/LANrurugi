import { useTranslation } from "react-i18next";

import {
  CheckboxField,
  Input,
  NullableNumberInput,
  Select,
  Switch,
} from "@/components/common-ui/Form";
import { DateTimeField } from "@/components/common-ui/Form/DateTimeField";
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme";

import type { RatioAction, ShareFields, SharePermissions } from "./types";

/** Editable share settings as the form holds them. */
/** One click each, because these are the ratios people actually pick. */
const RATIO_PRESETS = [1, 1.1, 5, 10];

export interface ShareFormState {
  title: string;
  permissions: SharePermissions;
  downloadLimit: number | "";
  /** Imported bytes over the share's own size; `""` means no target. */
  targetRatio: number | "";
  ratioAction: RatioAction;
  /** Bytes per second once throttled; `""` pauses instead. */
  ratioRate: number | "";
  ipAllow: string;
  ipFirstN: number | "";
  /** Unix seconds; `""` never expires. Held as the instant itself, not a day count, so opening and
   * saving a share leaves its expiry exactly where it was. */
  expiresAt: number | "";
  embed: boolean;
  allowHelpers: boolean;
}

const DAY = 86_400;

const nowSecs = () => Math.floor(Date.now() / 1000);

/** Whole days left until `expiresAt`, rounded up, as the days field shows it. */
export function daysUntil(expiresAt: number, now = nowSecs()): number {
  return Math.max(1, Math.ceil((expiresAt - now) / DAY));
}

/** `YYYY-MM-DD` of the local day `expiresAt` falls on, as a date input wants it. */
export function localDateOf(expiresAt: number): string {
  const d = new Date(expiresAt * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** A picked date means "through that day": the last second of it, local time. */
export function endOfLocalDay(date: string): number | "" {
  const [y, m, d] = date.split("-").map(Number);
  if (!y || !m || !d) return "";
  return Math.floor(new Date(y, m - 1, d, 23, 59, 59).getTime() / 1000);
}

/** The expiry `days` from now, the way a days count has always been applied. */
export function expiryInDays(days: number | ""): number | "" {
  return days === "" ? "" : nowSecs() + days * DAY;
}

export function formFromShare(s: {
  title: string;
  permissions: SharePermissions;
  download_limit: number | null;
  target_ratio?: number | null;
  ratio_action?: RatioAction;
  ratio_rate?: number | null;
  ip_allow?: string[];
  ip_first_n?: number | null;
  expires_at: number | null;
  embed: boolean;
  allow_helpers: boolean;
}): ShareFormState {
  return {
    title: s.title,
    permissions: s.permissions,
    downloadLimit: s.download_limit ?? "",
    targetRatio: s.target_ratio ?? "",
    ratioAction: s.ratio_action ?? "stop",
    ratioRate: s.ratio_rate ?? "",
    ipAllow: (s.ip_allow ?? []).join("\n"),
    ipFirstN: s.ip_first_n ?? "",
    expiresAt: s.expires_at ?? "",
    embed: s.embed,
    allowHelpers: s.allow_helpers,
  };
}

export function fieldsFromForm(f: ShareFormState): ShareFields {
  return {
    title: f.title.trim() || null,
    permissions: f.permissions,
    download_limit: f.downloadLimit === "" ? null : f.downloadLimit,
    target_ratio: f.targetRatio === "" ? null : f.targetRatio,
    ratio_action: f.ratioAction,
    ratio_rate: f.ratioRate === "" ? null : f.ratioRate,
    ip_allow: f.ipAllow
      .split(/[\n,]/)
      .map((s) => s.trim())
      .filter(Boolean),
    ip_first_n: f.ipFirstN === "" ? null : f.ipFirstN,
    expires_at: f.expiresAt === "" ? null : f.expiresAt,
    embed: f.embed,
    allow_helpers: f.allowHelpers,
  };
}

export function permissionsChosen(f: ShareFormState): boolean {
  return f.permissions.import || f.permissions.read || f.permissions.download;
}

/** The share settings editor used by the share dialog and the Settings → Sharing detail panel. */
export function ShareFieldsForm({
  value,
  onChange,
  embedAvailable,
}: {
  value: ShareFormState;
  onChange: (next: ShareFormState) => void;
  /** Embedding needs a public URL, since the card is served by this instance (FR-030). */
  embedAvailable: boolean;
}) {
  const { t } = useTranslation();
  const set = (patch: Partial<ShareFormState>) =>
    onChange({ ...value, ...patch });
  const setPermission = (key: keyof SharePermissions, on: boolean) =>
    set({ permissions: { ...value.permissions, [key]: on } });
  return (
    <div className="share-form">
      <label htmlFor="share-title">{t("share.form.title")}</label>
      <Input
        id="share-title"
        value={value.title}
        onValueChange={(v) => set({ title: v })}
      />

      <span>{t("share.form.permissions")}</span>
      <div className="share-form-checks">
        {(["import", "read", "download"] as const).map((key) => (
          <CheckboxField
            key={key}
            checked={value.permissions[key]}
            onCheckedChange={(on) => setPermission(key, on)}
          >
            {t(`share.permission.${key}`)}
          </CheckboxField>
        ))}
      </div>
      {!permissionsChosen(value) && (
        <span className="share-form-hint">
          {t("share.form.permissionsRequired")}
        </span>
      )}

      <span>{t("share.form.downloadLimit")}</span>
      <NullableNumberInput
        value={value.downloadLimit}
        min={1}
        onValueChange={(v) => set({ downloadLimit: v })}
        placeholder={t("share.form.unlimited")}
        style={{ width: "8em" }}
      />
      <span className="share-form-hint">
        {t("share.form.downloadLimitHint")}
      </span>

      <span>{t("share.form.targetRatio")}</span>
      <div className="share-ratio-row">
        <NullableNumberInput
          value={value.targetRatio}
          min={0}
          step={0.1}
          onValueChange={(v) => set({ targetRatio: v })}
          placeholder={t("share.form.off")}
          style={{ width: "7em" }}
          aria-label={t("share.form.targetRatio")}
        />
        <div className="share-ratio-presets">
          {RATIO_PRESETS.map((preset) => (
            <button
              key={preset}
              type="button"
              className="stdbtn"
              onClick={() => set({ targetRatio: preset })}
            >
              {preset.toFixed(1)}
            </button>
          ))}
        </div>
      </div>
      <span className="share-form-hint">{t("share.form.targetRatioHint")}</span>

      {/* "Once reached" is meaningless with no target, so both it and the rate hide until one
          is typed — an empty target means the ratio machinery is off entirely. */}
      {value.targetRatio !== "" && (
        <>
          <span>{t("share.form.ratioAction")}</span>
          <Select
            popupZIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
            value={value.ratioAction}
            onValueChange={(v) => set({ ratioAction: v as RatioAction })}
            align="start"
            items={[
              { value: "stop", label: t("share.ratio.stop") },
              { value: "throttle", label: t("share.ratio.throttle") },
            ]}
          />
        </>
      )}
      {value.targetRatio !== "" && value.ratioAction === "throttle" && (
        <>
          <span>{t("share.form.ratioRate")}</span>
          <NullableNumberInput
            value={value.ratioRate}
            min={0}
            step={1024}
            onValueChange={(v) => set({ ratioRate: v })}
            placeholder={t("share.form.ratioRatePaused")}
            style={{ width: "10em" }}
            aria-label={t("share.form.ratioRate")}
          />
          <span className="share-form-hint">{t("share.form.ratioRateHint")}</span>
        </>
      )}

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
        <DateTimeField
          mode="date"
          ariaLabel={t("share.form.expiryDate")}
          value={value.expiresAt === "" ? "" : localDateOf(value.expiresAt)}
          onChange={(v) => set({ expiresAt: v ? endOfLocalDay(v) : "" })}
          /* One width for both states: "选择日期" is wider than "2026-11-07", so an auto width made
             the whole expiry row shift the moment a date was picked. */
          style={{ width: "10.5em" }}
        />
      </div>
      <span className="share-form-hint">{t("share.form.expiryHint")}</span>

      <span>{t("share.form.embed")}</span>
      <Switch
        checked={value.embed && embedAvailable}
        disabled={!embedAvailable}
        onCheckedChange={(on) => set({ embed: on })}
      />
      {!embedAvailable && (
        <span className="share-form-hint">
          {t("share.form.embedNeedsPublicUrl")}
        </span>
      )}

      <span>{t("share.form.allowHelpers")}</span>
      <Switch
        checked={value.allowHelpers}
        onCheckedChange={(on) => set({ allowHelpers: on })}
      />
      <span className="share-form-hint">{t("share.form.helpersIpNote")}</span>
    </div>
  );
}
