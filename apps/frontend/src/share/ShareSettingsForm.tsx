import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { Input, NullableNumberInput, Select, Switch } from "@/components/common-ui/Form"
import { toast } from "@/toast"

import { useSaveShareSettings, useShareSettings } from "./api"
import type { ShareSettings } from "./types"

export interface SettingField {
  key: string
  kind: "bool" | "number" | "text" | "lines" | "choice"
  /** For `choice`: the allowed values; each label is `${label}.${value}`. */
  options?: string[]
  /** i18n key of the label; the hint, when present, is `${label}Hint`. */
  label: string
  hint?: boolean
  placeholder?: string
}

type Value = string | number | boolean | null

/** One block of `share_*` settings with its own Save button. Only changed fields are sent, so two
 * blocks never overwrite each other's edits. */
export function ShareSettingsForm({ fields }: { fields: SettingField[] }) {
  const settings = useShareSettings()
  if (!settings.data) return <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>
  // Re-keyed on every fetch, so the draft restarts from what the server now holds.
  return <SettingsDraft key={settings.dataUpdatedAt} fields={fields} saved={settings.data} />
}

function SettingsDraft({ fields, saved }: { fields: SettingField[]; saved: ShareSettings }) {
  const { t } = useTranslation()
  const save = useSaveShareSettings()
  const [draft, setDraft] = useState<ShareSettings>(saved)

  const changed = fields.filter((f) => draft[f.key] !== saved[f.key])
  const set = (key: string, value: Value) => setDraft((d) => ({ ...d, [key]: value }))
  const submit = () => {
    const body: ShareSettings = {}
    for (const f of changed) body[f.key] = draft[f.key] ?? ""
    save.mutate(body, {
      onSuccess: () => toast({ heading: t("share.settings.saved"), icon: "success", hideAfter: 3000 }),
      onError: (e) =>
        toast({ heading: t("share.settings.saveFailed"), text: e instanceof ApiError ? e.message : String(e), icon: "error" }),
    })
  }

  return (
    <div className="share-form">
      {fields.map((f) => (
        <FieldRow key={f.key} field={f} value={(draft[f.key] as Value | undefined) ?? null} onChange={(v) => set(f.key, v)} label={t(f.label)}>
          {f.hint && <span className="share-form-hint">{t(`${f.label}Hint`)}</span>}
        </FieldRow>
      ))}
      <span />
      <div>
        <button type="button" className="stdbtn" disabled={changed.length === 0 || save.isPending} onClick={submit}>
          {t("share.settings.save")}
        </button>
      </div>
    </div>
  )
}

function FieldRow({
  field,
  value,
  onChange,
  label,
  children,
}: {
  field: SettingField
  value: Value
  onChange: (v: Value) => void
  label: string
  children?: React.ReactNode
}) {
  const id = `share-setting-${field.key}`
  const { t } = useTranslation()
  let control: React.ReactNode
  switch (field.kind) {
    case "choice":
      control = (
        <Select
          value={String(value ?? "")}
          onValueChange={onChange}
          align="start"
          items={(field.options ?? []).map((o) => ({ value: o, label: t(`${field.label}.${o}`) }))}
        />
      )
      break
    case "bool":
      control = <Switch checked={value === true} onCheckedChange={onChange} />
      break
    case "number":
      control = (
        <NullableNumberInput
          id={id}
          value={typeof value === "number" ? value : ""}
          onValueChange={(v) => onChange(v === "" ? null : v)}
          placeholder={field.placeholder}
          style={{ width: "10em" }}
        />
      )
      break
    case "lines":
      control = (
        <Input rows={3} value={String(value ?? "")} onValueChange={onChange} placeholder={field.placeholder} />
      )
      break
    default:
      control = <Input id={id} value={String(value ?? "")} onValueChange={onChange} placeholder={field.placeholder} />
  }
  return (
    <>
      <label htmlFor={id}>{label}</label>
      {control}
      {children}
    </>
  )
}
