import "./share.css"

import type { TFunction } from "i18next"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { useArchiveMetadata, useTankoubon } from "@/api/hooks"
import { Modal } from "@/components/common-ui/Display"
import { CheckboxField, RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { toast } from "@/toast"

import { useAddShareItems, useCreateShare, useShare, useShares, useShareSettings } from "./api"
import { parseSharePermissions } from "./permissions"
import {
  daysUntil,
  expiryInDays,
  fieldsFromForm,
  formFromShare,
  permissionsChosen,
  ShareFieldsForm,
  type ShareFormState,
} from "./ShareFieldsForm"
import { ShareResult } from "./ShareResult"
import type { ShareCodeInfo, ShareDetail, ShareItemRef, ShareSettings } from "./types"

function defaultsFrom(settings: ShareSettings | undefined): ShareFormState {
  // A setting written as "import, read, download" still ticks all three (spaces are ignored).
  const permissions = parseSharePermissions(settings?.share_default_permissions, "import,read,download")
  const num = (v: unknown) => (typeof v === "number" ? v : "")
  return {
    title: "",
    permissions,
    downloadLimit: num(settings?.share_default_download_limit),
    ipAllow: String(settings?.share_default_ip_allow ?? "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
      .join("\n"),
    ipFirstN: num(settings?.share_default_ip_first_n),
    expiresAt: expiryInDays(num(settings?.share_default_expiry_days)),
    embed: settings?.share_default_embed === true,
    allowHelpers: settings?.share_default_allow_helpers === true,
  }
}

/** A short code is only issued for a share that expires within this many days. */
const SHORT_CODE_MAX_DAYS = 7

/** Longest first-item name kept in a several-item title, in user-perceived characters. */
const MAX_NAME_IN_TITLE = 30

/** The title a new share starts with: the item's own name, or "Share <first> and N items" with the
 * first name cut short so the whole stays readable in lists. */
export function defaultShareTitle(first: string, count: number, t: TFunction): string {
  const name = first.trim()
  if (count <= 1) return name
  const chars =
    typeof Intl.Segmenter === "function"
      ? [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(name)].map((g) => g.segment)
      : Array.from(name)
  const short = chars.length > MAX_NAME_IN_TITLE ? `${chars.slice(0, MAX_NAME_IN_TITLE - 1).join("").trimEnd()}…` : name
  return t("share.dialog.autoTitle", { name: short, count })
}

function errorText(e: unknown): string {
  return e instanceof ApiError ? e.message : String(e)
}

type Done = (result: { id: string; info?: ShareCodeInfo }) => void

/** Create a share from `items`, or add them to an existing one (017 US1, FR-001, FR-002). */
export function ShareDialog({ items, onClose }: { items: ShareItemRef[]; onClose: () => void }) {
  const { t } = useTranslation()
  const settings = useShareSettings()
  const embedAvailable = Boolean(settings.data?.share_public_url)
  const [tab, setTab] = useState<"new" | "existing">("new")
  const [result, setResult] = useState<{ id: string; info?: ShareCodeInfo } | null>(null)

  return (
    <Modal onClose={onClose} width={640} textAlign="left">
      <h2 className="ih">{t("share.dialog.heading", { count: items.length })}</h2>
      {result ? (
        <>
          <ShareResult shareId={result.id} info={result.info} />
          <div style={{ textAlign: "right" }}>
            <button type="button" className="stdbtn" onClick={onClose}>
              {t("share.action.close")}
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="share-dialog-tabs">
            {(["new", "existing"] as const).map((which) => (
              <button
                key={which}
                type="button"
                className="stdbtn share-dialog-tab"
                aria-pressed={tab === which}
                onClick={() => setTab(which)}
              >
                {which === "new" ? t("share.dialog.newShare") : t("share.dialog.addToExisting")}
              </button>
            ))}
          </div>
          {tab === "new" &&
            (settings.data ? (
              <NewShareTab items={items} defaults={settings.data} embedAvailable={embedAvailable} onDone={setResult} />
            ) : (
              <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>
            ))}
          {tab === "existing" && <ExistingShareTab items={items} embedAvailable={embedAvailable} onDone={setResult} />}
        </>
      )}
    </Modal>
  )
}

function NewShareTab({
  items,
  defaults,
  embedAvailable,
  onDone,
}: {
  items: ShareItemRef[]
  defaults: ShareSettings
  embedAvailable: boolean
  onDone: Done
}) {
  const { t } = useTranslation()
  const create = useCreateShare()
  const [form, setForm] = useState<ShareFormState>(() => defaultsFrom(defaults))
  const [shortCode, setShortCode] = useState(false)
  const first = items[0]
  const firstArchive = useArchiveMetadata(first?.kind === "archive" ? first.id : null)
  const firstTank = useTankoubon(first?.kind === "tankoubon" ? first.id : "")
  const firstName = first?.kind === "tankoubon" ? firstTank.data?.name : firstArchive.data?.title
  const autoTitle = firstName ? defaultShareTitle(firstName, items.length, t) : ""
  // The generated title fills the field once it is known, unless the user has typed their own.
  const titleEdited = useRef(false)
  useEffect(() => {
    if (autoTitle && !titleEdited.current) setForm((f) => ({ ...f, title: autoTitle }))
  }, [autoTitle])
  // Whole days rounded up, so "within 7 days" here is the same bound the server checks.
  const shortCodeAllowed = form.expiresAt !== "" && daysUntil(form.expiresAt) <= SHORT_CODE_MAX_DAYS

  const submit = () => {
    if (!permissionsChosen(form)) return
    create.mutate(
      { items, short_code: shortCode && shortCodeAllowed, ...fieldsFromForm(form) },
      {
        onSuccess: (detail) => onDone({ id: detail.id, info: detail.code }),
        onError: (e) => toast({ heading: t("share.dialog.failed"), text: errorText(e), icon: "error" }),
      },
    )
  }

  return (
    <>
      <ShareFieldsForm
        value={form}
        onChange={(next) => {
          if (next.title !== form.title) titleEdited.current = true
          setForm(next)
        }}
        embedAvailable={embedAvailable}
      />
      <div style={{ marginTop: 8 }}>
        <CheckboxField checked={shortCode && shortCodeAllowed} disabled={!shortCodeAllowed} onCheckedChange={setShortCode}>
          {t("share.dialog.shortCode")}
        </CheckboxField>
        {!shortCodeAllowed && <div className="share-result-note">{t("share.dialog.shortCodeHint")}</div>}
      </div>
      <p className="share-disclaimer">{t("share.disclaimer.share")}</p>
      <div style={{ textAlign: "right", marginTop: 8 }}>
        <button type="button" className="stdbtn" disabled={create.isPending || !permissionsChosen(form)} onClick={submit}>
          {t("share.dialog.create")}
        </button>
      </div>
    </>
  )
}

function ExistingShareTab({
  items,
  embedAvailable,
  onDone,
}: {
  items: ShareItemRef[]
  embedAvailable: boolean
  onDone: Done
}) {
  const { t } = useTranslation()
  const active = useShares("active")
  const [existingId, setExistingId] = useState("")
  return (
    <>
      {active.data && active.data.length === 0 && <p>{t("share.dialog.noActiveShares")}</p>}
      <div className="share-existing-list">
        <RadioGroup value={existingId} onValueChange={setExistingId}>
          {(active.data ?? []).map((s) => (
            <div key={s.id}>
              <RadioItem value={s.id}>
                {s.title || s.id} · {t("share.dialog.itemCount", { count: s.item_count })}
              </RadioItem>
            </div>
          ))}
        </RadioGroup>
      </div>
      {existingId && <ExistingShareEditor key={existingId} shareId={existingId} items={items} embedAvailable={embedAvailable} onDone={onDone} />}
    </>
  )
}

function ExistingShareEditor({
  shareId,
  ...rest
}: {
  shareId: string
  items: ShareItemRef[]
  embedAvailable: boolean
  onDone: Done
}) {
  const existing = useShare(shareId)
  if (!existing.data) return <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>
  return <ExistingShareForm share={existing.data} {...rest} />
}

function ExistingShareForm({
  share,
  items,
  embedAvailable,
  onDone,
}: {
  share: ShareDetail
  items: ShareItemRef[]
  embedAvailable: boolean
  onDone: Done
}) {
  const { t } = useTranslation()
  const add = useAddShareItems()
  const [alsoUpdate, setAlsoUpdate] = useState(false)
  const [form, setForm] = useState<ShareFormState>(() => formFromShare(share))

  const submit = () => {
    if (alsoUpdate && !permissionsChosen(form)) return
    add.mutate(
      { id: share.id, items, update: alsoUpdate ? fieldsFromForm(form) : undefined },
      {
        onSuccess: (detail) => onDone({ id: detail.id }),
        onError: (e) => toast({ heading: t("share.dialog.failed"), text: errorText(e), icon: "error" }),
      },
    )
  }

  return (
    <>
      <div style={{ marginTop: 8 }}>
        <CheckboxField checked={alsoUpdate} onCheckedChange={setAlsoUpdate}>
          {t("share.dialog.alsoUpdate")}
        </CheckboxField>
        {alsoUpdate && <ShareFieldsForm value={form} onChange={setForm} embedAvailable={embedAvailable} />}
      </div>
      <p className="share-disclaimer">{t("share.disclaimer.share")}</p>
      <div style={{ textAlign: "right", marginTop: 8 }}>
        <button type="button" className="stdbtn" disabled={add.isPending} onClick={submit}>
          {t("share.dialog.add")}
        </button>
      </div>
    </>
  )
}
