import { type CSSProperties, Fragment, useState } from "react"
import { useTranslation } from "react-i18next"

import { useApprovePending, useDismissPending, usePendingApprovals, useSubscriptionHistory } from "@/api/hooks"
import type { PendingApproval, Subscription } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { Checkbox } from "@/components/common-ui/Form"
import { TagTable } from "@/components/Display/TagTable"
import { useLanguageOrder } from "@/i18n/useLanguageOrder"
import { toast } from "@/toast"

import { ICON_BUTTON_STYLE } from "../Upload/shared"
import { CandidateTitleLink } from "./CandidateTitleLink"
import { preferredTitle } from "./preferredTitle"
import { SubscriptionGroupHead, useCollapsedGroups } from "./SubscriptionGroupHead"
import { PostedCell, RatingCell, SUBSCRIPTION_LIST_COLUMNS, UploaderCell } from "./SubscriptionPreview"
import { groupBySubscription } from "./subscriptionVerdicts"

/** Sized to its label: the theme's fixed `.stdbtn` width made two of them take half the row. */
const ROW_BUTTON_STYLE: React.CSSProperties = { width: "auto", minWidth: "4em", padding: "0 10px" }

export function PendingApprovals({
  subscriptions,
  subscriptionId = "",
  heading = true,
}: {
  subscriptions: Subscription[]
  /** Only this subscription's works; `""` for all of them. */
  subscriptionId?: string
  /** Off where a tab already names the list. */
  heading?: boolean
}) {
  const { t } = useTranslation()
  const pending = usePendingApprovals()
  const approve = useApprovePending()
  const dismiss = useDismissPending()
  const titleOrder = useLanguageOrder()
  /** Selection exists so a batch of matches can be answered in one go (FR-007b). Nothing is selected
   * by default — pre-selecting would make "approve selected" a one-click way to spend credit on
   * works the user has not looked at. */
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const groupsState = useCollapsedGroups()

  const history = useSubscriptionHistory()
  const items = (pending.data ?? []).filter((p) => !subscriptionId || p.subscription_id === subscriptionId)
  if (items.length === 0) return null

  const nameOf = (id: string) => subscriptions.find((s) => s.id === id)?.name ?? id
  const busy = approve.isPending || dismiss.isPending
  // Only rows still on the list count: an answered row leaves it at once (see `useApprovePending`),
  // and a selection kept past that would offer to act on works that are already gone.
  const chosenIds = items.filter((p) => selected.has(p.id)).map((p) => p.id)
  // Always grouped, even with one subscription: the heading is where its select-all and its name
  // live, which is what lets the rows drop a subscription column of their own.
  const groups = groupBySubscription(items, (p) => p.subscription_id)
  // A pending work carries only its title; what the listing said about it (posted, uploader, rating,
  // tags) is in the check that matched it, read from the same history the other tabs show.
  const bySource = new Map((history.data ?? []).map((e) => [`${e.subscription_id}\u0000${e.candidate.source_url}`, e.candidate]))
  const candidateOf = (p: PendingApproval) => bySource.get(`${p.subscription_id}\u0000${p.source_url}`)
  /** A selection may span subscriptions; a group's own checkbox adds or removes just that group. */
  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  function toggleGroup(ids: string[], on: boolean) {
    setSelected((prev) => {
      const next = new Set(prev)
      for (const id of ids) {
        if (on) next.add(id)
        else next.delete(id)
      }
      return next
    })
  }

  async function runApprove(ids: string[]) {
    setSelected(new Set())
    const result = await approve.mutateAsync(ids)
    if (result.failures.length > 0) {
      // Named rather than summarised: a partial batch is reported as-is, so the user knows which
      // works did not make it into the queue instead of assuming all of them did.
      toast({
        text:
          t("subscriptions.approvedPartially", {
            approved: result.approved,
            failed: result.failures.length,
          }) ?? undefined,
        icon: "warning",
      })
    } else {
      toast({ text: t("subscriptions.queued", { count: result.approved }) ?? undefined, icon: "success" })
    }
  }

  return (
    // Left-aligned explicitly: the settings section around it centres its text, which a table
    // ignored but this grid inherits.
    <div style={{ marginBottom: 12, textAlign: "left" }}>
      {heading && (
        <h3 className="ih" style={{ fontSize: "1.0em", margin: "0 0 6px" }}>
          {t("subscriptions.awaitingApproval", { count: items.length })}
        </h3>
      )}
      {/* Batch actions sit above the list, where the selection they act on starts. Always rendered
          (disabled with nothing selected) so selecting a row does not push the list down. */}
      <div className="pending-batch">
        <input
          type="button"
          className="stdbtn"
          style={ROW_BUTTON_STYLE}
          disabled={busy || chosenIds.length === 0}
          value={t("subscriptions.approveSelected", { count: chosenIds.length }) ?? undefined}
          onClick={() => void runApprove(chosenIds)}
        />
        <input
          type="button"
          className="stdbtn"
          style={ROW_BUTTON_STYLE}
          disabled={busy || chosenIds.length === 0}
          value={t("subscriptions.dismissSelected", { count: chosenIds.length }) ?? undefined}
          onClick={() => {
            setSelected(new Set())
            void dismiss.mutateAsync(chosenIds)
          }}
        />
      </div>
      {/* The match history's own grid, columns and row bands, so this list reads like the other
          tabs beside it: a pending work is a match waiting on one more answer. */}
      <div
        className="preview-grid"
        style={{ "--preview-grid-columns": SUBSCRIPTION_LIST_COLUMNS } as CSSProperties}
      >
        <div className="preview-row preview-head">
          <span className="preview-index">#</span>
          <span className="preview-title">{t("subscriptions.colWork")}</span>
          <span className="preview-posted">{t("subscriptions.field.posted_at")}</span>
          <span className="preview-uploader">{t("subscriptions.colUploader")}</span>
          <span className="preview-rating">{t("subscriptions.field.rating")}</span>
          <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
        </div>
        {groups.map((group) => {
          const ids = group.items.map((p) => p.id)
          const chosen = ids.filter((id) => selected.has(id)).length
          return (
            <Fragment key={group.subscriptionId}>
              <SubscriptionGroupHead
                name={nameOf(group.subscriptionId)}
                count={group.items.length}
                collapsed={groupsState.isCollapsed(group.subscriptionId)}
                onToggle={() => groupsState.toggle(group.subscriptionId)}
                control={
                  <Checkbox
                    checked={chosen > 0 && chosen === ids.length}
                    indeterminate={chosen > 0 && chosen < ids.length}
                    // From "some", a click selects the whole group rather than clearing it.
                    onCheckedChange={() => toggleGroup(ids, chosen < ids.length)}
                    aria-label={nameOf(group.subscriptionId)}
                  />
                }
              />

              {!groupsState.isCollapsed(group.subscriptionId) &&
                group.items.map((p: PendingApproval, i) => {
                const seen = candidateOf(p)
                return (
                <div key={p.id} className="preview-row preview-row-match pending-row">
                  <span className="preview-index">{i + 1}</span>
                  <span className="preview-title pending-title">
                    {/* The slot the other tabs give a queue-state icon, so titles line up across tabs. */}
                    <span className="history-title-state pending-check">
                      <Checkbox
                        checked={selected.has(p.id)}
                        onCheckedChange={() => toggle(p.id)}
                        aria-label={preferredTitle(p.title, titleOrder) ?? p.source_url}
                      />
                    </span>
                    {/* Its own box, so a long title wraps beside the checkbox instead of under it. */}
                    <span className="pending-title-text">
                      <CandidateTitleLink
                        record={{
                          source_url: p.source_url,
                          title: p.title,
                          tags: [],
                          verdict: { verdict: "awaiting_approval" },
                        }}
                        order={titleOrder}
                      />
                      {(seen?.tags?.length ?? 0) > 0 && (
                        <Tooltip
                          label={<TagTable tags={(seen?.tags ?? []).join(",")} links={false} />}
                          wrapperStyle={{ display: "block", minWidth: 0 }}
                          zIndex={9700}
                        >
                          <span className="tags">{(seen?.tags ?? []).join(", ")}</span>
                        </Tooltip>
                      )}
                    </span>
                  </span>
                  <PostedCell postedAt={seen?.posted_at} />
                  <UploaderCell name={seen?.uploader ?? ""} />
                  <RatingCell rating={seen?.rating} />
                  <span className="preview-verdict">
                    {t("subscriptions.verdictAwaitingApproval")}
                    <span className="history-when">{new Date(p.created_at * 1000).toLocaleString()}</span>
                    <span className="preview-row-actions">
                      {/* Icons, like the other tabs' row actions; the tooltip carries the word. */}
                      <Tooltip label={t("subscriptions.approve") ?? ""} zIndex={9700}>
                        <button
                          type="button"
                          className="stdbtn"
                          style={ICON_BUTTON_STYLE}
                          aria-label={t("subscriptions.approve") ?? "Download"}
                          disabled={busy}
                          onClick={() => void runApprove([p.id])}
                        >
                          <i className="fa fa-download" aria-hidden="true"></i>
                        </button>
                      </Tooltip>
                      <Tooltip label={t("subscriptions.dismiss") ?? ""} zIndex={9700}>
                        <button
                          type="button"
                          className="stdbtn"
                          style={ICON_BUTTON_STYLE}
                          aria-label={t("subscriptions.dismiss") ?? "Skip"}
                          disabled={busy}
                          onClick={() => void dismiss.mutateAsync([p.id])}
                        >
                          <i className="fa fa-ban" aria-hidden="true"></i>
                        </button>
                      </Tooltip>
                    </span>
                  </span>
                </div>
                )
              })}
            </Fragment>
          )
        })}
      </div>
    </div>
  )
}
