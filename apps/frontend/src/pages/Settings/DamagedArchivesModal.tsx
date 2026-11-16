import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { useBatchDeleteArchives, useIntegrityScan, useRecheckArchive } from "@/api/hooks"
import type { DamagedArchive } from "@/api/types"
import { Modal } from "@/components/common-ui/Display"
import { Checkbox, CheckboxField } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { copyWithToast } from "@/lib/clipboard"
import { formatBytes } from "@/lib/formatBytes"
import { routes } from "@/lib/routes"
import { FONT_SIZE_XS } from "@/theme"
import { toast } from "@/toast"

/** #292/#291: the archives a scan (or a failed self-heal) found damaged, filterable by title or
 * path, with a batch move to the recycle bin and a per-archive re-check for a replaced file. */
export function DamagedArchivesModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const scan = useIntegrityScan()
  const del = useBatchDeleteArchives()
  const recheck = useRecheckArchive()
  const [filter, setFilter] = useState("")
  const [selected, setSelected] = useState<Set<string>>(() => new Set())

  const archives = useMemo(() => scan.data?.archives ?? [], [scan.data])
  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    if (!needle) return archives
    return archives.filter(
      (a) => a.title.toLowerCase().includes(needle) || a.file.toLowerCase().includes(needle),
    )
  }, [archives, filter])
  // A row that left the list (deleted, or passed a re-check) must not stay counted as selected.
  const chosen = shown.filter((a) => selected.has(a.id))

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  async function moveToRecycleBin(items: DamagedArchive[]) {
    if (items.length === 0) return
    const question =
      items.length === 1
        ? t("settings.integrityDeleteConfirm", { title: items[0].title })
        : t("settings.integrityDeleteSelectedConfirm", { count: items.length })
    if (!(await confirmDialog(question, true))) return
    await del.mutateAsync(items.map((a) => a.id))
    setSelected(new Set())
    await scan.refetch()
    toast({ text: t("settings.integrityDeleted") ?? undefined, icon: "success" })
  }

  async function checkAgain(a: DamagedArchive) {
    const result = await recheck.mutateAsync(a.id)
    toast({
      text: (result.damaged ? t("settings.integrityRecheckFailed") : t("settings.integrityRecheckPassed")) ?? undefined,
      icon: result.damaged ? "warning" : "success",
    })
  }

  return (
    <Modal onClose={onClose} width={760} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 8px" }}>
        {t("settings.integrityListTitle", { count: archives.length })}
      </h3>
      <input
        type="search"
        className="stdinput"
        style={{ width: "100%", marginBottom: 8 }}
        placeholder={t("settings.integrityFilterPlaceholder") ?? ""}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />
      {shown.length > 0 && (
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", marginBottom: 8 }}>
          <CheckboxField
            checked={chosen.length === shown.length}
            indeterminate={chosen.length > 0 && chosen.length < shown.length}
            onCheckedChange={(on) => setSelected(on ? new Set(shown.map((a) => a.id)) : new Set())}
          >
            {t("settings.integritySelectAll")}
          </CheckboxField>
          <input
            type="button"
            className="stdbtn"
            style={{ width: "auto" }}
            disabled={chosen.length === 0 || del.isPending}
            value={t("settings.integrityDeleteSelected", { count: chosen.length }) ?? undefined}
            onClick={() => void moveToRecycleBin(chosen)}
          />
        </div>
      )}
      {scan.isPending ? (
        <p>{t("common.loading")}</p>
      ) : shown.length === 0 ? (
        <p style={{ opacity: 0.8 }}>{t("settings.integrityNone")}</p>
      ) : (
        <div style={{ maxHeight: "60vh", overflowY: "auto" }}>
          <table className="itg" style={{ width: "100%" }}>
            <tbody>
              {shown.map((a) => (
                <tr key={a.id} className="gtr1">
                  <td style={{ width: 28, textAlign: "center", verticalAlign: "top", paddingTop: 6 }}>
                    <Checkbox checked={selected.has(a.id)} onCheckedChange={() => toggle(a.id)} aria-label={a.title} />
                  </td>
                  <td style={{ padding: "4px 6px", overflowWrap: "anywhere" }}>
                    <a href={routes.reader(a.id)} target="_blank" rel="noreferrer">
                      {a.title}
                    </a>
                    <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.85 }}>{problemText(t, a)}</div>
                    <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.7, userSelect: "all" }}>{a.file}</div>
                    <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.7 }}>
                      {sizeText(t, a)}
                      {detectedAt(a) && ` · ${t("settings.integrityFirstSeen", { time: detectedAt(a) })}`}
                    </div>
                  </td>
                  <td style={{ whiteSpace: "nowrap", textAlign: "right", verticalAlign: "top" }}>
                    <input
                      type="button"
                      className="stdbtn"
                      value={t("settings.integrityCopyPath") ?? undefined}
                      onClick={() => void copyWithToast(a.file, t("share.result.copied"), t("share.result.copyFailed"))}
                    />{" "}
                    <input
                      type="button"
                      className="stdbtn"
                      disabled={recheck.isPending}
                      value={t("settings.integrityRecheck") ?? undefined}
                      onClick={() => void checkAgain(a)}
                    />{" "}
                    <input
                      type="button"
                      className="stdbtn"
                      disabled={del.isPending}
                      value={t("settings.integrityDelete") ?? undefined}
                      onClick={() => void moveToRecycleBin([a])}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Modal>
  )
}

type TFn = ReturnType<typeof useTranslation>["t"]

function problemText(t: TFn, a: DamagedArchive): string {
  if (!a.finding) return t("settings.integrityProblem.heal_failed")
  const label = t(`settings.integrityProblem.${a.finding.problem}`)
  return a.finding.detail ? t("settings.integrityProblemWithDetail", { label, detail: a.finding.detail }) : label
}

function sizeText(t: TFn, a: DamagedArchive): string {
  const onDisk = a.finding?.size_on_disk
  if (onDisk == null || onDisk === a.arcsize) return formatBytes(a.arcsize)
  return t("settings.integritySizeShrunk", {
    catalogued: formatBytes(a.arcsize),
    onDisk: formatBytes(onDisk),
  })
}

/** When it was first known broken: the self-heal failure, else the scan that found it. */
function detectedAt(a: DamagedArchive): string | null {
  const secs = a.heal_failed_at ?? a.finding?.checked_at
  return secs ? new Date(secs * 1000).toLocaleString() : null
}
