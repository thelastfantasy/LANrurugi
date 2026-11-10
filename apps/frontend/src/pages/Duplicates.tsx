import { useState } from "react"
import { useTranslation } from "react-i18next"

import { waitForJob } from "@/api/client"
import {
  useClearDuplicates,
  useDeleteArchive,
  useDuplicateCompare,
  useDuplicateGroups,
  useSaveDuplicateDecisions,
  useScanDuplicates,
} from "@/api/hooks"
import type { DuplicateGroupView, DuplicateMember } from "@/api/types"
import { NumberInput } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { useApplyTheme } from "@/theme"

/**
 * Duplicate review (spec 024). Each group is a card: members with checkboxes (default: keep),
 * a suggestion the local review produced (or the model's, when one is configured), and one
 * primary action — the side-by-side comparison.
 */
export function Duplicates() {
  const { t } = useTranslation()
  const groups = useDuplicateGroups()
  const scanDuplicates = useScanDuplicates()
  const clearDuplicates = useClearDuplicates()
  useApplyTheme()
  useDocumentTitle(t("app.duplicateDetection") ?? undefined)

  const [threshold, setThreshold] = useState(7)
  const [busy, setBusy] = useState(false)
  const [compare, setCompare] = useState<[string, string] | null>(null)

  async function handleScan() {
    setBusy(true)
    try {
      const { job } = await scanDuplicates.mutateAsync(threshold)
      await waitForJob(job)
      await groups.refetch()
    } finally {
      setBusy(false)
    }
  }

  const list = groups.data?.groups ?? []

  return (
    <div className="ido">
      <h1 className="ih">{t("app.duplicateDetection")}</h1>
      <p>{t("duplicates.intro")}</p>

      <div className="mb-4 flex flex-wrap items-end gap-3">
        <label className="flex items-center gap-2 text-sm">
          {t("duplicates.threshold")}
          <div className="w-20">
            <NumberInput value={threshold} min={0} max={16} onValueChange={setThreshold} />
          </div>
        </label>
        <button className="stdbtn" onClick={handleScan} disabled={busy || scanDuplicates.isPending}>
          {(busy || scanDuplicates.isPending) && <i className="fa fa-spinner fa-spin mr-2" />}
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
        <span className="text-xs opacity-70">
          {groups.data?.scanned_at
            ? t("duplicates.lastScan", {
                count: list.length,
                chained: groups.data.chained_groups,
              })
            : t("duplicates.neverScanned")}
        </span>
      </div>

      {groups.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {!groups.isLoading && list.length === 0 && (
        <p className="opacity-70">{t("duplicates.empty")}</p>
      )}

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {list.map((group) => (
          <GroupCard key={group.key} group={group} onCompare={setCompare} busy={busy} />
        ))}
      </div>

      {compare && (
        <SmartCompareModal
          a={compare[0]}
          b={compare[1]}
          onClose={() => setCompare(null)}
        />
      )}
    </div>
  )
}

function GroupCard({
  group,
  onCompare,
  busy,
}: {
  group: DuplicateGroupView
  onCompare: (pair: [string, string]) => void
  busy: boolean
}) {
  const { t } = useTranslation()
  const save = useSaveDuplicateDecisions()
  const deleteArchive = useDeleteArchive()
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(group.members.filter((m) => m.decision !== "discard").map((m) => m.id)),
  )
  const [expanded, setExpanded] = useState(false)

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

  async function persist(states: Record<string, "keep" | "discard">, pairs?: [string, string][]) {
    const decisions: Record<string, "keep" | "discard"> = {}
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
    if (!group.review) return
    const states: Record<string, "keep" | "discard"> = {}
    for (const member of group.members) {
      states[member.id] = group.review.keep.includes(member.id) ? "keep" : "discard"
    }
    setSelected(new Set(group.review.keep))
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
    if (
      !(await confirmDialog(
        t("duplicates.deleteConfirm", { count: doomed.length }),
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
          {group.chained && (
            <span className="rounded bg-black/10 px-1.5 py-0.5">
              {t("duplicates.chained")}
            </span>
          )}
          <span>{Math.round(maxSimilarity * 100)}%</span>
        </span>
      </div>

      <ul className="flex flex-col gap-2">
        {visible.map((member) => (
          <MemberRow
            key={member.id}
            member={member}
            checked={selected.has(member.id)}
            onToggle={() => toggle(member.id)}
          />
        ))}
      </ul>

      {hidden > 0 && (
        <button className="mt-1 self-start text-xs underline" onClick={() => setExpanded(true)}>
          {t("duplicates.showMore", { count: hidden })}
        </button>
      )}

      {group.review && (
        <div className="mt-2 rounded bg-black/5 p-2 text-xs">
          <div className="mb-1 font-semibold">
            {t(`duplicates.verdict.${group.review.verdict}`)}
            {group.review.fallback && (
              <span className="ml-2 font-normal opacity-70">{t("duplicates.localOnly")}</span>
            )}
          </div>
          <div className="opacity-80">{group.review.reason}</div>
        </div>
      )}

      <div className="mt-3 flex flex-col gap-2">
        <button
          className="stdbtn w-full"
          disabled={busy || group.members.length < 2}
          onClick={() => {
            const pair = bestPair(group)
            if (pair) onCompare(pair)
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
    </div>
  )
}

function bestPair(group: DuplicateGroupView): [string, string] | null {
  const pair = group.pairs.reduce<null | { a: string; b: string; similarity: number }>(
    (best, p) => (best === null || p.similarity > best.similarity ? p : best),
    null,
  )
  return pair ? [pair.a, pair.b] : null
}

function MemberRow({
  member,
  checked,
  onToggle,
}: {
  member: DuplicateMember
  checked: boolean
  onToggle: () => void
}) {
  const { t } = useTranslation()
  return (
    <li className="flex items-start gap-2">
      <input
        type="checkbox"
        className="mt-1"
        checked={checked}
        onChange={onToggle}
        aria-label={member.title}
      />
      <img src={member.cover} alt="" className="h-14 w-10 shrink-0 object-cover" loading="lazy" />
      <div className="min-w-0 flex-1 text-sm">
        <div className="flex items-start gap-1">
          <span className="line-clamp-2 break-words">{member.title}</span>
          {member.recommended && <i className="fa fa-star mt-0.5 text-xs" title={t("duplicates.recommended")} />}
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-2 text-xs opacity-75">
          <span>{member.pages} {t("duplicates.pages")}</span>
          <span>{humanSize(member.size)}</span>
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

export function SmartCompareModal({
  a,
  b,
  onClose,
}: {
  a: string
  b: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  const compare = useDuplicateCompare(a, b)
  const [index, setIndex] = useState(0)
  const data = compare.data
  const samples = data?.samples ?? []
  const sample = samples[Math.min(index, Math.max(0, samples.length - 1))]

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-black/70 p-2 md:p-6" role="dialog">
      <div className="mx-auto flex h-full w-full max-w-6xl flex-col overflow-hidden rounded bg-[var(--theme-bg,#f5efe8)]">
        <div className="flex items-center justify-between border-b p-3">
          <div className="text-sm">
            {data ? (
              <>
                <span className="font-semibold">{data.a.title}</span> · {data.a.pages}{" "}
                {t("duplicates.pages")} — <span className="font-semibold">{data.b.title}</span> ·{" "}
                {data.b.pages} {t("duplicates.pages")}
              </>
            ) : (
              t("duplicates.comparing")
            )}
          </div>
          <button className="stdbtn" onClick={onClose}>
            <i className="fa fa-times mr-1" />
            {t("common.close")}
          </button>
        </div>

        {data && (
          <div className="flex flex-wrap items-center gap-3 border-b px-3 py-2 text-xs">
            <span>{t("duplicates.aligned", { count: data.aligned_pairs })}</span>
            {data.recommendation && (
              <span className="rounded bg-black/10 px-1.5 py-0.5">
                {t("duplicates.recommendKeep")}{" "}
                {data.recommendation === "a" ? data.a.title : data.b.title}
              </span>
            )}
            {data.likely_different_language && (
              <span className="rounded bg-black/10 px-1.5 py-0.5">
                {t("duplicates.differentLanguage")}
              </span>
            )}
            {data.a_unmatched_pages.length > 0 && (
              <span>{t("duplicates.onlyInA", { count: data.a_unmatched_pages.length })}</span>
            )}
            {data.b_unmatched_pages.length > 0 && (
              <span>{t("duplicates.onlyInB", { count: data.b_unmatched_pages.length })}</span>
            )}
          </div>
        )}

        <div className="flex-1 overflow-auto p-3">
          {compare.isLoading && (
            <div id="processing">
              <i className="fa fa-3x fa-compact-disc fa-spin"></i>
            </div>
          )}
          {compare.isError && <p className="text-sm">{String(compare.error)}</p>}
          {sample && data && (
            <div className="grid grid-cols-2 gap-3">
              <figure>
                <img
                  src={`/api/archives/${encodeURIComponent(a)}/thumbnail?page=${sample.a_page_index + 1}`}
                  alt=""
                  className="w-full object-contain"
                />
                <figcaption className="mt-1 text-xs opacity-75">
                  {data.a.title} · p{sample.a_page_index + 1} ·{" "}
                  {sample.a_sharpness.toFixed(0)} {t("duplicates.sharpness")}
                </figcaption>
              </figure>
              <figure>
                <img
                  src={`/api/archives/${encodeURIComponent(b)}/thumbnail?page=${sample.b_page_index + 1}`}
                  alt=""
                  className="w-full object-contain"
                />
                <figcaption className="mt-1 text-xs opacity-75">
                  {data.b.title} · p{sample.b_page_index + 1} ·{" "}
                  {sample.b_sharpness.toFixed(0)} {t("duplicates.sharpness")}
                </figcaption>
              </figure>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-2 border-t p-3">
          <button className="stdbtn" disabled={index <= 0} onClick={() => setIndex((i) => Math.max(0, i - 1))}>
            <i className="fa fa-chevron-left" />
          </button>
          <span className="text-xs">
            {samples.length === 0 ? "" : `${index + 1} / ${samples.length}`}
          </span>
          <button
            className="stdbtn"
            disabled={index >= samples.length - 1}
            onClick={() => setIndex((i) => Math.min(samples.length - 1, i + 1))}
          >
            <i className="fa fa-chevron-right" />
          </button>
          <a
            className="stdbtn ml-auto"
            href={routes.reader(a)}
            target="_blank"
            rel="noreferrer"
          >
            {t("duplicates.openA")}
          </a>
          <a className="stdbtn" href={routes.reader(b)} target="_blank" rel="noreferrer">
            {t("duplicates.openB")}
          </a>
        </div>
      </div>
    </div>
  )
}
