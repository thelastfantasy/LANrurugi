import { useTranslation } from "react-i18next"

import { type StatsBucketSelection,useStatsBucket } from "@/api/hooks"
import { FormatBadge } from "@/components/Display"
import { routes } from "@/lib/routes"

import { useChartPalette } from "./chartPalette"
import { useStatsPanel } from "./Charts"
import { formatBytes, formatTimestamp } from "./format"

/** How tall the archive list may get before it scrolls. Fixed rather than content-sized: a
 *  200-archive band would otherwise push the rest of the section several screens down, and the
 *  point of clicking a bar is to look *at* the section, not to lose it. */
const LIST_MAX_HEIGHT = 240

/** The archives behind one clicked bar, listed directly under the chart that was clicked.
 *
 *  A bucket with no archives gets a single muted line rather than the boxed list: an empty list box
 *  would be dead space (the chart already says the bucket is empty), but rendering nothing at all
 *  makes a click look like it missed — which is exactly what a reader concludes when they click a
 *  small bar and the page does not react.
 */
export function BucketArchivesPanel({
  selection,
  title,
  subtitle,
  onClose,
}: {
  selection: StatsBucketSelection | null
  /** What this bucket is, in the reader's own terms — the chart already labels it, but the list is
   *  read on its own once scrolled to, so it repeats which bar it belongs to. */
  title: string
  subtitle?: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { palette, muted } = useStatsPanel()
  const series = useChartPalette()
  const { data, isPending } = useStatsBucket(selection)

  if (!selection) return null
  if (!data) {
    // Every click starts from no data for that bucket (see the hook's own note on why the previous
    // bucket's list is not held over).
    return isPending ? (
      <p style={{ ...muted, fontSize: "0.75rem", margin: "8px 0 12px" }}>{t("common.loading")}</p>
    ) : null
  }
  if (data.archives.length === 0) {
    return (
      <p
        style={{
          ...muted,
          fontSize: "0.75rem",
          margin: "4px 0 12px",
          display: "flex",
          alignItems: "center",
          gap: 6,
        }}
      >
        <i className="fa fa-info-circle" />
        {t("stats.bucketArchivesEmpty")}
        <button
          type="button"
          onClick={onClose}
          title={t("common.close")}
          aria-label={t("common.close")}
          style={{ background: "transparent", border: "none", color: "inherit", cursor: "pointer", opacity: 0.7 }}
        >
          <i className="fa fa-times" />
        </button>
      </p>
    )
  }

  return (
    <div
      style={{
        border: `1px solid ${palette.separator}`,
        borderRadius: 4,
        padding: "8px 10px",
        margin: "4px 0 12px",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
          gap: 8,
          marginBottom: 6,
        }}
      >
        <span style={{ fontSize: "0.78rem" }}>
          <strong>{title}</strong>
          {subtitle ? <span style={{ ...muted, marginLeft: 6 }}>{subtitle}</span> : null}
        </span>
        <span style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
          <span style={{ ...muted, fontSize: "0.72rem" }}>
            {t("stats.bucketArchivesTotal", { count: data.total })}
            {data.truncated
              ? ` · ${t("stats.bucketArchivesTruncated", { count: data.archives.length })}`
              : ""}
          </span>
          <button
            type="button"
            onClick={onClose}
            title={t("common.close")}
            aria-label={t("common.close")}
            style={{ background: "transparent", border: "none", color: "inherit", cursor: "pointer", opacity: 0.7 }}
          >
            <i className="fa fa-times" />
          </button>
        </span>
      </div>
      <div style={{ maxHeight: LIST_MAX_HEIGHT, overflowY: "auto" }}>
        {data.archives.map((archive, index) => (
          <div
            key={archive.id}
            style={{
              display: "flex",
              alignItems: "baseline",
              justifyContent: "space-between",
              gap: 10,
              fontSize: "0.76rem",
              padding: "2px 0",
              borderTop: index === 0 ? "none" : `1px solid ${palette.separator}`,
            }}
          >
            <a
              href={routes.reader(archive.id)}
              title={archive.title}
              style={{
                minWidth: 0,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
                color: series[1],
              }}
            >
              <i className="fa fa-book" style={{ marginRight: 6, opacity: 0.6 }} />
              {archive.title} <FormatBadge extension={archive.extension} />
            </a>
            <span style={{ ...muted, whiteSpace: "nowrap" }}>
              {formatBytes(archive.bytes)} · {t("stats.pages")} {archive.pages} ·{" "}
              {formatTimestamp(archive.timestamp)}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
