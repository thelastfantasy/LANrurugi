// Verdict wording shared by the three places a candidate's verdict is rendered (the form's own
// preview, the check-history list, and the history list).
//
// The split exists because one verdict covers two quite different waits: an age rule that becomes
// true on its own, and a rule over a field the listing never carried — which may never become true.
// Labelling the second as "published too recently" sent a reader to their clock while the actual
// blocker was a field their own condition needed (reported live, 2026-10-09).

import type { TFunction } from "i18next"

import type { PendingReason } from "@/api/types"
import { formatDuration } from "@/components/Display"

/** The waiting row's wording. `reason` is absent on records written before it existed, in which case
 *  the original generic text is still the best available answer. */
export function tooSoonText(t: TFunction, reason?: PendingReason): string {
  if (reason?.kind === "unknown_field") {
    return t("subscriptions.verdictPendingUnknownField", { field: fieldLabel(t, reason.field) })
  }
  if (reason?.kind === "age") {
    return t("subscriptions.verdictPendingAge", {
      duration: formatDuration(reason.remaining_secs * 1000),
    })
  }
  return t("subscriptions.verdictTooSoon")
}

/** A rule's field reads the way the preview's own column for it does (`uploader` → "上传者"); a field
 *  with no label of its own keeps its raw name rather than disappearing from the message. */
function fieldLabel(t: TFunction, field: string): string {
  const key = `subscriptions.field.${field}`
  const label = t(key)
  return label === key ? field : label
}

/** A work that never reached the download queue, in words: the server sends a stable reason key
 * (`insufficient_credit`, `download_failed:<detail>`) so it can be translated. */
export function reservationReasonText(t: TFunction, reason: string): string {
  if (reason === "source_unreachable") return t("reservations.sourceUnreachable")
  if (reason === "insufficient_credit") return t("reservations.insufficientCredit")
  if (reason === "no_download_plugin") return t("reservations.noDownloadPlugin")
  if (reason.startsWith("download_failed:"))
    return t("reservations.downloadFailed", { detail: reason.slice("download_failed:".length) })
  return reason
}

/** Items split by the subscription that produced them, groups in order of their first item, so a
 *  list mixing several subscriptions reads one subscription at a time. */
export function groupBySubscription<T>(
  items: readonly T[],
  subscriptionOf: (item: T) => string | null | undefined,
): { subscriptionId: string; items: T[] }[] {
  const groups = new Map<string, T[]>()
  for (const item of items) {
    const id = subscriptionOf(item) ?? ""
    const group = groups.get(id)
    if (group) group.push(item)
    else groups.set(id, [item])
  }
  return [...groups].map(([subscriptionId, grouped]) => ({ subscriptionId, items: grouped }))
}
