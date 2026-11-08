import type { CSSProperties, ReactNode } from "react"
import { useTranslation } from "react-i18next"

import type { ActivityEntry } from "@/api/types"
import { formatBytes } from "@/lib/formatBytes"
import { FONT_SIZE_SM } from "@/theme"

/** `share.send`/`share.receive`'s `after`: what moved, how it was reached, and what the pieces
 * did. Everything here is measured at record time from the kernel's socket-to-interface mapping
 * and the importer's own piece accounting — so the "through VPN" answer is what actually carried
 * the bytes, not what the settings hoped would. */

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined
}

function num(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined
}

function percent(value: unknown): string | undefined {
  const n = num(value)
  return n === undefined ? undefined : `${(n * 100).toFixed(1)}%`
}

const LABEL_STYLE: CSSProperties = { opacity: 0.65, whiteSpace: "nowrap" }
const VALUE_STYLE: CSSProperties = { margin: 0, wordBreak: "break-word" }

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt style={LABEL_STYLE}>{label}</dt>
      <dd style={VALUE_STYLE}>{children}</dd>
    </>
  )
}

function Grid({ children }: { children: ReactNode }) {
  return (
    <dl
      style={{
        display: "grid",
        gridTemplateColumns: "auto 1fr",
        alignItems: "baseline",
        columnGap: 12,
        rowGap: 6,
        fontSize: FONT_SIZE_SM,
        margin: 0,
      }}
    >
      {children}
    </dl>
  )
}

const SECTION_TITLE_STYLE: CSSProperties = { fontSize: FONT_SIZE_SM, opacity: 0.65, margin: "0 0 6px" }

export function ShareTransferDetails({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()
  const after = isRecord(entry.after) ? entry.after : {}
  const bytes = num(after.bytes)
  const durationMs = num(after.duration_ms)
  const route = text(after.route)
  const technique = text(after.technique)
  const interfaceName = text(after.interface)
  const vpn = after.vpn === true
  const local = text(after.local)
  const remote = text(after.remote)

  const shareRatio = isRecord(after.share_ratio) ? after.share_ratio : null
  const importRatio = isRecord(after.import_ratio) ? after.import_ratio : null
  const pieces = isRecord(after.pieces) ? after.pieces : null
  const files = Array.isArray(after.files) ? after.files.filter(isRecord) : []
  const failedOffsets = pieces && Array.isArray(pieces.failed_offsets) ? pieces.failed_offsets : []

  return (
    <div style={{ marginTop: 16, display: "flex", flexDirection: "column", gap: 16 }}>
      <div>
        <h3 style={SECTION_TITLE_STYLE}>{t("activity.share.details")}</h3>
        <Grid>
          {bytes !== undefined && <Row label={t("activity.share.bytes")}>{formatBytes(bytes)}</Row>}
          {durationMs !== undefined && (
            <Row label={t("activity.share.duration")}>
              {durationMs < 1000 ? `${durationMs} ms` : `${(durationMs / 1000).toFixed(1)} s`}
            </Row>
          )}
          {route && (
            <Row label={t("activity.share.route")}>
              {route === "relay"
                ? t("activity.share.relay")
                : route === "direct"
                  ? t("activity.share.direct")
                  : route}
            </Row>
          )}
          {technique && <Row label={t("activity.share.technique")}>{technique}</Row>}
          {interfaceName && <Row label={t("activity.share.interface")}>{interfaceName}</Row>}
          {interfaceName && (
            <Row label={t("activity.share.vpn")}>
              {vpn ? t("activity.share.yes") : t("activity.share.no")}
            </Row>
          )}
          {local && <Row label={t("activity.share.local")}>{local}</Row>}
          {remote && <Row label={t("activity.share.remote")}>{remote}</Row>}
        </Grid>
      </div>

      {(shareRatio || importRatio) && (
        <div>
          <h3 style={SECTION_TITLE_STYLE}>
            {shareRatio ? t("activity.share.shareRatio") : t("activity.share.importRatio")}
          </h3>
          <Grid>
            <Row label={t("activity.share.ratioValue")}>
              {percent((shareRatio ?? importRatio)?.ratio)}
            </Row>
            <Row label={t("activity.share.ratioBytes")}>
              {formatBytes(num((shareRatio ?? importRatio)?.imported ?? (shareRatio ?? importRatio)?.bytes) ?? 0)}
              {" / "}
              {formatBytes(num((shareRatio ?? importRatio)?.total) ?? 0)}
            </Row>
          </Grid>
        </div>
      )}

      {pieces && (
        <div>
          <h3 style={SECTION_TITLE_STYLE}>{t("activity.share.pieces")}</h3>
          <Grid>
            <Row label={t("activity.share.pieceBytes")}>
              {formatBytes(num(pieces.piece_bytes) ?? 0)}
            </Row>
            <Row label={t("activity.share.piecesVerified")}>{String(pieces.verified ?? 0)}</Row>
            <Row label={t("activity.share.piecesRetried")}>{String(pieces.retried ?? 0)}</Row>
            <Row label={t("activity.share.piecesFailed")}>{String(pieces.failed ?? 0)}</Row>
            {failedOffsets.length > 0 && (
              <Row label={t("activity.share.failedOffsets")}>
                {failedOffsets.map((offset) => String(offset)).join(", ")}
              </Row>
            )}
          </Grid>
        </div>
      )}

      {files.length > 0 && (
        <div>
          <h3 style={SECTION_TITLE_STYLE}>{t("activity.share.files")}</h3>
          <table style={{ width: "100%", borderCollapse: "collapse", fontSize: FONT_SIZE_SM }}>
            <thead>
              <tr style={{ opacity: 0.65 }}>
                <th style={{ textAlign: "left", padding: "2px 6px" }}>{t("activity.share.file")}</th>
                <th style={{ textAlign: "right", padding: "2px 6px" }}>{t("activity.share.size")}</th>
                <th style={{ textAlign: "right", padding: "2px 6px" }}>{t("activity.share.ratioValue")}</th>
              </tr>
            </thead>
            <tbody>
              {files.map((file, index) => (
                <tr key={`${String(file.title)}-${index}`}>
                  <td style={{ padding: "2px 6px", wordBreak: "break-word" }}>{String(file.title ?? "")}</td>
                  <td style={{ padding: "2px 6px", textAlign: "right", whiteSpace: "nowrap" }}>
                    {formatBytes(num(file.size) ?? 0)}
                  </td>
                  <td style={{ padding: "2px 6px", textAlign: "right", whiteSpace: "nowrap" }}>
                    {percent(file.ratio)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
