import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ApiError } from "@/api/client";
import {
  Input,
  NullableNumberInput,
  Select,
  Switch,
} from "@/components/common-ui/Form";
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme";
import { toast } from "@/toast";

import { useSaveShareSettings, useShareSettings } from "./api";
import type { ShareSettings } from "./types";

export interface SettingField {
  key: string;
  kind: "bool" | "number" | "text" | "lines" | "choice" | "flags";
  /** For `choice`: the allowed values, labelled `${label}.${value}`. For `flags`: the individual
   *  values, stored as a comma-joined list and rendered as one checkbox each. */
  options?: string[];
  /** Per-value i18n keys, when the block's own label prefix does not fit them (the permission
   *  names are shared with the share dialog's own checkboxes). A key with no translation is
   *  rendered as-is, which is what per-interface labels want. */
  optionLabels?: Record<string, string>;
  /** `"interfaces"`: the options are the host's network interfaces, filled in from the settings
   *  payload at render time — they change while the app runs (a VPN tunnel appears). */
  dynamic?: "interfaces";
  /** i18n key of the label; the hint, when present, is `${label}Hint`. */
  label: string;
  hint?: boolean;
  placeholder?: string;
  /** Render only while another field of the same block has this value — for settings that only
   *  matter in one mode (the relay's own PEM files, which ACME mode does not use at all). */
  onlyWhen?: { key: string; value: string };
  /** Hidden behind the block's "advanced" switch: a setting almost nobody needs to touch, whose
   *  default is already right. */
  advanced?: boolean;
}

type Value = string | number | boolean | null;

/** One block of `share_*` settings with its own Save button. Only changed fields are sent, so two
 * blocks never overwrite each other's edits. */
export function ShareSettingsForm({ fields }: { fields: SettingField[] }) {
  const settings = useShareSettings();
  if (!settings.data)
    return <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>;
  // Re-keyed on every fetch, so the draft restarts from what the server now holds.
  return (
    <SettingsDraft
      key={settings.dataUpdatedAt}
      fields={fillDynamicOptions(fields, settings.data)}
      saved={settings.data}
    />
  );
}

/** Turns `dynamic: "interfaces"` fields into real choices: "any" first, then each interface with
 *  the addresses it currently holds, so a wrong pick is visible before it is saved. */
function fillDynamicOptions(
  fields: SettingField[],
  settings: Record<string, unknown>,
): SettingField[] {
  return fields.map((field) => {
    if (field.dynamic !== "interfaces") return field;
    const interfaces = (
      Array.isArray(settings.interfaces) ? settings.interfaces : []
    ) as { name: string; addrs: string[] }[];
    const optionLabels: Record<string, string> = {
      "": "share.network.interfaceAny",
    };
    for (const iface of interfaces) {
      optionLabels[iface.name] = iface.addrs.length
        ? `${iface.name} (${iface.addrs.join(", ")})`
        : iface.name;
    }
    return { ...field, options: ["", ...interfaces.map((i) => i.name)], optionLabels };
  });
}

function SettingsDraft({
  fields,
  saved,
}: {
  fields: SettingField[];
  saved: ShareSettings;
}) {
  const { t } = useTranslation();
  const save = useSaveShareSettings();
  const [draft, setDraft] = useState<ShareSettings>(saved);

  const changed = fields.filter((f) => draft[f.key] !== saved[f.key]);
  const set = (key: string, value: Value) =>
    setDraft((d) => ({ ...d, [key]: value }));
  const submit = () => {
    const body: ShareSettings = {};
    for (const f of changed) body[f.key] = draft[f.key] ?? "";
    save.mutate(body, {
      onSuccess: () =>
        toast({
          heading: t("share.settings.saved"),
          icon: "success",
          hideAfter: 3000,
        }),
      onError: (e) =>
        toast({
          heading: t("share.settings.saveFailed"),
          text: e instanceof ApiError ? e.message : String(e),
          icon: "error",
        }),
    });
  };

  return (
    <div className="share-form">
      {fields
        .filter(
          (f) =>
            !f.onlyWhen ||
            String(draft[f.onlyWhen.key] ?? "") === f.onlyWhen.value,
        )
        .map((f) => (
          <FieldRow
            key={f.key}
            field={f}
            value={(draft[f.key] as Value | undefined) ?? null}
            onChange={(v) => set(f.key, v)}
            label={t(f.label)}
          >
            {f.hint && (
              <span className="share-form-hint">{t(`${f.label}Hint`)}</span>
            )}
          </FieldRow>
        ))}
      <span />
      <div>
        <button
          type="button"
          className="stdbtn"
          disabled={changed.length === 0 || save.isPending}
          onClick={submit}
        >
          {t("share.settings.save")}
        </button>
      </div>
    </div>
  );
}

function FieldRow({
  field,
  value,
  onChange,
  label,
  children,
}: {
  field: SettingField;
  value: Value;
  onChange: (v: Value) => void;
  label: string;
  children?: React.ReactNode;
}) {
  const id = `share-setting-${field.key}`;
  const { t } = useTranslation();
  let control: React.ReactNode;
  switch (field.kind) {
    case "choice":
      control = (
        <Select
          popupZIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
          value={String(value ?? "")}
          onValueChange={onChange}
          align="start"
          items={(field.options ?? []).map((o) => ({
            value: o,
            label: t(field.optionLabels?.[o] ?? `${field.label}.${o}`),
          }))}
        />
      );
      break;
    case "flags": {
      // One checkbox per value, stored as the comma-joined list the server already parses —
      // "import,read,download" typed by hand is exactly the kind of field people get wrong.
      const picked = new Set(
        String(value ?? "")
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean),
      );
      const options = field.options ?? [];
      control = (
        <span
          style={{
            display: "flex",
            gap: 10,
            alignItems: "center",
            flexWrap: "wrap",
          }}
        >
          {options.map((o) => (
            <label
              key={o}
              style={{ display: "flex", alignItems: "center", gap: 4 }}
            >
              <input
                type="checkbox"
                checked={picked.has(o)}
                onChange={(e) => {
                  const next = new Set(picked);
                  if (e.target.checked) next.add(o);
                  else next.delete(o);
                  // Keep the server's own order rather than click order.
                  onChange(options.filter((x) => next.has(x)).join(","));
                }}
              />
              {t(field.optionLabels?.[o] ?? `${field.label}.${o}`)}
            </label>
          ))}
        </span>
      );
      break;
    }
    case "bool":
      control = <Switch checked={value === true} onCheckedChange={onChange} />;
      break;
    case "number":
      control = (
        <NullableNumberInput
          id={id}
          value={typeof value === "number" ? value : ""}
          onValueChange={(v) => onChange(v === "" ? null : v)}
          placeholder={field.placeholder}
          style={{ width: "10em" }}
        />
      );
      break;
    case "lines":
      control = (
        <Input
          rows={3}
          value={String(value ?? "")}
          onValueChange={onChange}
          placeholder={field.placeholder}
        />
      );
      break;
    default:
      control = (
        <Input
          id={id}
          value={String(value ?? "")}
          onValueChange={onChange}
          placeholder={field.placeholder}
        />
      );
  }
  return (
    <>
      <label htmlFor={id}>{label}</label>
      {control}
      {children}
    </>
  );
}
