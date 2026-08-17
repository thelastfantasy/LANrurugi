import { useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"

import { useApiTokens, useCreateApiToken, useDeleteApiToken, useRenameApiToken } from "@/api/hooks"
import type { TokenRole } from "@/api/types"
import { CollapsibleSection, DateTimeStack, IpGeoLink, ShortId, Tooltip } from "@/components/Display"
import { IconButton } from "@/components/Display/IconButton"
import { confirmDialog, infoDialog, promptDialog } from "@/dialog"
import { FONT_SIZE_SM, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"
import { toast } from "@/toast"

/** `1周`/`1个月`/`3个月`/`1年`/`永久` — the exact five options requested for the "New Token" dialog's
 *  expiry picker. Months/years use a fixed 30/365-day approximation (not calendar-aware) — same
 *  simplification every other "duration in seconds" picker in this app already makes (e.g.
 *  `SecuritySection.tsx`'s own token-lifetime hours/days fields), consistent rather than a
 *  precision this single field doesn't need. `"permanent"` maps to `undefined` — omitted entirely
 *  from the create request, matching the backend's own `expires_in_secs: Option<i64>` (`None` =
 *  no expiry) rather than a sentinel value the server would have to special-case. */
const EXPIRY_OPTIONS: { value: string; labelKey: string; secs: number | undefined }[] = [
  { value: "1w", labelKey: "1 week", secs: 7 * 86400 },
  { value: "1m", labelKey: "1 month", secs: 30 * 86400 },
  { value: "3m", labelKey: "3 months", secs: 90 * 86400 },
  { value: "1y", labelKey: "1 year", secs: 365 * 86400 },
  { value: "permanent", labelKey: "Permanent", secs: undefined },
]

/** `lru_3b3c****...****77df`-style masking (the shape most API-key UIs — GitHub, Stripe,
 *  DeepSeek's own console — already converge on): first 8 and last 4 characters visible, the
 *  middle collapsed to a fixed-width run of `*`s — deliberately a *fixed* count, not one scaled to
 *  the real token's own length, so the masked string's own length never leaks how long the
 *  underlying secret is. `MASK_STAR_COUNT` is sized to roughly fill the reveal dialog's own input
 *  box width at its current `flex: 1` size (~40 monospace characters at that box's width) rather
 *  than picked arbitrarily — reads oddly short otherwise, left mostly empty in a wide box.
 *  Display-only — the *raw* token this operates on is the one this dialog is itself the one-time
 *  reveal of, never a stored/refetched value (this app never receives a token's raw value again
 *  after creation). */
const MASK_STAR_COUNT = 40

function maskToken(token: string): string {
  if (token.length <= 14) return token
  return `${token.slice(0, 8)}${"*".repeat(MASK_STAR_COUNT)}${token.slice(-4)}`
}

function copyToClipboard(value: string, t: (key: string) => string | null) {
  navigator.clipboard
    .writeText(value)
    .then(() => toast({ heading: t("Copied to clipboard!") ?? undefined, icon: "info", hideAfter: 3000 }))
    .catch(() => toast({ heading: t("Failed to copy.") ?? undefined, icon: "error" }))
}

/** Marks each token's role right in the Name column — no separate Role column, an icon +
 *  hover tooltip (native `title`) instead so it doesn't compete for horizontal space in an
 *  already-tight table. `fa-user-shield` (full access) vs. `fa-eye` (read-only) — distinct enough
 *  glyphs to tell apart at a glance without needing to actually hover for the tooltip text. */
function RoleMarker({ role, t }: { role: TokenRole; t: (key: string) => string | null }) {
  const icon = role === "guest" ? "fa-eye" : "fa-user-shield"
  const label = role === "guest" ? t("Guest") : t("Admin")
  return (
    <i
      className={`fas ${icon}`}
      title={label ?? undefined}
      style={{ marginRight: 6, opacity: 0.7 }}
      aria-hidden="true"
    ></i>
  )
}

/** `name` grows to fill remaining space; `id` gets a fixed narrow width (see `ShortId`'s own docs
 *  for why only a prefix is shown there) rather than sizing to the full UUID's own content width;
 *  every other column sizes to its own content, so e.g. the revoke button's column stays its
 *  natural width instead of stretching to match whichever column happens to be widest — the
 *  previous nested-native-`<table>` layout gave every column equal/auto width with no way to size
 *  the button column independently, which is why the Revoke button rendered far wider than its
 *  own label needed. */
const TOKEN_GRID_COLUMNS = "1fr 7ch auto auto auto auto auto"

/** The "New Token" creation form — name + role (`admin`/`guest`) + expiry, the three fields
 *  confirmed for this dialog. A local component (not routed through `dialog.tsx`'s shared
 *  `DialogRequest` union) since, unlike `newCategoryDialog`/`renameArchiveDialog`, this form has
 *  exactly one call site — growing the shared dialog module for a form nothing else uses would
 *  just be indirection for its own sake. */
function CreateTokenForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (values: { name: string; role: TokenRole; expiresInSecs: number | undefined }) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [name, setName] = useState("")
  // Defaults to the least-privileged, shortest-lived combination (`Guest` + 1 week) — a new token
  // starts out as narrow as possible; an operator who actually needs full/permanent access can
  // still pick it, but has to do so deliberately rather than it being what happens if they just
  // click through the dialog without touching either field.
  const [role, setRole] = useState<TokenRole>("guest")
  const [expiryValue, setExpiryValue] = useState("1w")
  const nameRef = useRef<HTMLInputElement>(null)

  function submit() {
    const trimmed = name.trim()
    if (!trimmed) return
    const expiresInSecs = EXPIRY_OPTIONS.find((o) => o.value === expiryValue)?.secs
    onSubmit({ name: trimmed, role, expiresInSecs })
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("settings.nameThisTokenEG")}</p>
      <input
        ref={nameRef}
        type="text"
        className="stdinput"
        style={{ width: "100%", height: 25, boxSizing: "border-box", marginBottom: 12 }}
        value={name}
        autoFocus
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit()
        }}
      />
      <div style={{ display: "flex", gap: 12, marginBottom: 12, textAlign: "left" }}>
        <label style={{ flex: 1 }}>
          <span style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 13, marginBottom: 4 }}>
            {t("settings.permission")}
            <Tooltip
              wrapperStyle={{ alignItems: "center", top: 2 }}
              label={
                <div style={{ textAlign: "left" }}>
                  <div>
                    <strong>{t("settings.admin")}</strong>: {t("settings.fullAccessExceptTokenManagement")}
                  </div>
                  <div style={{ marginTop: 4 }}>
                    <strong>{t("settings.guest")}</strong>: {t("settings.readonly")}
                  </div>
                </div>
              }
            >
              <i className="fas fa-question-circle" style={{ fontSize: 14, cursor: "help" }} aria-hidden="true"></i>
            </Tooltip>
          </span>
          <select
            className="stdinput"
            style={{ width: "100%", height: 25, boxSizing: "border-box" }}
            value={role}
            onChange={(e) => setRole(e.target.value as TokenRole)}
          >
            <option value="admin">{t("settings.adminSomePermissionsRestricted")}</option>
            <option value="guest">{t("settings.guestReadonly")}</option>
          </select>
        </label>
        <label style={{ flex: 1 }}>
          <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("settings.expires")}</span>
          <select
            className="stdinput"
            style={{ width: "100%", height: 25, boxSizing: "border-box" }}
            value={expiryValue}
            onChange={(e) => setExpiryValue(e.target.value)}
          >
            {EXPIRY_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {t(o.labelKey)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
        <input type="button" className="stdbtn" value={t("common.cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("common.ok") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

/** First-party API token management (issue #54) — replaces legacy's single fixed `apikey` field.
 *  Lives in its own `CollapsibleSection` (not folded into `SecuritySection`) since it's a full
 *  list-management UI (create/list/revoke), not a handful of settings-form rows. */
export function ApiTokensSection() {
  const { t } = useTranslation()
  const tokens = useApiTokens()
  const createToken = useCreateApiToken()
  const deleteToken = useDeleteApiToken()
  const renameToken = useRenameApiToken()
  const [createDialogOpen, setCreateDialogOpen] = useState(false)

  async function handleCreateSubmit(values: { name: string; role: TokenRole; expiresInSecs: number | undefined }) {
    setCreateDialogOpen(false)
    const response = await createToken.mutateAsync(values)
    // The one and only time the raw value is ever visible — shown once, in a dedicated
    // acknowledgement dialog (not a toast, which auto-dismisses and could easily be missed before
    // the value is copied), then never retrievable again. Masked on screen (same convention
    // GitHub/Stripe/DeepSeek's own key-management UIs use) with a copy button doing the real work
    // — a raw secret sitting fully visible in plaintext on screen is needless shoulder-surfing
    // exposure when nobody actually needs to *read* it character-by-character, only paste it.
    const rawToken = response.data.token
    await infoDialog(
      <>
        <p style={{ fontWeight: "bold" }}>{t("settings.copyThisTokenNow")}</p>
        <div style={{ display: "flex", gap: 8, alignItems: "center", justifyContent: "center" }}>
          <input
            type="text"
            className="stdinput"
            style={{ flex: 1, minWidth: 0, height: 25, boxSizing: "border-box", textAlign: "center", fontFamily: "monospace" }}
            value={maskToken(rawToken)}
            readOnly
          />
          <IconButton
            icon="fas fa-copy"
            title={t("settings.copyToClipboard") ?? undefined}
            onClick={() => copyToClipboard(rawToken, t)}
            size={25}
          />
        </div>
      </>,
    )
  }

  async function handleRename(id: string, currentName: string) {
    const name = await promptDialog(t("settings.renameThisToken") ?? "", currentName)
    if (!name || !name.trim() || name.trim() === currentName) return
    try {
      await renameToken.mutateAsync({ id, name: name.trim() })
      toast({ text: t("settings.tokenRenamed") ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("settings.errorRenamingToken") ?? undefined, icon: "error" })
    }
  }

  async function handleDelete(id: string, name: string) {
    if (!(await confirmDialog(t("settings.revokeTheTokenNameAny", { name }) ?? "", true))) return
    try {
      await deleteToken.mutateAsync(id)
      toast({ text: t("settings.tokenRevoked") ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("settings.errorRevokingToken") ?? undefined, icon: "error" })
    }
  }

  return (
    <CollapsibleSection id="api-tokens" icon="fa-key" title={t("settings.apiTokens")}>
      <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center", padding: "0 12px" }}>
        {t("settings.firstpartyTokensForThirdpartyClients")}
        <br />
        <br />
        {/* `.stdbtn`'s own legacy CSS carries a flat `min-width: 150px` (built for its usual
            longer labels elsewhere in the app) — overridden here so this button sizes to its own
            text + padding instead of stretching wider than "New Token" needs. */}
        <input
          id="create-api-token"
          className="stdbtn"
          type="button"
          style={{ minWidth: 0, width: "auto", padding: "0 12px" }}
          value={t("settings.newToken") ?? undefined}
          onClick={() => setCreateDialogOpen(true)}
        />
        <br />
        <br />
      </div>

      {tokens.isLoading && <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center" }}>{t("common.loading")}</div>}
      {tokens.data?.length === 0 && (
        <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center" }}>{t("settings.noTokensYet")}</div>
      )}

      {tokens.data && tokens.data.length > 0 && (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: TOKEN_GRID_COLUMNS,
            columnGap: 16,
            fontSize: FONT_SIZE_SM,
            padding: "0 12px 12px",
          }}
        >
          {/* Header row: lighter (opacity, not a new theme color — see this file's own precedent
              at `IconButtonWithTooltip`'s description text) and not bold, so it reads as a
              secondary label rather than competing with the actual row data for attention. No
              border — rows are separated by generous padding alone (DeepSeek's own key-management
              table, cited as the visual reference this follows), not a hairline grid. */}
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("jobs.name")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("settings.id")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("settings.created")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("settings.expires")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("settings.lastUsed")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("settings.lastUsedIp")}</div>
          <div></div>

          {tokens.data.map((token) => (
            <div key={token.id} style={{ display: "contents" }}>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left" }}>
                <RoleMarker role={token.role} t={t} />
                {token.name}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left", whiteSpace: "nowrap", fontFamily: "monospace", opacity: 0.8 }}>
                <ShortId id={token.id} />
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left", whiteSpace: "nowrap" }}>
                <DateTimeStack epochSeconds={token.created_at} />
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left", whiteSpace: "nowrap" }}>
                {token.expires_at ? <DateTimeStack epochSeconds={token.expires_at} /> : t("settings.permanent")}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left", whiteSpace: "nowrap" }}>
                {token.last_used_at ? <DateTimeStack epochSeconds={token.last_used_at} /> : t("settings.never")}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left" }}>
                {token.last_used_ip ? <IpGeoLink ip={token.last_used_ip} /> : "—"}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", display: "flex", gap: 6 }}>
                <IconButton
                  icon="fas fa-pencil"
                  size="medium"
                  title={t("edit.rename") ?? undefined}
                  onClick={() => void handleRename(token.id, token.name)}
                />
                <IconButton
                  icon="fas fa-trash"
                  className="stdbtn stdbtn-danger"
                  size="medium"
                  title={t("settings.revoke") ?? undefined}
                  onClick={() => void handleDelete(token.id, token.name)}
                />
              </div>
            </div>
          ))}
        </div>
      )}

      {createDialogOpen &&
        createPortal(
          <>
            <div
              style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }}
              onClick={() => setCreateDialogOpen(false)}
            />
            <div
              role="dialog"
              aria-modal="true"
              className="swal2-popup"
              style={{
                position: "fixed",
                top: "50%",
                left: "50%",
                transform: "translate(-50%, -50%)",
                zIndex: Z_OVERLAY_CONTENT,
                display: "block",
                width: 360,
                padding: 20,
                textAlign: "center",
                borderRadius: ".2em",
                boxShadow: "0 2px 10px rgba(0,0,0,.4)",
              }}
            >
              <CreateTokenForm
                onSubmit={(values) => void handleCreateSubmit(values)}
                onCancel={() => setCreateDialogOpen(false)}
              />
            </div>
          </>,
          document.body,
        )}
    </CollapsibleSection>
  )
}
