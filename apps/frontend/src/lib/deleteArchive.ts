import type { QueryClient } from "@tanstack/react-query"
import type { TFunction } from "i18next"

import { ApiError, fetchJson, sendJson } from "@/api/client"
import { toast } from "@/toast"

/** The archive's `source:` URL from its comma-separated tags, if it has one. */
export function sourceTagFrom(tags: string | null | undefined): string | undefined {
  for (const tag of (tags ?? "").split(",")) {
    const value = tag.trim()
    if (value.toLowerCase().startsWith("source:")) return value.slice("source:".length).trim()
  }
  return undefined
}

/**
 * Moves one archive to the recycle bin, the same way from every page that offers it: the server's
 * reason when it refuses, an undo on success, and — when `reconsiderSource` is given, i.e. the user
 * unticked "subscriptions no longer download it" — telling the subscriptions that saw that source
 * to offer it again. Resolves to whether the archive was deleted.
 */
export async function deleteArchiveToBin({
  archiveId,
  reconsiderSource,
  queryClient,
  t,
}: {
  archiveId: string
  reconsiderSource?: string
  queryClient: QueryClient
  t: TFunction
}): Promise<boolean> {
  try {
    await sendJson("DELETE", `/archives/${archiveId}`)
  } catch (err) {
    // The server refuses to delete a healthy archive that a Tankoubon still lists (it would
    // silently reshuffle that book), and says why — that message is what the user needs.
    toast({
      heading:
        err instanceof ApiError && err.status === 409
          ? t("library.deleteTankMemberRefused")
          : err instanceof Error
            ? err.message
            : String(err),
      icon: "error",
    })
    return false
  }
  // Recoverable for the retention window: offer the undo right where the mistake happened
  // (spec 019 FR-034) instead of making the user open the bin page.
  toast({
    heading: t("recycleBin.movedToBin"),
    icon: "success",
    action: {
      label: t("recycleBin.undo"),
      onClick: () => {
        void (async () => {
          try {
            await sendJson("POST", `/recycle-bin/${encodeURIComponent(archiveId)}/restore`)
            await queryClient.invalidateQueries({ queryKey: ["archives"] })
            await queryClient.invalidateQueries({ queryKey: ["recycle-bin"] })
            toast({ heading: t("recycleBin.restored"), icon: "success" })
          } catch {
            toast({ heading: t("recycleBin.restoreFailed"), icon: "error" })
          }
        })()
      },
    },
  })

  if (reconsiderSource) {
    // After the delete, and failures are swallowed: the archive is already gone, and a failure here
    // only means a subscription will not re-offer the work — not something to surface as a failed
    // delete.
    try {
      const { tracking } = await fetchJson<{ tracking: { id: string }[] }>(
        `/subscriptions/tracking?source=${encodeURIComponent(reconsiderSource)}`,
      )
      await Promise.all(
        tracking.map((s) =>
          sendJson("POST", `/subscriptions/${encodeURIComponent(s.id)}/forget`, {
            sources: [reconsiderSource],
            all: false,
          }),
        ),
      )
    } catch {
      // Nothing to do — the work simply stays recorded as seen.
    }
  }
  return true
}
