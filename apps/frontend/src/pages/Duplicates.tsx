import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { ApiError, fetchJson } from "@/api/client"
import {
  useClearDuplicates,
  useDeleteArchive,
  useDuplicateGroups,
  useSaveDuplicateDecisions,
  useScanDuplicates,
} from "@/api/hooks"
import type { DuplicateGroupView, DuplicateMember, DuplicateScanStatus } from "@/api/types"
import { Button, NumberInput } from "@/components/common-ui/Form"
import { STATE_COLOR } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import {
  DEFAULT_DUPLICATES_COVER_PERCENT,
  DUPLICATES_COVER_PERCENT_KEY,
  MAX_DUPLICATES_COVER_PERCENT,
  MIN_DUPLICATES_COVER_PERCENT,
} from "@/lib/storageKeys"
import { SmartCompareModal } from "@/pages/DuplicateCompare/SmartCompareModal"

/** The cover slider is stepped, not continuous: five preset widths read as "small → large" and
 *  are easy to hit on a phone, where a 35-step range is not. The stored value is the percentage,
 *  so a slider position always means the same thing across screens. */
const COVER_STOPS = [16, 24, 32, 40, 50] as const

/** How many lines a title may take. A wider cover leaves less room beside it, so the text needs
 *  more lines for the same title — 2 lines at the smallest thumbnail, 5 at the largest. */
function titleLines(percent: number): number {
  if (percent >= 50) return 5
  if (percent >= 40) return 4
  if (percent >= 32) return 3
  return 2
}

function nearestCoverStop(percent: number): number {
  return COVER_STOPS.reduce((best, stop) =>
    Math.abs(stop - percent) < Math.abs(best - percent) ? stop : best,
  )
}
import { useApplyTheme } from "@/theme"

/**
 * Duplicate review (spec 024). Each group is a card: members with checkboxes (default: keep),
 * a suggestion the local review produced (or the model's, when one is configured), and one
 * primary action — the side-by-side comparison.
 */
/** A running scan: what it is doing, how far, and what that means for the groups shown. */
function ScanProgressPanel({ status }: { status: DuplicateScanStatus }) {
  const { t } = useTranslation()
  const percent = Math.round(status.progress * 100)
  const phase = status.phase ? t(`duplicates.phase.${status.phase}`) : t("duplicates.phase.preparing")
  return (
    <div className="mb-3 text-sm" role="status">
      <div className="mb-1 flex items-center gap-2">
        <i className="fa fa-spinner fa-spin" aria-hidden="true" />
        <span>
          {phase}
          {status.total > 0 ? ` · ${status.done.toLocaleString()} / ${status.total.toLocaleString()}` : ""}
        </span>
        <span className="ml-auto tabular-nums opacity-70">{percent}%</span>
      </div>
      {/* The track is translucent grey and the fill the theme's job colour, so it reads on every theme. */}
      <div style={{ height: 6, borderRadius: 3, background: "rgba(128,128,128,0.25)", overflow: "hidden" }}>
        <div style={{ width: `${percent}%`, height: "100%", background: STATE_COLOR.active, transition: "width 0.4s" }} />
      </div>
      <p className="mt-2 text-xs">
        <i className="fa fa-triangle-exclamation" aria-hidden="true" /> {t("duplicates.scanningWarning")}
      </p>
    </div>
  )
}

export function Duplicates() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const groups = useDuplicateGroups()
  const scanDuplicates = useScanDuplicates()
  const clearDuplicates = useClearDuplicates()
  useApplyTheme()
  useDocumentTitle(t("app.duplicateDetection") ?? undefined)

  const [threshold, setThreshold] = useState(7)
  const [coverPercent, setCoverPercent] = useState(() => {
    const stored = Number(localStorage.getItem(DUPLICATES_COVER_PERCENT_KEY))
    if (!Number.isFinite(stored) || stored <= 0) return DEFAULT_DUPLICATES_COVER_PERCENT
    return nearestCoverStop(
      Math.min(MAX_DUPLICATES_COVER_PERCENT, Math.max(MIN_DUPLICATES_COVER_PERCENT, stored)),
    )
  })

  useEffect(() => {
    localStorage.setItem(DUPLICATES_COVER_PERCENT_KEY, String(coverPercent))
  }, [coverPercent])
  const [liveStatus, setLiveStatus] = useState<DuplicateScanStatus | null>(null)
  const scan = liveStatus ?? groups.data?.status ?? null
  const busy = Boolean(scan?.scanning) || scanDuplicates.isPending

  // The scan is a server job: it keeps running when this page is left or reloaded, and it publishes
  // the groups found so far as it goes. The server pushes its status on connect and on every change
  // (progress included, about once a second); the groups are refetched only when their revision
  // moved or a scan just ended (its model reviews change the cards without changing the groups).
  useEffect(() => {
    const source = new EventSource("/api/database/duplicates/events")
    let last: DuplicateScanStatus | null = null
    source.addEventListener("status", (event) => {
      let status: DuplicateScanStatus
      try {
        status = JSON.parse((event as MessageEvent<string>).data) as DuplicateScanStatus
      } catch {
        return
      }
      if (!last || status.revision !== last.revision || (last.scanning && !status.scanning)) void groups.refetch()
      last = status
      setLiveStatus(status)
    })
    return () => source.close()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- one subscription per visit
  }, [])

  async function handleScan() {
    try {
      await scanDuplicates.mutateAsync(threshold)
    } catch (error) {
      // 409: a scan started elsewhere (another tab, or before a reload) is already running, and the
      // event stream is already following it.
      if (!(error instanceof ApiError && error.status === 409)) throw error
    }
  }

  // A group whose other members were deleted since the scan has nothing left to compare.
  const list = (groups.data?.groups ?? []).filter((group) => group.members.length >= 2)
  // A group whose members were all kept is handled; a rescan with the same members keeps it so.
  const pending = list.filter((group) => !group.resolved)
  const handled = list.filter((group) => group.resolved)

  return (
    <div className="ido duplicates-page">
      <h1 className="ih">{t("app.duplicateDetection")}</h1>
      <p>{t("duplicates.intro")}</p>

      <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className="flex items-center gap-2 whitespace-nowrap text-sm">
          {t("duplicates.threshold")}
          <div className="w-16">
            <NumberInput value={threshold} min={0} max={7} onValueChange={setThreshold} />
          </div>
        </label>
        <label className="ml-auto flex items-center gap-2 text-xs whitespace-nowrap">
          <i className="fa fa-image opacity-70" aria-hidden="true" />
          <input
            type="range"
            min={0}
            max={COVER_STOPS.length - 1}
            step={1}
            value={Math.max(0, COVER_STOPS.indexOf(coverPercent as (typeof COVER_STOPS)[number]))}
            onChange={(e) => setCoverPercent(COVER_STOPS[Number(e.target.value)])}
            title={t("duplicates.coverSize") ?? undefined}
            aria-label={t("duplicates.coverSize")}
            aria-valuetext={`${coverPercent}%`}
            list="duplicate-cover-stops"
          />
          <datalist id="duplicate-cover-stops">
            {COVER_STOPS.map((stop) => (
              <option key={stop} value={COVER_STOPS.indexOf(stop)} />
            ))}
          </datalist>
          <span className="w-8 text-right tabular-nums opacity-70">{coverPercent}%</span>
        </label>
        <div className="flex items-center gap-2">
          <button className="stdbtn" onClick={handleScan} disabled={busy}>
            {busy && <i className="fa fa-spinner fa-spin mr-2" />}
            {t("duplicates.scan")}
          </button>
          {list.length > 0 && (
            <button
              className="stdbtn"
              onClick={async () => {
                if (await confirmDialog(t("duplicates.clearConfirm"), true)) {
                  await clearDuplicates.mutateAsync()
                }
              }}
            >
              {t("duplicates.clear")}
            </button>
          )}
        </div>
      </div>
      <p className="mb-4 text-xs opacity-70">
        {groups.data?.scanned_at
          ? t("duplicates.lastScan", {
              count: pending.length,
              chained: groups.data.chained_groups,
            })
          : busy
            ? t("duplicates.scanning")
            : t("duplicates.neverScanned")}
      </p>

      {scan?.scanning && <ScanProgressPanel status={scan} />}
      {scan && !scan.scanning && scan.interrupted && (
        <p className="mb-3 text-sm" role="status">
          <i className="fa fa-triangle-exclamation" aria-hidden="true" /> {t("duplicates.interrupted")}
        </p>
      )}
      {scan && !scan.scanning && scan.no_cover > 0 && (
        <p className="mb-3 text-xs opacity-70">{t("duplicates.noCover", { count: scan.no_cover })}</p>
      )}

      {groups.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {!groups.isLoading && !busy && pending.length === 0 && (
        <p className="opacity-70">{handled.length > 0 ? t("duplicates.nothingPending") : t("duplicates.empty")}</p>
      )}

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {pending.map((group) => (
          <GroupCard
            key={group.key}
            group={group}
            // Groups stay usable while a scan runs; the warning above says they may still change.
            busy={scanDuplicates.isPending}
            coverPercent={coverPercent}
          />
        ))}
      </div>

      {handled.length > 0 && (
        <details className="mt-6">
          <summary className="cursor-pointer text-sm font-semibold">
            {t("duplicates.handledSection", { count: handled.length })}
          </summary>
          <div className="mt-3 grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {handled.map((group) => (
              <GroupCard key={group.key} group={group} busy={scanDuplicates.isPending} coverPercent={coverPercent} />
            ))}
          </div>
        </details>
      )}

      <Button id="return" className="mt-4" onClick={() => navigate(routes.library())}>
        {t("common.returnToLibrary")}
      </Button>
    </div>
  )
}

function GroupCard({
  group,
  busy,
  coverPercent,
}: {
  group: DuplicateGroupView
  busy: boolean
  coverPercent: number
}) {
  const { t } = useTranslation()
  const save = useSaveDuplicateDecisions()
  const deleteArchive = useDeleteArchive()
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(group.members.filter((m) => m.decision !== "discard").map((m) => m.id)),
  )
  const [expanded, setExpanded] = useState(false)
  const [compare, setCompare] = useState<[string, string] | null>(null)

  // The modal records its own decision; mirror it into the checkboxes, which only read the saved
  // decisions when the card mounts.
  function applyCompareDecision(decisions: Record<string, "keep" | "discard">) {
    setSelected((current) => {
      const next = new Set(current)
      for (const [id, state] of Object.entries(decisions)) {
        if (state === "keep") next.add(id)
        else next.delete(id)
      }
      return next
    })
  }

  const visible = expanded ? group.members : group.members.slice(0, 3)
  const hidden = group.members.length - visible.length
  const keepCount = selected.size
  const maxSimilarity = group.pairs.reduce((max, p) => Math.max(max, p.similarity), 0)

  function toggle(id: string) {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  async function persist(states: Record<string, "keep" | "discard" | "undecided">, pairs?: [string, string][]) {
    const decisions: Record<string, "keep" | "discard" | "undecided"> = {}
    for (const member of group.members) {
      const state = states[member.id]
      if (state) decisions[member.id] = state
    }
    await save.mutateAsync({ group_key: group.key, decisions, not_duplicate_pairs: pairs })
  }

  async function keepSelected() {
    const states: Record<string, "keep" | "discard"> = {}
    for (const member of group.members) {
      states[member.id] = selected.has(member.id) ? "keep" : "discard"
    }
    await persist(states)
  }

  async function onlyRecommended() {
    const review = group.review
    if (!review) return
    // The review only ever sees the first `MAX_MEMBERS` members, so `keep`/`discard` are the only
    // ids it actually ruled on. Members outside that set keep whatever the user had chosen —
    // marking them "discard" would present an opinion the model never held.
    const judged = new Set([...review.keep, ...review.discard])
    const states: Record<string, "keep" | "discard"> = {}
    for (const member of group.members) {
      if (!judged.has(member.id)) continue
      states[member.id] = review.keep.includes(member.id) ? "keep" : "discard"
    }
    setSelected((current) => {
      const next = new Set(current)
      for (const id of review.keep) next.add(id)
      for (const id of review.discard) next.delete(id)
      return next
    })
    await persist(states)
  }

  async function markNotDuplicate() {
    const pair = bestPair(group)
    if (!pair) return
    await persist({}, [pair])
  }

  async function deleteUnselected() {
    if (keepCount === 0 || keepCount === group.members.length) return
    const doomed = group.members.filter((m) => !selected.has(m.id))
    // Spec FR-011: say what else these archives belong to before deleting them.
    const impact = await describeImpact(doomed)
    if (
      !(await confirmDialog(
        [t("duplicates.deleteConfirm", { count: doomed.length }), impact].join("\n\n"),
        true,
        true,
      ))
    ) {
      return
    }
    for (const member of doomed) {
      await deleteArchive.mutateAsync(member.id)
    }
  }

  return (
    <div className="flex flex-col rounded border border-[var(--theme-border,#b9a89a)] p-3">
      <div className="mb-2 flex items-center justify-between text-sm">
        <span className="font-semibold">
          {t("duplicates.members", { count: group.members.length })}
        </span>
        <span className="flex items-center gap-2 text-xs opacity-80">
          {group.resolved && (
            <>
              <span className="rounded bg-black/10 px-1.5 py-0.5">
                <i className="fa fa-check mr-1" aria-hidden="true" />
                {t("duplicates.allKept")}
              </span>
              <button
                type="button"
                className="underline opacity-80"
                disabled={save.isPending}
                onClick={() =>
                  void persist(Object.fromEntries(group.members.map((m) => [m.id, "undecided" as const])))
                }
              >
                {t("duplicates.undoHandled")}
              </button>
            </>
          )}
          {group.chained && (
            <span className="rounded bg-black/10 px-1.5 py-0.5">
              {t("duplicates.chained")}
            </span>
          )}
          <span>{Math.round(maxSimilarity * 100)}%</span>
        </span>
      </div>

      {/* `ps-0`: the member list is a bare flex column. Without it the browser's (or a legacy
          * theme's) default `ul` indent of ~40px reads as an unexplained gap before the first
          checkbox. */}
      <ul className="m-0 flex list-none flex-col gap-2 ps-0">
        {visible.map((member) => (
          <MemberRow
            key={member.id}
            member={member}
            checked={selected.has(member.id)}
            onToggle={() => toggle(member.id)}
            coverPercent={coverPercent}
          />
        ))}
      </ul>

      {hidden > 0 && (
        <button className="mt-1 self-start text-xs underline" onClick={() => setExpanded(true)}>
          {t("duplicates.showMore", { count: hidden })}
        </button>
      )}

      {group.review && (
        <div className="mt-2 rounded bg-black/5 p-2 text-left text-xs">
          <div className="mb-1 flex items-center gap-1.5 font-semibold">
            <i className="fa fa-lightbulb opacity-80" aria-hidden="true" />
            <span>{t(`duplicates.verdict.${group.review.verdict}`)}</span>
            {group.review.fallback && (
              <span className="font-normal opacity-70">{t("duplicates.localOnly")}</span>
            )}
          </div>
          {group.review.reason && <div className="opacity-80">{group.review.reason}</div>}
          {(group.patch_links ?? []).map((link) => (
            <div key={`${link.target}-${link.source}`} className="opacity-80">
              {t("duplicates.patchLinkNote", {
                target: titleOf(group, link.target),
                source: titleOf(group, link.source),
              })}
            </div>
          ))}
        </div>
      )}

      <div className="mt-3 flex flex-col gap-2">
        <button
          className="stdbtn w-full"
          disabled={busy || group.members.length < 2}
          onClick={() => {
            const pair = bestPair(group)
            if (pair) setCompare(pair)
          }}
        >
          <i className="fa fa-columns mr-2" />
          {t("duplicates.smartCompare")}
        </button>
        <div className="flex gap-2">
          <button className="stdbtn flex-1" disabled={save.isPending || keepCount === 0} onClick={keepSelected}>
            {t("duplicates.keepSelected", { count: keepCount })}
          </button>
          {group.review && (
            <button className="stdbtn flex-1" disabled={save.isPending} onClick={onlyRecommended}>
              {t("duplicates.keepSuggested")}
            </button>
          )}
        </div>
        <div className="flex gap-2 text-xs">
          <button className="stdbtn flex-1" disabled={save.isPending} onClick={markNotDuplicate}>
            {t("duplicates.notDuplicate")}
          </button>
          <button
            className="stdbtn flex-1"
            disabled={deleteArchive.isPending || keepCount === 0 || keepCount === group.members.length}
            onClick={deleteUnselected}
          >
            {t("duplicates.deleteUnselected", { count: group.members.length - keepCount })}
          </button>
        </div>
      </div>
      {compare && (
        <SmartCompareModal
          group={group}
          pair={compare}
          onDecided={applyCompareDecision}
          onClose={() => setCompare(null)}
        />
      )}
    </div>
  )
}

/** Fetches each archive's category and tankoubon memberships so the delete confirmation can name
 *  the places a deletion will also empty (spec FR-011). A failed lookup is reported as unknown
 *  rather than blocking the deletion. */
async function describeImpact(members: DuplicateMember[]): Promise<string> {
  const rows = await Promise.all(
    members.map(async (member) => {
      try {
        const [cats, tanks] = await Promise.all([
          fetchJson<{ categories: { id: string; name: string }[] }>(
            `/archives/${member.id}/categories`,
          ),
          fetchJson<{ tankoubons: string[] }>(`/archives/${member.id}/tankoubons`),
        ])
        const names = cats.categories.map((c) => c.name)
        return `${member.title}: ${names.length ? names.join(", ") : "-"} (+${tanks.tankoubons.length})`
      } catch {
        return `${member.title}: ?`
      }
    }),
  )
  return rows.join("\n")
}

function titleOf(group: DuplicateGroupView, id: string): string {
  return group.members.find((m) => m.id === id)?.title ?? id
}

function bestPair(group: DuplicateGroupView): [string, string] | null {
  const pair = group.pairs.reduce<null | { a: string; b: string; similarity: number }>(
    (best, p) => (best === null || p.similarity > best.similarity ? p : best),
    null,
  )
  if (!pair) return null
  // The suggested keeper goes on side A, so the comparison reads "recommended vs. its closest copy".
  const recommendedIsB = group.members.find((m) => m.id === pair.b)?.recommended
  return recommendedIsB ? [pair.b, pair.a] : [pair.a, pair.b]
}

function MemberRow({
  member,
  checked,
  onToggle,
  coverPercent,
}: {
  member: DuplicateMember
  checked: boolean
  onToggle: () => void
  coverPercent: number
}) {
  const { t } = useTranslation()
  const language = (member.tags ?? [])
    .find((tag) => tag.toLowerCase().startsWith("language:"))
    ?.slice("language:".length)
    .trim()
  return (
    <li className="flex items-start gap-2 text-left">
      {/* The checkbox and the two member badges share one narrow gutter on the left: the title
          gets the whole remaining width instead of losing it to icons on the right. */}
      <div className="flex shrink-0 flex-col items-center gap-1.5 pt-0.5">
        <input
          type="checkbox"
          className="lrr-checkbox-bare shrink-0"
          checked={checked}
          onChange={onToggle}
          aria-label={member.title}
        />
        {member.recommended && <i className="fa fa-star text-xs" title={t("duplicates.recommended")} />}
        {member.best && <i className="fa fa-crown text-xs" title={t("duplicates.bestCopy")} />}
        <a
          href={`/reader/${member.id}`}
          className="px-1 opacity-70 hover:opacity-100"
          title={t("common.read")}
          aria-label={t("common.read")}
        >
          <i className="fa fa-book-open" aria-hidden="true" />
        </a>
      </div>
      <img
        src={member.cover}
        alt=""
        className="shrink-0 object-cover"
        style={{ width: `${coverPercent}%`, aspectRatio: "5 / 7", height: "auto" }}
        loading="lazy"
      />
      <div className="min-w-0 flex-1 text-left text-sm">
        <span
          className="break-words text-left"
          title={member.title}
          style={{
            display: "-webkit-box",
            WebkitBoxOrient: "vertical",
            WebkitLineClamp: titleLines(coverPercent),
            overflow: "hidden",
          }}
        >
          {member.title}
        </span>
        <div className="mt-1 flex flex-wrap items-center gap-2 text-xs opacity-75">
          <span>
            {member.pages} {t("duplicates.pages")}
            {(member.patch_pages ?? 0) > 0 && (
              <span title={t("duplicates.patchPagesIncluded", { count: member.patch_pages })}>
                {" "}
                <i className="fa fa-puzzle-piece" aria-hidden="true" /> +{member.patch_pages}
              </span>
            )}
          </span>
          <span>{humanSize(member.size)}</span>
          {language && <span>{language}</span>}
          {member.decision === "discard" && <span>{t("duplicates.markedDiscard")}</span>}
        </div>
      </div>
    </li>
  )
}

function humanSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB"]
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`
}
