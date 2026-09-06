import { useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useArchiveSplitSuggestion,
  useArchiveSplitTree,
  useExecuteArchiveSplit,
  useSettings,
  useTankoubonFull,
} from "@/api/hooks"
import type { ArchiveMetadata } from "@/api/types"
import { Modal } from "@/components/common-ui/Display"
import { EntryTreePopover } from "@/pages/Upload/EntryTreePopover"

function TankMemberSplitRow({
  member,
  selected,
  onToggle,
}: {
  member: ArchiveMetadata
  selected: boolean
  onToggle: () => void
}) {
  const { t } = useTranslation()
  const suggestion = useArchiveSplitSuggestion(member.arcid)
  const tree = useArchiveSplitTree(member.arcid)

  return (
    <div style={{ border: "1px solid rgba(128,128,128,0.4)", borderRadius: 6, padding: 10, marginBottom: 8 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
          <input type="checkbox" checked={selected} onChange={onToggle} />
          <strong>{member.title}</strong>
        </label>
        <EntryTreePopover entries={tree.data?.entries ?? []} />
      </div>
      {suggestion.data?.suggestion ? (
        <table className="itg" style={{ width: "100%", marginTop: 6, tableLayout: "auto" }}>
          <thead>
            <tr>
              <th style={{ padding: "4px 6px", textAlign: "left" }}>{t("reader.splitOutputFile")}</th>
              <th style={{ padding: "4px 6px", textAlign: "left" }}>{t("reader.splitDescription")}</th>
            </tr>
          </thead>
          <tbody>
            {suggestion.data.suggestion.split_groups.map((g) => (
              <tr key={g.zip_name}>
                <td style={{ padding: "4px 6px", textAlign: "left" }}>{g.zip_name}</td>
                <td style={{ padding: "4px 6px", textAlign: "left" }}>{g.description}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div style={{ marginTop: 6, opacity: 0.7 }}>
          {t("reader.noArchiveSplitSuggestion")}
        </div>
      )}
    </div>
  )
}

export function TankoubonSplitModal({
  tankId,
  onClose,
}: {
  tankId: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  const settings = useSettings()
  const full = useTankoubonFull(tankId)
  const executeSplit = useExecuteArchiveSplit()
  const [selected, setSelected] = useState<Set<string> | null>(null)
  const [deleteOriginal, setDeleteOriginal] = useState(
    settings.data?.archive_split_delete_original_enabled ?? false,
  )
  const [status, setStatus] = useState("")

  const candidates = (full.data?.result.full_data ?? []).filter(
    (m) => m.has_split_suggestion,
  )
  const effectiveSelected = selected ?? new Set(candidates.map((c) => c.arcid))

  async function runSelected() {
    const list = candidates.filter((c) => effectiveSelected.has(c.arcid))
    if (list.length === 0) return
    setStatus(t("reader.splitBatchStarting") ?? "")
    for (const member of list) {
      setStatus(`${t("reader.splitBatchRunning")}: ${member.title}`)
      try {
        await executeSplit.mutateAsync({ id: member.arcid, deleteOriginal })
      } catch (e) {
        setStatus(`${member.title}: ${String(e)}`)
        return
      }
    }
    setStatus(t("reader.splitBatchDone") ?? "")
  }

  return (
    <Modal onClose={onClose} width={760} textAlign="left">
      <h2 style={{ textAlign: "center" }}>{t("reader.archiveSplit")}</h2>
      <div style={{ marginBottom: 8 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
          <input
            type="checkbox"
            checked={deleteOriginal}
            onChange={(e) => setDeleteOriginal(e.target.checked)}
          />
          {t("reader.splitDeleteOriginalCheckbox")}
        </label>
        {deleteOriginal && (
          <div style={{ marginTop: 4, color: "red", fontWeight: 600 }}>
            {t("reader.splitDeleteWarning")}
          </div>
        )}
      </div>
      <div style={{ marginBottom: 8, fontWeight: 600 }}>
        {t("reader.tankSplitSelectArchives")}
      </div>
      {candidates.length === 0 ? (
        <div>{t("reader.tankSplitNoSuggestions")}</div>
      ) : (
        candidates.map((m) => (
          <TankMemberSplitRow
            key={m.arcid}
            member={m}
            selected={effectiveSelected.has(m.arcid)}
            onToggle={() => {
              setSelected((prev) => {
                const next = new Set(prev)
                if (next.has(m.arcid)) next.delete(m.arcid)
                else next.add(m.arcid)
                return next
              })
            }}
          />
        ))
      )}
      <div style={{ display: "flex", justifyContent: "center", gap: 8, marginTop: 12 }}>
        <input
          className="stdbtn"
          type="button"
          value={(executeSplit.isPending ? t("reader.running") : t("reader.executeSplit")) ?? undefined}
          disabled={executeSplit.isPending || effectiveSelected.size === 0}
          onClick={() => void runSelected()}
        />
        <input
          className="stdbtn"
          type="button"
          value={t("common.cancel") ?? undefined}
          onClick={onClose}
        />
      </div>
      {status && <div style={{ marginTop: 8, textAlign: "center" }}>{status}</div>}
    </Modal>
  )
}
