import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { fetchJson } from "@/api/client"
import { Confirm } from "@/components/Display"

/** Remembers the last answer, so deleting several subscription-fetched archives in a row does not ask
 *  the same question each time. Per-viewer convenience only — wrapped because site data can be
 *  cleared or blocked, and the dialog must still work when reading it throws. */
const MEMORY_KEY = "lrr.deleteBlocksResubscribe"

function rememberedChoice(): boolean {
  try {
    const raw = localStorage.getItem(MEMORY_KEY)
    // Defaults to blocking: a manual delete usually means "I don't want this", and the cost of
    // getting that wrong (it silently returns on the next check) is worse than the cost of the other
    // way round (reconsidering it by hand).
    return raw === null ? true : raw === "1"
  } catch {
    return true
  }
}

function remember(value: boolean) {
  try {
    localStorage.setItem(MEMORY_KEY, value ? "1" : "0")
  } catch {
    // A viewer with site data blocked just gets the default next time.
  }
}

/** The subscriptions that already handled this work, which are the ones that could fetch it again. */
function useTrackingSubscriptions(source: string | undefined) {
  const [tracking, setTracking] = useState<{ id: string; name: string }[]>([])

  useEffect(() => {
    // No source means nothing could be tracking it, so there is nothing to ask and nothing to show.
    if (!source) return
    let live = true
    void fetchJson<{ tracking: { id: string; name: string }[] }>(
      `/subscriptions/tracking?source=${encodeURIComponent(source)}`,
    )
      .then((r) => {
        if (live) setTracking(r.tracking)
      })
      // A failure here must not block a delete: the checkbox is an extra, not a precondition.
      .catch(() => {
        if (live) setTracking([])
      })
    return () => {
      live = false
    }
  }, [source])

  // Read through rather than stored: with no source there is nothing to track, and deriving it here
  // avoids an effect whose only job is to clear state it just set.
  return source ? tracking : []
}

export function DeleteConfirmDialog({
  isTank,
  source,
  onConfirm,
  onCancel,
}: {
  isTank: boolean
  /** The archive's `source:` URL, when it has one. Absent for a hand-uploaded file. */
  source?: string
  /** `blockResubscribe` is only meaningful when a subscription is actually tracking this work. */
  onConfirm: (blockResubscribe: boolean) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const tracking = useTrackingSubscriptions(isTank ? undefined : source)
  const [block, setBlock] = useState(rememberedChoice)

  return (
    <Confirm
      danger
      message={
        <>
          {isTank
            ? t("library.thisWillDeleteThisTankoubon")
            : t("common.thisWillDeleteBothMetadata")}
          {/* Offered only for an archive a subscription actually brought in. For a hand-uploaded
              file the option would have no effect, which is worse than not offering it. */}
          {tracking.length > 0 && (
            <label
              style={{ display: "block", marginTop: 10, fontWeight: "normal", fontSize: "0.9em" }}
            >
              <input
                type="checkbox"
                className="fa"
                checked={block}
                onChange={(e) => {
                  setBlock(e.target.checked)
                  remember(e.target.checked)
                }}
              />{" "}
              {t("library.blockResubscribe", {
                names: tracking.map((s) => s.name).join(", "),
              })}
            </label>
          )}
        </>
      }
      confirmLabel={t("library.yesDeleteIt") ?? undefined}
      onConfirm={() => onConfirm(tracking.length > 0 ? block : false)}
      onCancel={onCancel}
    />
  )
}
