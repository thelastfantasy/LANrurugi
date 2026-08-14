import { useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"

import { useApiTokens, useCreateApiToken, useDeleteApiToken, useRenameApiToken } from "@/api/hooks"
import type { TokenRole } from "@/api/types"
import { CollapsibleSection, Tooltip } from "@/components/Display"
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

/** Loopback/private/link-local ranges (IPv4 + IPv6) — a geolocation lookup for any of these
 *  resolves to nothing meaningful (there's no real-world location for "this machine" or "someone
 *  on the LAN"), so `IpGeoLink` skips linking for these and just renders plain text instead of a
 *  dead-end link. Deliberately loose/best-effort (string prefix checks, not a real CIDR parser) —
 *  this is a display convenience, not a security boundary; a value that slips through and gets
 *  linked anyway just means one extra click to a lookup that says "private range," not a defect
 *  that matters. */
function isPrivateOrLocalIp(ip: string): boolean {
  if (ip === "127.0.0.1" || ip === "::1" || ip === "0.0.0.0") return true
  if (/^10\./.test(ip)) return true
  if (/^192\.168\./.test(ip)) return true
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(ip)) return true
  if (/^169\.254\./.test(ip)) return true // link-local (IPv4)
  if (/^f[cd][0-9a-f]{2}:/i.test(ip)) return true // fc00::/7, unique local (IPv6)
  if (/^fe80:/i.test(ip)) return true // link-local (IPv6)
  return false
}

/** Links a `last_used_ip` value out to an IP-geolocation lookup — skipped for a private/local
 *  address (see `isPrivateOrLocalIp`), rendered as plain text instead since there's nothing a geo
 *  lookup could tell you about one. The scheme/host are always the fixed `https://ipinfo.io/`
 *  literal below — `ip` is only ever interpolated as an `encodeURIComponent`-escaped *path
 *  segment*, never used as the href's own scheme/host — because `last_used_ip` is sourced from
 *  `X-Forwarded-For` (see `AuthContext`/`client_ip`'s own docs: spoofable, display-only, never a
 *  security control). Building `href={ip}` directly would let whoever controls that header hand an
 *  admin viewing this table a `javascript:`-scheme link to click inside their own authenticated
 *  session — this construction makes that impossible regardless of what the header contains.
 *  `rel="noopener noreferrer"` — `noreferrer` so this admin's own presence on this page/instance
 *  isn't leaked to the third-party lookup site via the `Referer` header, `noopener` so the opened
 *  tab can't reach back into this one via `window.opener`. */
function IpGeoLink({ ip }: { ip: string }) {
  if (isPrivateOrLocalIp(ip)) return <>{ip}</>
  return (
    <a
      href={`https://ipinfo.io/${encodeURIComponent(ip)}`}
      target="_blank"
      rel="noopener noreferrer"
    >
      {ip}
    </a>
  )
}

/** Date and time on their own lines — `toLocaleString()`'s combined "2026/8/14 10:05:34" was
 *  overflowing its grid column at this table's own font size, since `whiteSpace: "nowrap"` (needed
 *  so the two halves don't wrap at an arbitrary space) left nothing shorter to wrap to. Splitting
 *  the value itself into two deliberate lines gives the same "never breaks mid-word" guarantee
 *  without needing the column to be wide enough for the full combined string in one line. */
function DateTimeStack({ epochSeconds }: { epochSeconds: number }) {
  const date = new Date(epochSeconds * 1000)
  return (
    <>
      {date.toLocaleDateString()}
      <br />
      {date.toLocaleTimeString()}
    </>
  )
}

/** The full UUID (36 chars) took too much width for a column that's mostly useful for "confirm
 *  which row this is" rather than actually being read character-by-character — visually clipped to
 *  its first 7 characters (matching `git`'s own default short-hash length, a familiar convention
 *  for "enough to usually disambiguate, not the whole thing") via `overflow: hidden` on a
 *  fixed-width container, rather than actually truncating the *text*: the full id is still the
 *  real DOM text content underneath, so a click selecting "everything in this element" (the handler
 *  below) selects the complete value ready to copy, not just whatever few characters happen to be
 *  visible. The native `title` hover tooltip also still shows the full value without needing to
 *  click first. */
function ShortId({ id }: { id: string }) {
  function selectAll(e: React.MouseEvent<HTMLSpanElement>) {
    const selection = window.getSelection()
    if (!selection) return
    const range = document.createRange()
    range.selectNodeContents(e.currentTarget)
    selection.removeAllRanges()
    selection.addRange(range)
  }
  return (
    <span
      title={id}
      onClick={selectAll}
      style={{ display: "inline-block", width: "7ch", overflow: "hidden", cursor: "text" }}
    >
      {id}
    </span>
  )
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
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("Name this token (e.g. \"Mihon phone\"):")}</p>
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
            {t("Permission")}
            <Tooltip
              wrapperStyle={{ alignItems: "center", top: 2 }}
              label={
                <div style={{ textAlign: "left" }}>
                  <div>
                    <strong>{t("Admin")}</strong>: {t("Full access except token management, account security, and database deletion.")}
                  </div>
                  <div style={{ marginTop: 4 }}>
                    <strong>{t("Guest")}</strong>: {t("Read-only.")}
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
            <option value="admin">{t("Admin (some permissions restricted)")}</option>
            <option value="guest">{t("Guest (read-only)")}</option>
          </select>
        </label>
        <label style={{ flex: 1 }}>
          <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("Expires")}</span>
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
        <input type="button" className="stdbtn" value={t("Cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("OK") ?? "OK"} onClick={submit} />
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
        <p style={{ fontWeight: "bold" }}>{t("Copy this token now — it won't be shown again.")}</p>
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
            title={t("Copy to clipboard") ?? undefined}
            onClick={() => copyToClipboard(rawToken, t)}
            size={25}
          />
        </div>
      </>,
    )
  }

  async function handleRename(id: string, currentName: string) {
    const name = await promptDialog(t("Rename this token:") ?? "", currentName)
    if (!name || !name.trim() || name.trim() === currentName) return
    try {
      await renameToken.mutateAsync({ id, name: name.trim() })
      toast({ text: t("Token renamed.") ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("Error renaming token") ?? undefined, icon: "error" })
    }
  }

  async function handleDelete(id: string, name: string) {
    if (!(await confirmDialog(t("Revoke the token \"{{name}}\"? Any client using it will stop working immediately.", { name }) ?? "", true))) return
    try {
      await deleteToken.mutateAsync(id)
      toast({ text: t("Token revoked.") ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("Error revoking token") ?? undefined, icon: "error" })
    }
  }

  return (
    <CollapsibleSection icon="fa-key" title={t("API Tokens")}>
      <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center", padding: "0 12px" }}>
        {t("First-party tokens for third-party clients (Tachiyomi/Mihon, OPDS readers, scripts) — sent as an Authorization: Bearer header. Each token can be individually named, tracked, and revoked.")}
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
          value={t("New Token") ?? undefined}
          onClick={() => setCreateDialogOpen(true)}
        />
        <br />
        <br />
      </div>

      {tokens.isLoading && <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center" }}>{t("Loading…")}</div>}
      {tokens.data?.length === 0 && (
        <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center" }}>{t("No tokens yet.")}</div>
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
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("Name")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("ID")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("Created")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("Expires")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("Last Used")}</div>
          <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("Last Used IP")}</div>
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
                {token.expires_at ? <DateTimeStack epochSeconds={token.expires_at} /> : t("Permanent")}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left", whiteSpace: "nowrap" }}>
                {token.last_used_at ? <DateTimeStack epochSeconds={token.last_used_at} /> : t("Never")}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", textAlign: "left" }}>
                {token.last_used_ip ? <IpGeoLink ip={token.last_used_ip} /> : "—"}
              </div>
              <div style={{ padding: "10px 0", alignSelf: "center", display: "flex", gap: 6 }}>
                <IconButton
                  icon="fas fa-pencil"
                  size="medium"
                  title={t("Rename") ?? undefined}
                  onClick={() => void handleRename(token.id, token.name)}
                />
                <IconButton
                  icon="fas fa-trash"
                  className="stdbtn stdbtn-danger"
                  size="medium"
                  title={t("Revoke") ?? undefined}
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
