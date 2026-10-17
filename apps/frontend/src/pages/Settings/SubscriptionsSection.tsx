import { Fragment, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useApprovePending,
  useCheckSubscriptionNow,
  useCreateSubscription,
  useDeleteSubscription,
  useDismissPending,
  usePendingApprovals,
  usePreviewSubscription,
  useSetSubscriptionState,
  useSubscriptions,
  useSubscriptionSources,
} from "@/api/hooks"
import type {
  PendingApproval,
  Subscription,
  SubscriptionBody,
  SubscriptionSource,
} from "@/api/types"
import { Menu, MenuItem, MenuSeparator, Modal, Tooltip } from "@/components/common-ui/Display"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { routes } from "@/lib/routes"
import { toast } from "@/toast"

import { ICON_BUTTON_STYLE } from "../Upload/shared"
import { sourceHref } from "./preferredTitle"
import { SubscriptionCheckHistoryModal } from "./SubscriptionCheckHistoryModal"
import { PreviewSkeleton, SubscriptionFormModal } from "./SubscriptionFormModal"
import { SubscriptionHistoryModal } from "./SubscriptionHistoryModal"
import { SubscriptionPreview } from "./SubscriptionPreview"



function emptyBody(source: string, intervalSecs: number): SubscriptionBody {
  return {
    name: "",
    source,
    criteria: {},
    filters: {},
    interval_secs: intervalSecs,
    metadata_tags: [],
    enrich_metadata: true,
    // Off by default (FR-007a): a brand-new subscription's rules are usually still being tuned.
    auto_download: false,
    credit_policy: "pause",
    // Ask, like every download before subscriptions existed.
    conflict_policy: "ask",
    // Least destructive defaults: a change is surfaced, a removal is only marked. Neither touches an
    // archive already in the library.
    on_source_changed: "notify_only",
    on_source_removed: "mark_only",
  }
}

/** The form edits a `SubscriptionBody`, so an existing subscription is projected down to one —
 * dropping the server-owned fields (`id`, `state`, `last_checked_at`) that editing must not touch. */
function bodyOf(s: Subscription): SubscriptionBody {
  return {
    name: s.name,
    source: s.source,
    criteria: s.criteria,
    filters: s.filters,
    interval_secs: s.interval_secs,
    target_category: s.target_category ?? undefined,
    metadata_tags: s.metadata_tags ?? [],
    enrich_metadata: s.enrich_metadata,
    auto_download: s.auto_download,
    credit_policy: s.credit_policy,
    conflict_policy: s.conflict_policy ?? "ask",
    on_source_changed: s.on_source_changed,
    on_source_removed: s.on_source_removed,
  }
}


/** Seconds a subscription is waiting out after a settings change, or `null` once it has settled.
 *
 * Mirrors the host's own `SETTLE_SECS`. Duplicated rather than fetched because it is only used to
 * phrase a label — the host decides when a check actually runs, and a drifted copy here would at
 * worst show the notice for a moment too long. */
const SETTLE_SECS = 60

export function settlingUntil(s: Subscription): number | null {
  if (!s.settings_changed_at) return null
  const elapsed = Math.floor(Date.now() / 1000) - s.settings_changed_at
  return elapsed < SETTLE_SECS ? SETTLE_SECS - elapsed : null
}

export function formatTime(secs: number): string {
  return new Date(secs * 1000).toLocaleString()
}

export function SubscriptionsSection() {
  const { t } = useTranslation()
  const subscriptions = useSubscriptions()
  const sources = useSubscriptionSources()
/** Cross-subscription history, opened from the section heading rather than from any one row — it is
   *  about all of them at once. */
  const [historyOpen, setHistoryOpen] = useState(false)

  return (
    <CollapsibleSection
      id="subscriptions"
      icon="fa-rss"
      title={
        <span className="section-title-row">
          <span>{t("subscriptions.title") ?? "Subscriptions"}</span>
          <Tooltip label={t("subscriptions.historyTitle") ?? ""}>
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              aria-label={t("subscriptions.historyTitle") ?? "History"}
              onClick={(e) => {
                // The heading toggles the section; this button must not do that as well.
                e.stopPropagation()
                setHistoryOpen(true)
              }}
            >
              <i className="fa fa-th" aria-hidden="true"></i>
            </button>
          </Tooltip>
          {/* The other end of the link the upload page's own subscription line makes: a rule and
              what it downloaded are one workflow, and this is where you go to watch the second
              half. */}
          <Tooltip label={t("subscriptions.openUploadQueue") ?? ""}>
            <a
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              href={routes.upload()}
              aria-label={t("subscriptions.openUploadQueue") ?? "Queue"}
            >
              <i className="fa fa-download" aria-hidden="true"></i>
            </a>
          </Tooltip>
        </span>
      }
    >
      {historyOpen && <SubscriptionHistoryModal onClose={() => setHistoryOpen(false)} />}
      {sources.isLoading || subscriptions.isLoading ? (
        <SubscriptionsSkeleton />
      ) : (sources.data?.length ?? 0) === 0 ? (
        // Stated rather than left as an empty form: a source only qualifies if its extension can
        // discover, and silently showing nothing would look like a bug.
        <p>{t("subscriptions.noSourcesAvailable")}</p>
      ) : (
        <SubscriptionsBody
          subscriptions={subscriptions.data ?? []}
          sources={sources.data ?? []}
        />
      )}
    </CollapsibleSection>
  )
}
function SubscriptionsBody({
  subscriptions,
  sources,
}: {
  subscriptions: Subscription[]
  sources: SubscriptionSource[]
}) {
  const { t } = useTranslation()
  const remove = useDeleteSubscription()
  const setState = useSetSubscriptionState()

  /** Which subscription's form is open, and in which mode. Editing reuses the create form rather
   * than a second one: the fields are identical, and a divergence between them would show up as a
   * field that can be set at creation but never changed afterwards. */
  const [editing, setEditing] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  /** Which subscription's check history is expanded. One at a time — history rows are tall. */
  const [historyFor, setHistoryFor] = useState<string | null>(null)
  /** Which subscription's preview is shown. One at a time — a preview lists a whole page of works. */
  const [previewFor, setPreviewFor] = useState<string | null>(null)
  const preview = usePreviewSubscription()
  const checkNow = useCheckSubscriptionNow()
  const create = useCreateSubscription()

  /** Duplicate a subscription as a starting point for another one. The copy starts disabled so it
   *  can be reviewed and switched on deliberately, rather than checking (and possibly downloading)
   *  before its rules have been adjusted. */
  function copySubscription(s: Subscription) {
    void create
      .mutateAsync({
        ...bodyOf(s),
        name: t("subscriptions.copyName", { name: s.name }) ?? s.name,
        enabled: false,
      })
      .then(() => {
        toast({ text: t("subscriptions.copied") ?? undefined, icon: "success" })
      })
      .catch((e) => {
        toast({ text: String(e), icon: "error" })
      })
  }

  const historySubscription = subscriptions.find((s) => s.id === historyFor)
  const previewSubscription = subscriptions.find((s) => s.id === previewFor)
  const previewSource = sources.find((source) => source.namespace === previewSubscription?.source)
  const previewFields = previewSource?.candidate_fields

  function openPreview(s: Subscription) {
    // Clear the previous row's result/error before the new request starts; otherwise the modal can
    // briefly show the last subscription's preview while this one is loading.
    preview.reset()
    setPreviewFor(s.id)
    void preview.mutateAsync({ id: s.id }).catch(() => {})
  }

  function renderTiming(s: Subscription) {
    if (settlingUntil(s)) {
      // Said plainly, or a subscription that saved cleanly and then does nothing for a minute reads
      // as broken.
      return <span style={{ color: "#c79121" }}>{t("subscriptions.settling")}</span>
    }
    return s.last_checked_at ? formatTime(s.last_checked_at) : t("subscriptions.neverChecked")
  }

  function renderState(s: Subscription) {
    if (s.state.state === "paused") {
      // A pause is the system's doing, so it owes the user both the reason and a way back — unlike
      // "disabled", which the user chose.
      return (
        <span style={{ color: "#c79121" }}>
          {t("subscriptions.pausedInsufficientCredit")}{" "}
          <input
            type="button"
            className="stdbtn"
            value={t("subscriptions.resume") ?? undefined}
            onClick={() => void setState.mutateAsync({ id: s.id, action: "resume" })}
          />
        </span>
      )
    }
    return (
      <label>
        <input
          type="checkbox"
          className="fa"
          checked={s.state.state === "enabled"}
          onChange={(e) =>
            void setState.mutateAsync({
              id: s.id,
              action: e.target.checked ? "enable" : "disable",
            })
          }
        />{" "}
        {s.state.state === "enabled"
          ? t("subscriptions.enabled")
          : t("subscriptions.disabled")}
      </label>
    )
  }

  function renderActions(s: Subscription) {
    return (
      <>
        <Tooltip label={t("subscriptions.preview") ?? ""}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            aria-label={t("subscriptions.preview") ?? "Preview"}
            // Enabled regardless of the subscription's state: checking what a rule would catch is
            // exactly what is done while it is switched off and being tuned.
            disabled={preview.isPending}
            onClick={() => openPreview(s)}
          >
            <i className="fa fa-search" aria-hidden="true"></i>
          </button>
        </Tooltip>{" "}
        <Tooltip label={t("subscriptions.checkNow") ?? ""}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            aria-label={t("subscriptions.checkNow") ?? "Check now"}
            onClick={() => {
              void checkNow.mutateAsync(s.id).then(() => {
                toast({ text: t("subscriptions.checkStarted") ?? undefined, icon: "info" })
              })
            }}
          >
            <i className="fa fa-refresh" aria-hidden="true"></i>
          </button>
        </Tooltip>{" "}
        <Tooltip label={t("subscriptions.copy") ?? ""}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            aria-label={t("subscriptions.copy") ?? "Duplicate"}
            disabled={create.isPending}
            onClick={() => copySubscription(s)}
          >
            <i className="fa fa-clone" aria-hidden="true"></i>
          </button>
        </Tooltip>{" "}
        <Menu
            align="end"
            trigger={
              <button
                type="button"
                className="stdbtn"
                style={ICON_BUTTON_STYLE}
                aria-label={t("subscriptions.moreActions") ?? "More actions"}
              >
                <i className="fa fa-ellipsis-h" aria-hidden="true"></i>
              </button>
            }
          >
            <MenuItem onClick={() => setHistoryFor(historyFor === s.id ? null : s.id)}>
              <i className="fa fa-history" style={{ marginRight: 8 }} aria-hidden="true" />
              {t("subscriptions.viewHistory")}
            </MenuItem>
            <MenuItem
              onClick={() => {
                setCreating(false)
                setEditing(editing === s.id ? null : s.id)
              }}
            >
              <i className="fa fa-pencil" style={{ marginRight: 8 }} aria-hidden="true" />
              {t("subscriptions.edit")}
            </MenuItem>
            <MenuSeparator />
            <MenuItem
              onClick={() => {
                void confirmDialog(
                  t("subscriptions.confirmDelete", { name: s.name }) ?? "",
                ).then((ok) => {
                  if (ok) void remove.mutateAsync(s.id)
                })
              }}
            >
              <i className="fa fa-times" style={{ marginRight: 8 }} aria-hidden="true" />
              {t("subscriptions.delete")}
            </MenuItem>
        </Menu>
      </>
    )
  }

  return (
    <div>
      <PendingApprovals subscriptions={subscriptions} />

      <div className="settings-table-scroll">
        <table className="itg" style={{ minWidth: 760 }}>
          <thead>
            <tr className="jtr0">
              <th>{t("subscriptions.name")}</th>
              <th>{t("subscriptions.source")}</th>
              <th>{t("subscriptions.interval")}</th>
              <th>{t("subscriptions.lastChecked")}</th>
              <th>{t("subscriptions.state")}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {subscriptions.length === 0 && (
              <tr className="gtr1">
                <td colSpan={6}>{t("subscriptions.none")}</td>
              </tr>
            )}
            {subscriptions.map((s) => (
              <Fragment key={s.id}>
                <tr className="gtr1">
                  <td>{s.name}</td>
                  <td>{s.source}</td>
                  <td>{Math.round(s.interval_secs / 3600)}h</td>
                  <td>{renderTiming(s)}</td>
                  <td>{renderState(s)}</td>
                  <td style={{ whiteSpace: "nowrap" }}>{renderActions(s)}</td>
                </tr>
              </Fragment>
            ))}
          </tbody>
        </table>
      </div>

      <input
        type="button"
        className="stdbtn"
        style={{ marginTop: 8 }}
        value={t("subscriptions.addNew") ?? undefined}
        onClick={() => {
          setEditing(null)
          setCreating(true)
        }}
      />

      {previewFor && previewSubscription && (
        <Modal onClose={() => setPreviewFor(null)} width={1180} textAlign="left">
          <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 10px" }}>
            {t("subscriptions.previewPaneTitle")} — {previewSubscription.name}
          </h3>
          {/* Same fixed pane as the edit modal, so loading -> loaded does not resize the dialog. */}
          <div className="sub-modal-preview">
            {preview.isPending ? (
              // Reuse the same shaped skeleton as the edit modal's preview pane, so opening either
              // preview enters the same loading state instead of a one-line spinner message.
              <PreviewSkeleton fields={previewFields} />
            ) : preview.isError ? (
              <p style={{ color: "red" }}>{String(preview.error)}</p>
            ) : preview.data ? (
              <SubscriptionPreview preview={preview.data} subscriptionId={previewFor} fields={previewFields} />
            ) : (
              <p>{t("subscriptions.previewRunning")}</p>
            )}
          </div>
        </Modal>
      )}

      {historySubscription && (
        <SubscriptionCheckHistoryModal
          subscription={historySubscription}
          onClose={() => setHistoryFor(null)}
        />
      )}

      {/* One modal serves both: the form is identical and `subscriptionId` is the only difference. */}
      {(creating || editing) && (
        <SubscriptionFormModal
          sources={sources}
          subscriptionId={editing ?? undefined}
          initial={
            editing
              ? bodyOf(subscriptions.find((x) => x.id === editing)!)
              : emptyBody(sources[0]?.namespace ?? "", sources[0]?.suggested_secs ?? 6 * 3600)
          }
          onClose={() => {
            setCreating(false)
            setEditing(null)
          }}
        />
      )}
    </div>
  )
}

/** Matched works awaiting a go-ahead. Shown above the subscription list, not inside each row: the
 * question "what is waiting for me" is the one a user opens this page to answer, and burying it one
 * expand-click deep in the row that produced it would hide it behind the thing they already know. */
export function PendingApprovals({ subscriptions }: { subscriptions: Subscription[] }) {
  const { t } = useTranslation()
  const pending = usePendingApprovals()
  const approve = useApprovePending()
  const dismiss = useDismissPending()
  /** Selection exists so a batch of matches can be answered in one go (FR-007b). Nothing is selected
   * by default — pre-selecting would make "approve selected" a one-click way to spend credit on
   * works the user has not looked at. */
  const [selected, setSelected] = useState<Set<string>>(new Set())

  const items = pending.data ?? []
  if (items.length === 0) return null

  const nameOf = (id: string) => subscriptions.find((s) => s.id === id)?.name ?? id
  const busy = approve.isPending || dismiss.isPending

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  async function runApprove(ids: string[]) {
    const result = await approve.mutateAsync(ids)
    setSelected(new Set())
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
    <div style={{ marginBottom: 12 }}>
      <h3 className="ih" style={{ fontSize: "1.0em", margin: "0 0 6px" }}>
        {t("subscriptions.awaitingApproval", { count: items.length })}
      </h3>
      <table className="itg" style={{ width: "100%" }}>
        <tbody>
          {items.map((p: PendingApproval) => (
            <tr key={p.id} className="gtr1">
              <td style={{ width: 24 }}>
                <input
                  type="checkbox"
                  className="fa"
                  checked={selected.has(p.id)}
                  onChange={() => toggle(p.id)}
                />
              </td>
              <td>
                <a href={sourceHref(p.source_url)} target="_blank" rel="noreferrer">
                  {p.title ?? p.source_url}
                </a>
                <span style={{ marginLeft: 8, opacity: 0.7 }}>{nameOf(p.subscription_id)}</span>
              </td>
              <td style={{ whiteSpace: "nowrap", textAlign: "right" }}>
                <input
                  type="button"
                  className="stdbtn"
                  disabled={busy}
                  value={t("subscriptions.approve") ?? undefined}
                  onClick={() => void runApprove([p.id])}
                />{" "}
                <input
                  type="button"
                  className="stdbtn"
                  disabled={busy}
                  value={t("subscriptions.dismiss") ?? undefined}
                  onClick={() => void dismiss.mutateAsync([p.id])}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {selected.size > 0 && (
        <div style={{ display: "flex", gap: 8, marginTop: 6 }}>
          <input
            type="button"
            className="stdbtn"
            disabled={busy}
            value={t("subscriptions.approveSelected", { count: selected.size }) ?? undefined}
            onClick={() => void runApprove([...selected])}
          />
          <input
            type="button"
            className="stdbtn"
            disabled={busy}
            value={t("subscriptions.dismissSelected", { count: selected.size }) ?? undefined}
            onClick={() => {
              void dismiss.mutateAsync([...selected]).then(() => setSelected(new Set()))
            }}
          />
        </div>
      )}
    </div>
  )
}

/** Stand-in rows while the list loads.
 *
 * The same table shell and column headers as the real list, with bars where the cells will be. A
 * bare "loading…" line is one row tall, so the section grew by a few hundred pixels the moment the
 * subscriptions arrived and pushed everything below it down. */
const SUBSCRIPTION_SKELETON_ROWS = 3

function SubscriptionsSkeleton() {
  const { t } = useTranslation()
  // Roughly each column's own content: a name and a namespace are long, the rest are short.
  const widths = ["9em", "11em", "3em", "7em", "5em", "13em"]
  return (
    <div className="settings-table-scroll" role="status" aria-label={t("common.loading") ?? "Loading"}>
      <table className="itg" style={{ minWidth: 760 }} aria-hidden="true">
        <thead>
          <tr className="jtr0">
            <th>{t("subscriptions.name")}</th>
            <th>{t("subscriptions.source")}</th>
            <th>{t("subscriptions.interval")}</th>
            <th>{t("subscriptions.lastChecked")}</th>
            <th>{t("subscriptions.state")}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: SUBSCRIPTION_SKELETON_ROWS }, (_, row) => (
            <tr className="gtr1 subscriptions-skeleton-row" key={row}>
              {widths.map((width, col) => (
                <td key={col}>
                  <span
                    className="skeleton-bar"
                    style={{ display: "block", width: `${55 + ((row * 13 + col * 7) % 40)}%`, maxWidth: width }}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
