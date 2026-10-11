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
