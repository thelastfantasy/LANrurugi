import type { ReactNode } from "react"

import type { ApiToken } from "@/api/types"
import type { ActivityEntry } from "@/api/types"

import { actorKindColor } from "./activityColors"

/** Shared actor-chip label/tooltip/color derivation — pulled out of `ActivityRow.tsx` (its
 * original, still-only-other call site) so `ActivityDetailPanel.tsx` can render the exact same
 * chip for "操作者" instead of the plain text it used to fall back to (per direct feedback: the
 * detail view's operator field looked inconsistent with the row/filter chips describing the same
 * actor everywhere else on this page). `token` is the live `useApiTokens()` lookup result for this
 * entry's own actor id — passed in rather than looked up inside this function so both call sites
 * share one `useApiTokens()` query instead of issuing it twice. */
export function actorChipParts(
  t: (key: string, opts?: Record<string, unknown>) => string | null,
  entry: ActivityEntry,
  token: ApiToken | undefined,
): { label: ReactNode; color: ReturnType<typeof actorKindColor>; tooltip: ReactNode | undefined } {
  // `fa-user-shield` matches `Settings/ApiTokensSection.tsx`'s own `RoleMarker` — the same glyph
  // that component already uses for a token's "full access" admin role, reused here since a
  // Session actor is exactly that same full-access case (see `AuthMethod::Session`'s own docs:
  // "a real human who's already proven the admin password, never subject to a Guest-role
  // restriction") rather than inventing a second icon for the same concept. A token whose id no
  // longer resolves (`!token`, looked up live against `apiTokens.data`) gets the same
  // `fa-user-slash` treatment `ActivityFilterCombobox.tsx`'s own facet dropdown uses — `entry.
  // actor.display_name` is the real name snapshotted at write time (still legible after
  // revocation, per `Actor::display_name`'s own docs), so unlike that dropdown's own bare-UUID
  // fallback the icon is the only signal here that the token is actually gone now.
  const label =
    entry.actor.kind === "token" && entry.actor.id && !token
      ? (
          <>
            <i className="fas fa-user-slash activity-chip-danger-icon" aria-hidden="true"></i>{" "}
            {entry.actor.display_name ?? entry.actor.id}
          </>
        )
      : entry.actor.kind === "token" || entry.actor.kind === "system"
        ? (entry.actor.display_name ?? entry.actor.id ?? entry.actor.kind)
        : entry.actor.kind === "session"
          ? (
              <>
                <i className="fas fa-user-shield" aria-hidden="true"></i> {t("activity.sessionUser")}
              </>
            )
          : t("activity.anonymousUser")

  const color = actorKindColor(entry.actor.kind, entry.actor.id ?? undefined)
  const tooltip =
    entry.actor.kind === "token" && entry.actor.id ? (
      <>
        <div style={{ fontFamily: "monospace" }}>{entry.actor.id}</div>
        {token && <div>{t(token.role === "guest" ? "Guest" : "Admin")}</div>}
        {!token && <div>{t("activity.revoked")}</div>}
      </>
    ) : undefined

  return { label, color, tooltip }
}
