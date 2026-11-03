import DOMPurify from "dompurify"
import { marked } from "marked"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { useReleaseNotes } from "@/api/hooks"
import type { ChangelogRelease } from "@/api/types"
import { Button } from "@/components/common-ui/Form"
import { useSession } from "@/session/SessionProvider"
import { FONT_SIZE_SM } from "@/theme"

/**
 * "New Version Release Notes" — legacy's `#updateOverlay` (`templates/index.html.tt2` +
 * `index.js::fetchChangelog`), extended from a single undifferentiated markdown blob into one
 * section per release.
 *
 * What it shows is the window between the build *this browser* last saw and the one running now,
 * newest first — so a user who skipped three releases gets three sections rather than the newest
 * one only. Legacy's own trigger is preserved (`localStorage` bookkeeping against the running
 * version); the range itself comes from the backend's `?since=` on `/api/version`.
 *
 * Renders nothing unless a signed-in admin actually just upgraded.
 *
 * It lives in `Layout`, which a guest-mode deployment also renders for an anonymous visitor on the
 * library, so the admin-only condition is explicit here rather than inherited from the route:
 * `isAuthenticated` gates the query itself, so a guest does not even ask for a changelog, and the
 * overlay stays unmounted. (The content is public either way — `/api/version` is a public route,
 * since the pre-login update banner needs it — but an upgrade notice addressed to the instance's
 * administrator is not a guest's business.)
 */
export function ReleaseNotesOverlay() {
  const { t } = useTranslation()
  const { isAuthenticated, firstRunWizardPending } = useSession()
  // Deferred while the first-run wizard is still owed: that one is a full page the admin has been
  // sent to, and stacking this modal on top of it means dismissing one to read the other. The
  // marker is only written once a query actually runs, so waiting costs nothing — the notes appear
  // as soon as the wizard is finished or skipped.
  const canShow = isAuthenticated && !firstRunWizardPending
  const notes = useReleaseNotes(canShow)
  const [dismissed, setDismissed] = useState(false)
  const entries = notes.data

  // `canShow` is checked here as well as passed to the hook: disabling the query already leaves
  // `data` undefined, but the overlay being drawn is what actually exposes the notes, so the
  // condition that decides it should not be an indirect consequence of a cache being cold.
  const open = canShow && !dismissed && !!entries && entries.length > 0

  useEffect(() => {
    if (!open) return
    // Legacy closes on a shade click; Esc is the other half every other overlay in this app
    // honours, and without it the modal is a keyboard trap.
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setDismissed(true)
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [open])

  if (!open) return null

  return (
    <>
      <div
        id="overlay-shade"
        style={{ display: "block", opacity: 0.6 }}
        onClick={() => setDismissed(true)}
      />
      <div
        id="updateOverlay"
        className="id1 base-overlay small-overlay"
        role="dialog"
        aria-modal="true"
        aria-label={t("New Version Release Notes") ?? undefined}
      >
        <div style={{ display: "flex", alignItems: "flex-start", gap: 8 }}>
          {/* A visible close on top of the shade click and Esc: legacy had neither, and a modal
              whose only exits are undiscoverable is a modal users learn to resent. */}
          <h2 className="ih" style={{ textAlign: "center", flex: 1, margin: 0 }}>
            {t("New Version Release Notes")}
          </h2>
          <Button
            variant="ghost-btn"
            className="release-notes-close"
            aria-label={t("releaseNotes.dismiss") ?? undefined}
            title={t("releaseNotes.dismiss") ?? undefined}
            onClick={() => setDismissed(true)}
          >
            <i className="fa fa-times" aria-hidden="true"></i>
          </Button>
        </div>
        <div id="changelog">
          {entries.map((entry, index) => (
            <ReleaseSection key={entry.tag} entry={entry} first={index === 0} />
          ))}
        </div>
      </div>
    </>
  )
}

function ReleaseSection({ entry, first }: { entry: ChangelogRelease; first: boolean }) {
  return (
    <section style={{ marginTop: first ? 8 : 24 }}>
      <hr style={{ margin: "0 0 12px" }} />
      <h3 style={{ margin: 0, textAlign: "center" }}>{entry.tag}</h3>
      <div style={{ textAlign: "center", fontSize: FONT_SIZE_SM, opacity: 0.75 }}>
        {formatReleaseDate(entry.published_at)}
        {entry.sha ? (
          <>
            {" · "}
            <code>{entry.sha.slice(0, 7)}</code>
          </>
        ) : null}
      </div>
      <div dangerouslySetInnerHTML={{ __html: renderReleaseBody(entry.body) }} />
    </section>
  )
}

/** `YYYY-MM-DD` straight out of the RFC3339 timestamp: this is the date GitHub published the
 *  release, so rendering it in the reader's own timezone would move it a day either way for no
 *  gain. `null` (a release with neither `published_at` nor `created_at`) renders as a dash rather
 *  than as `Invalid Date`. */
export function formatReleaseDate(publishedAt: string | null): string {
  if (!publishedAt) return "—"
  const match = /^(\d{4}-\d{2}-\d{2})/.exec(publishedAt)
  return match ? match[1] : "—"
}

/** GitHub-flavored markdown → sanitized HTML, the same `marked` + `DOMPurify` pairing legacy used.
 *
 *  `marked` passes raw HTML through by design, and a release body is externally-authored content
 *  even when it comes from this project's own repository, so the sanitize step is not optional. */
export function renderReleaseBody(body: string): string {
  const html = DOMPurify.sanitize(marked.parse(body, { gfm: true, breaks: true, async: false }))
  // Release notes are mostly links (compare ranges, issue references). Following one in this tab
  // would navigate away from the modal with no way back to where the reader was.
  const holder = document.createElement("div")
  holder.innerHTML = html
  for (const anchor of holder.querySelectorAll("a[href]")) {
    anchor.setAttribute("target", "_blank")
    anchor.setAttribute("rel", "noreferrer")
  }
  return holder.innerHTML
}
