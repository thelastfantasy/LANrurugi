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

/** The order every subscription list reads in: publication time, newest first.
 *
 *  `posted_at` is the only order a reader can predict. `checked_at` is when a check ran, and one
 *  check gives every work it accepted the same value — sorting by it alone left the rows in
 *  whatever order the server's snapshot map iterated, so the list appeared to reshuffle between
 *  reads. Rows with no publication time (a source that exposes none, or one known only from the
 *  download queue) come last; the check time and then the URL make the order total, so identical
 *  data always reads the same way. */
export function comparePostedDesc(
  a: { checked_at: number; candidate: { source_url: string; posted_at?: number | null } },
  b: { checked_at: number; candidate: { source_url: string; posted_at?: number | null } },
): number {
  const posted = (row: typeof a) => row.candidate.posted_at ?? Number.NEGATIVE_INFINITY
  const byPosted = posted(b) - posted(a)
  if (byPosted !== 0) return byPosted
  const byChecked = b.checked_at - a.checked_at
  if (byChecked !== 0) return byChecked
  if (a.candidate.source_url === b.candidate.source_url) return 0
  return a.candidate.source_url < b.candidate.source_url ? -1 : 1
}

/** The columns a subscription list can be read in, i.e. the ones the reader can click. */
export type SubscriptionSortKey = "title" | "posted" | "uploader" | "rating" | "verdict"

export interface SubscriptionSort {
  key: SubscriptionSortKey
  dir: "asc" | "desc"
}

/** Publication time, newest first: the same order a list opens in (see `comparePostedDesc`). */
export const SUBSCRIPTION_DEFAULT_SORT: SubscriptionSort = { key: "posted", dir: "desc" }

/** Clicking a heading: the column already in use flips, a new one starts in the order it is read in
 *  — newest/highest first for a time or a rating, alphabetical for text. */
export function nextSubscriptionSort(
  current: SubscriptionSort,
  key: SubscriptionSortKey,
): SubscriptionSort {
  if (current.key === key) return { key, dir: current.dir === "asc" ? "desc" : "asc" }
  return { key, dir: key === "posted" || key === "rating" ? "desc" : "asc" }
}

/** One row's sortable values, spelled out so one comparator serves every list. */
export interface SortFields {
  title: string
  posted: number | null
  uploader: string
  rating: number | null
  verdict: string
}

export function compareBySort(a: SortFields, b: SortFields, sort: SubscriptionSort): number {
  const sign = sort.dir === "asc" ? 1 : -1
  switch (sort.key) {
    case "posted":
    case "rating": {
      const left = a[sort.key]
      const right = b[sort.key]
      // A row with no value has no place in the order — it is unknown, not the smallest — so it sits
      // at the bottom whichever way the column is read.
      if (left === null || right === null) return left === right ? 0 : left === null ? 1 : -1
      return (left - right) * sign
    }
    default:
      return (
        a[sort.key].localeCompare(b[sort.key], undefined, { numeric: true, sensitivity: "base" }) *
        sign
      )
  }
}

/** Items split by the subscription that produced them, so a list mixing several subscriptions reads
 *  one subscription at a time.
 *
 *  Group order is the subscription list's own order (`order`, i.e. what the filter above the list
 *  offers) rather than the order groups happen to appear in: sorting the works inside a subscription
 *  must not reorder the subscriptions themselves. A group whose subscription is not in that list —
 *  deleted since the works were recorded — keeps its place at the end. Without `order`, groups fall
 *  back to first appearance. */
export function groupBySubscription<T>(
  items: readonly T[],
  subscriptionOf: (item: T) => string | null | undefined,
  order?: readonly string[],
): { subscriptionId: string; items: T[] }[] {
  const groups = new Map<string, T[]>()
  for (const item of items) {
    const id = subscriptionOf(item) ?? ""
    const group = groups.get(id)
    if (group) group.push(item)
    else groups.set(id, [item])
  }
  if (!order) return [...groups].map(([subscriptionId, grouped]) => ({ subscriptionId, items: grouped }))

  const ordered: { subscriptionId: string; items: T[] }[] = []
  for (const id of order) {
    const grouped = groups.get(id)
    if (grouped) ordered.push({ subscriptionId: id, items: grouped })
  }
  for (const [subscriptionId, grouped] of groups) {
    if (!order.includes(subscriptionId)) ordered.push({ subscriptionId, items: grouped })
  }
  return ordered
}
