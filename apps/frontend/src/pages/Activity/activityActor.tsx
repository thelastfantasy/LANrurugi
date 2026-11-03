import type { ReactNode } from "react"

import type { ApiToken } from "@/api/types"
import type { ActivityEntry } from "@/api/types"

import { actorKindColor } from "./activityColors"

export function actorChipParts(
  t: (key: string, opts?: Record<string, unknown>) => string | null,
  entry: ActivityEntry,
  token: ApiToken | undefined,
): { label: ReactNode; color: ReturnType<typeof actorKindColor>; tooltip: ReactNode | undefined } {
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
