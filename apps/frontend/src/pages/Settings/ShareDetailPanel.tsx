import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { type DragHandleProps,Modal, SortableList } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { DateTimeStack, IpGeoLink } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { useRemoveShareItems, useReorderShareItems, useRevokeShare, useShare, useShareSettings, useUpdateShare } from "@/share/api"
import { fieldsFromForm, formFromShare, permissionsChosen, ShareFieldsForm, type ShareFormState } from "@/share/ShareFieldsForm"
import { ShareResult } from "@/share/ShareResult"
import type { ResolvedShareItem, ShareDetail, ShareItemRef } from "@/share/types"
import { toast } from "@/toast"

/** One shared item: a drag handle and up/down buttons while the share can be reordered, plus a
 * remove button while it has more than one item. */
function ShareDetailItem({
  item,
  drag,
  onMoveUp,
  onMoveDown,
  onRemove,
}: {
  item: ResolvedShareItem
  drag?: DragHandleProps
  onMoveUp?: () => void
  onMoveDown?: () => void
  onRemove?: () => void
}) {
  const { t } = useTranslation()
  const [failed, setFailed] = useState(false)
  const base = item.kind === "tankoubon" ? "tankoubons" : "archives"
  const title = item.title ?? item.id
  return (
    <div className="share-preview-item share-detail-item">
      {drag && (
        <span
          {...drag.attributes}
          {...drag.listeners}
          className="share-detail-drag"
          title={t("share.detail.dragToReorder")}
          aria-label={t("share.detail.dragToReorder")}
          style={{ cursor: drag.isDragging ? "grabbing" : "grab" }}
        >
          <i className="fa fa-grip-vertical" aria-hidden="true"></i>
        </span>
      )}
      <img
        src={failed || !item.available ? "/legacy/img/noThumb.png" : `/api/${base}/${encodeURIComponent(item.id)}/thumbnail`}
        alt=""
        loading="lazy"
        onError={() => setFailed(true)}
      />
      <span className="share-preview-title" title={title}>
        {item.available ? title : <s>{title}</s>}
      </span>
      {item.member_count !== null && (
        <span className="share-badge">{t("share.page.volumes", { count: item.member_count })}</span>
      )}
      {!item.available && <span className="share-badge share-status-revoked">{t("share.page.unavailableItem")}</span>}
      {(onMoveUp || onMoveDown) && (
        <span className="share-detail-move">
          <IconButton
            variant="ghost-btn"
            icon={<i className="fa fa-chevron-up" />}
            size={22}
            title={t("share.detail.moveUp")}
            aria-label={t("share.detail.moveUp")}
            disabled={!onMoveUp}
            onClick={onMoveUp}
          />
          <IconButton
            variant="ghost-btn"
            icon={<i className="fa fa-chevron-down" />}
            size={22}
            title={t("share.detail.moveDown")}
            aria-label={t("share.detail.moveDown")}
            disabled={!onMoveDown}
            onClick={onMoveDown}
          />
        </span>
      )}
      {onRemove && (
        <IconButton
          variant="ghost-btn"
          icon={<i className="fa fa-trash" />}
          size={22}
          title={t("share.detail.removeItem")}
          aria-label={t("share.detail.removeItem")}
          onClick={onRemove}
        />
      )}
    </div>
  )
}

/** One share's items, claim history and settings, with Show code and Revoke (017 US2, FR-005/6). */
export function ShareDetailPanel({ shareId }: { shareId: string }) {
  const detail = useShare(shareId)
  if (!detail.data) return <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>
  return <ShareDetailBody key={detail.dataUpdatedAt} share={detail.data} />
}

function ShareDetailBody({ share }: { share: ShareDetail }) {
  const { t } = useTranslation()
  const settings = useShareSettings()
  const update = useUpdateShare()
  const removeItems = useRemoveShareItems()
  const reorderItems = useReorderShareItems()
  const revoke = useRevokeShare()
  const [form, setForm] = useState<ShareFormState>(() => formFromShare(share))
  const [showCode, setShowCode] = useState(false)

  const revoked = share.status === "revoked"
  const failed = (e: unknown) =>
    toast({ heading: t("share.settings.saveFailed"), text: e instanceof ApiError ? e.message : String(e), icon: "error" })

  // The order lives here while a reorder is in flight, so the row moves under the pointer instead
  // of waiting for the round trip; the panel is re-keyed on the refetched share, which takes over.
  const toRefs = (items: ResolvedShareItem[]): ShareItemRef[] => items.map(({ kind, id }) => ({ kind, id }))
  // Re-keyed on every refetch (`ShareDetailPanel`), so this starts from the server's order each
  // time one arrives and no effect has to re-sync it.
  const [order, setOrder] = useState<ShareItemRef[]>(() => toRefs(share.items))

  const keyOf = (ref: ShareItemRef) => `${ref.kind}:${ref.id}`
  const byKey = new Map(share.items.map((item) => [keyOf(item), item]))

  const applyOrder = (next: ShareItemRef[]) => {
    setOrder(next)
    reorderItems.mutate(
      { id: share.id, items: next },
      { onError: (e) => { failed(e); setOrder(toRefs(share.items)) } },
    )
  }

  const move = (from: number, to: number) => {
    if (to < 0 || to >= order.length || from === to) return
    const next = order.slice()
    const [moved] = next.splice(from, 1)
    next.splice(to, 0, moved)
    applyOrder(next)
  }

  const reorderable = !revoked && order.length > 1

  return (
    <div className="share-detail">
      <h3>{t("share.detail.items")}</h3>
      <div className="share-preview-list">
        <SortableList
          items={order}
          getId={keyOf}
          // A drop hands back the complete new order; anything unrecognised (a stale id) is
          // dropped rather than sent to the server.
          onReorder={(keys) => {
            const next = keys.flatMap((k) => {
              const ref = order.find((r) => keyOf(r) === k)
              return ref ? [ref] : []
            })
            if (reorderable && next.length === order.length) applyOrder(next)
          }}
          renderItem={(ref, drag) => {
            const item = byKey.get(keyOf(ref))
            if (!item) return null
            const index = order.findIndex((r) => keyOf(r) === keyOf(ref))
            return (
              <ShareDetailItem
                item={item}
                drag={reorderable ? drag : undefined}
                onMoveUp={reorderable && index > 0 ? () => move(index, index - 1) : undefined}
                onMoveDown={reorderable && index < order.length - 1 ? () => move(index, index + 1) : undefined}
                onRemove={
                  !revoked && share.items.length > 1
                    ? () => removeItems.mutate({ id: share.id, items: [{ kind: item.kind, id: item.id }] }, { onError: failed })
                    : undefined
                }
              />
            )
          }}
        />
      </div>

      <h3>{t("share.detail.claims", { used: share.downloads_used, limit: share.download_limit ?? "∞" })}</h3>
      {share.claims.length === 0 ? (
        <p>{t("share.detail.noClaims")}</p>
      ) : (
        <table className="itg share-detail-table">
          <thead>
            <tr>
              <th>{t("share.detail.claimKind")}</th>
              <th>{t("share.detail.claimAddress")}</th>
              <th>{t("share.detail.claimTime")}</th>
            </tr>
          </thead>
          <tbody>
            {share.claims.map((c) => (
              <tr key={c.claim_id}>
                <td>{t(`share.claimKind.${c.kind}`)}</td>
                <td>
                  <IpGeoLink ip={c.first_address} />
                </td>
                <td>
                  <DateTimeStack epochSeconds={c.started_at} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {(share.allow_helpers || share.helpers.length > 0) && (
        <>
          <h3>{t("share.detail.helpers")}</h3>
          {share.helpers.length === 0 ? (
            <p>{t("share.detail.noHelpers")}</p>
          ) : (
            <table className="itg share-detail-table">
              <tbody>
                {share.helpers.map((h) => (
                  <tr key={h.endpoint_id}>
                    <td>
                      <code title={h.endpoint_id}>{h.endpoint_id.slice(0, 16)}…</code>
                    </td>
                    <td>
                      <DateTimeStack epochSeconds={h.last_seen} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}

      {!revoked && (
        <>
          <h3>{t("share.detail.settings")}</h3>
          <ShareFieldsForm value={form} onChange={setForm} embedAvailable={Boolean(settings.data?.share_public_url)} />
        </>
      )}

      <div className="share-detail-actions">
        <button type="button" className="stdbtn" onClick={() => setShowCode(true)}>
          {t("share.detail.showCode")}
        </button>
        {!revoked && (
          <button
            type="button"
            className="stdbtn stdbtn-danger"
            onClick={async () => {
              if (await confirmDialog(t("share.detail.revokeConfirm"), true)) revoke.mutate(share.id, { onError: failed })
            }}
          >
            {t("share.detail.revoke")}
          </button>
        )}
        {!revoked && (
          <button
            type="button"
            className="stdbtn share-detail-save"
            disabled={update.isPending || !permissionsChosen(form)}
            onClick={() =>
              update.mutate(
                { id: share.id, fields: fieldsFromForm(form) },
                { onSuccess: () => toast({ heading: t("share.settings.saved"), icon: "success", hideAfter: 3000 }), onError: failed },
              )
            }
          >
            {t("share.settings.save")}
          </button>
        )}
      </div>
      {/* Portaled after the detail modal, so it stacks above it. */}
      {showCode && (
        <Modal onClose={() => setShowCode(false)} width={560} textAlign="left">
          <h2 className="share-modal-title">{share.title || t("share.detail.showCode")}</h2>
          <ShareResult shareId={share.id} />
        </Modal>
      )}
    </div>
  )
}
