import type { CSSProperties, ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import type { ActivityEntry } from "@/api/types"
import { CodeBlock, TagTable } from "@/components/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

import { PLUGIN_TYPE_LABEL_KEYS } from "./activityTarget"

/** `download_queue.add`/`download_queue.start` plus the unified `archive.ingest` record's own
 * `after` payloads are heterogeneous (a plugin identity, resolved execution parameters, an
 * optional metadata preview, and — for a start that finished — the resulting archive ids), which
 * reads far better as a small structured summary than as the generic raw-JSON code block every
 * other action type falls back to. Everything here is optional: a start entry is enriched
 * progressively by `plugins::start_download` (`patch_after`), so fields genuinely may be absent
 * when the operator opens it mid-download, and "not known yet" is simply omitted rather than
 * rendered as `null`. An `archive.ingest` record additionally carries `after.source`
 * (`"download"` | `"upload"` | `"scanner"`), which decides which of its rows are meaningful. */

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined
}

function readString(record: Record<string, unknown> | null, key: string): string | undefined {
  return record ? nonEmptyString(record[key]) : undefined
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

/** A metadata plugin's `{tags?, title?, summary?}` preview — reuses the Upload page's own
 * `TagTable` so namespaced tags look identical everywhere, and falls back to raw JSON for the
 * uncommon preview that carries none of those three well-known fields. */
function MetadataBody({ metadata }: { metadata: Record<string, unknown> }) {
  const title = readString(metadata, "title")
  const tags = readString(metadata, "tags")
  const summary = readString(metadata, "summary")
  if (!title && !tags && !summary) return <CodeBlock code={JSON.stringify(metadata, null, 2)} />
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: FONT_SIZE_SM }}>
      {title && <div style={{ fontWeight: "bold", wordBreak: "break-word" }}>{title}</div>}
      {tags && <TagTable tags={tags} />}
      {summary && <div style={{ whiteSpace: "pre-wrap", opacity: 0.85 }}>{summary}</div>}
    </div>
  )
}

const SECTION_TITLE_STYLE: CSSProperties = { fontSize: FONT_SIZE_SM, opacity: 0.65, margin: "0 0 6px" }

export function DownloadActivityDetails({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()
  const after = isRecord(entry.after) ? entry.after : {}
  const isIngest = entry.action_type === "archive.ingest"

  const plugin = isRecord(after.plugin) ? after.plugin : null
  const pluginNamespace = readString(after, "plugin_namespace")
  const pluginName = readString(plugin, "name") ?? pluginNamespace
  const pluginType = readString(plugin, "type")
  const pluginTypeLabel =
    pluginType && pluginType in PLUGIN_TYPE_LABEL_KEYS ? t(PLUGIN_TYPE_LABEL_KEYS[pluginType]) : undefined

  const execution = isRecord(after.execution_parameters) ? after.execution_parameters : null
  const pluginParameters = execution?.plugin_parameters
  const category = readString(execution, "category") ?? readString(after, "category")
  const url = readString(after, "url") ?? readString(after, "source_url")
  const metadata = isRecord(after.metadata) ? after.metadata : null

  const rows: ReactNode[] = []
  const addRow = (label: string, value: ReactNode, key?: string) => {
    if (value === null || value === undefined || value === "") return
    rows.push(
      <Row key={key ?? label} label={label}>
        {value}
      </Row>,
    )
  }

  // Every `archive.ingest` record says which of the three ingestion paths produced it
  // ("download" | "upload" | "scanner"); the add/start records below have no source dimension.
  const source = readString(after, "source")
  if (isIngest) {
    addRow(t("activity.ingestSource"), source ? t(`activity.ingestSource.${source}`) : undefined)
  }

  // Plugin identity only meaningfully exists for the download source (or an add/start record).
  if (!isIngest || source === "download") {
    addRow(
      t("activity.downloadPlugin"),
      <>
        {pluginName ?? "—"}
        {pluginTypeLabel && <span style={{ opacity: 0.7 }}> · {pluginTypeLabel}</span>}
        {pluginNamespace && pluginNamespace !== pluginName && (
          <span style={{ opacity: 0.55 }}> ({pluginNamespace})</span>
        )}
      </>,
    )
  }

  const filename = readString(after, "filename") ?? entry.target.label ?? undefined
  if (isIngest) addRow(t("activity.downloadFilename"), filename)

  const path = isIngest && source === "scanner" ? readString(after, "path") : undefined
  if (path && path !== filename) addRow(t("activity.downloadPath"), path)

  if (url) {
    addRow(
      t("activity.downloadSource"),
      <a href={url} target="_blank" rel="noreferrer" style={{ wordBreak: "break-all" }}>
        {url}
      </a>,
    )
  }

  if (category) addRow(t("activity.downloadCategory"), category)

  if (!isIngest) {
    addRow(t("activity.downloadTitle"), readString(after, "title"))
    if (after.auto_fetch_metadata !== undefined) {
      addRow(
        t("activity.downloadAutoFetch"),
        after.auto_fetch_metadata ? t("common.yes") : t("common.no"),
      )
    }
    if (after.overwrite_on_duplicate !== undefined) {
      addRow(
        t("activity.downloadOverwrite"),
        after.overwrite_on_duplicate ? t("common.yes") : t("common.no"),
      )
    }
  }

  if (after.is_new !== undefined && after.is_new !== null) {
    addRow(
      t("activity.downloadIsNew"),
      after.is_new ? t("activity.downloadIsNewYes") : t("activity.downloadIsNewNo"),
    )
  }

  const archiveIds = [
    ...(typeof after.archive_id === "string" ? [after.archive_id] : []),
    ...(Array.isArray(after.archive_ids)
      ? after.archive_ids.filter((value): value is string => typeof value === "string")
      : []),
  ]
  if (archiveIds.length > 0) {
    addRow(
      t("activity.downloadArchives"),
      archiveIds.map((id) => (
        <Link key={id} to={routes.edit(id)} style={{ display: "block" }}>
          {id}
        </Link>
      )),
    )
  }

  const hasPluginParameters =
    pluginParameters !== undefined &&
    (Array.isArray(pluginParameters)
      ? pluginParameters.length > 0
      : isRecord(pluginParameters) && Object.keys(pluginParameters).length > 0)

  return (
    <div style={{ marginTop: 16, display: "flex", flexDirection: "column", gap: 12 }}>
      {rows.length > 0 && (
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
          {rows}
        </dl>
      )}
      {hasPluginParameters && (
        <section>
          <h3 style={SECTION_TITLE_STYLE}>{t("activity.executionParameters")}</h3>
          <CodeBlock code={JSON.stringify(pluginParameters, null, 2)} />
        </section>
      )}
      {metadata && (
        <section>
          <h3 style={SECTION_TITLE_STYLE}>{t("activity.downloadMetadata")}</h3>
          <MetadataBody metadata={metadata} />
        </section>
      )}
    </div>
  )
}
