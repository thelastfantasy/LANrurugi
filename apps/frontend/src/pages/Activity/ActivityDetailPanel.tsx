import { Fragment } from "react"
import { useTranslation } from "react-i18next"

import { useApiTokens } from "@/api/hooks"
import type {
  ActivityClientReportedInfo,
  ActivityEntry,
  ActivityGeoInfo,
  ActivityUserAgentInfo,
} from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { CodeBlock, IpGeoLink } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { actorChipParts } from "./activityActor"
import { ActivityChip } from "./ActivityChip"
import { outcomeColor } from "./activityColors"
import { actionTypeLabel, outcomeLabel } from "./activityTarget"
import { DownloadActivityDetails } from "./DownloadActivityDetails"
import { LlmCallActivityDetails } from "./LlmCallActivityDetails"
import { MetadataDiff, type MetadataUpdateAfter, type MetadataUpdateBefore } from "./MetadataDiff"
import { OperationDescription } from "./OperationDescription"

/** `archive.metadata_update` gets the GitHub-commit-style word/tag diff (`MetadataDiff`) instead
 * of the generic raw-JSON `before`/`after` code blocks every other action type falls back to. */
const METADATA_DIFF_ACTION_TYPES = new Set(["archive.metadata_update"])

/** The download pipeline's own action types (add-to-queue, start) plus the unified
 * `archive.ingest` catalog record get a structured summary — plugin identity, resolved execution
 * parameters, metadata preview, ingestion source, result — instead of raw JSON; see
 * `DownloadActivityDetails`'s own docs. */
const DOWNLOAD_DETAIL_ACTION_TYPES = new Set([
  "download_queue.add",
  "download_queue.start",
  "archive.ingest",
])

/** `translation.llm_call` gets a labeled provider/model/token/cost summary (`LlmCallActivityDetails`)
 * instead of the generic raw-JSON `after` code block. */
const LLM_CALL_DETAIL_ACTION_TYPES = new Set(["translation.llm_call"])

/** "smartphone · iOS 17.0 · Safari 17.0" — `woothee`'s own `"UNKNOWN"` placeholder (whichever
 * field it couldn't classify) is dropped rather than shown verbatim, since a literal "UNKNOWN"
 * reads as an error to a non-technical admin rather than "this part just wasn't determinable". */
function userAgentSummary(info: ActivityUserAgentInfo): string {
  const os = [info.os, info.os_version].filter((p) => p && p !== "UNKNOWN").join(" ")
  const browser = [info.browser, info.browser_version].filter((p) => p && p !== "UNKNOWN").join(" ")
  return [info.category, os, browser].filter((part) => part && part !== "UNKNOWN").join(" · ")
}

/** The most precise browser-version signal available, preferring Client Hints' full version list
 * (Chromium, e.g. "Chrome 120.0.6099.129") over the plain `User-Agent`-parsed version
 * (`userAgentSummary`'s own `browser`/`browser_version`, all that's available on Firefox/Safari). */
function preciseBrowserVersion(
  userAgent: ActivityUserAgentInfo | null,
  clientReported: ActivityClientReportedInfo | null,
): string | null {
  return clientReported?.uach_full_version_list ?? clientReported?.uach_brands ?? (userAgent && userAgent.browser_version !== "UNKNOWN" ? `${userAgent.browser} ${userAgent.browser_version}` : null)
}

/** "Tokyo, Tokyo, Japan (JP)" — most-specific to least-specific, matching how a postal address is
 * read; any missing part (no city-level coverage for this IP, etc.) is simply skipped rather than
 * leaving a stray comma. */
function geoSummary(info: ActivityGeoInfo): string {
  const parts = [info.city_name, info.subdivision_name, info.country_name].filter(Boolean)
  const label = parts.join(", ")
  return info.country_code ? `${label} (${info.country_code})` : label
}

/** Every client-reported fact worth a label, in the same fixed order every entry shows them in —
 * `undefined`/`null` fields are simply omitted rather than shown as "—", so an entry from a
 * browser that withheld a Chromium-only field (`deviceMemory`, Client Hints, Network Information)
 * doesn't clutter the panel with rows that carry no information. */
function clientReportedRows(
  t: (key: string) => string,
  info: ActivityClientReportedInfo,
  userAgent: ActivityUserAgentInfo | null,
): { label: string; value: string }[] {
  const rows: { label: string; value: string }[] = []
  const push = (labelKey: string, value: string | number | boolean | null | undefined) => {
    if (value === null || value === undefined || value === "") return
    rows.push({ label: t(labelKey), value: String(value) })
  }
  const yesNo = (v: boolean | null | undefined) => (v === null || v === undefined ? null : v ? t("common.yes") : t("common.no"))

  const preciseVersion = preciseBrowserVersion(userAgent, info)
  push("activity.browserVersion", preciseVersion)
  push("activity.osVersion", info.uach_platform && info.uach_platform_version ? `${info.uach_platform} ${info.uach_platform_version}` : null)

  if (info.screen_width && info.screen_height) {
    push(
      "activity.screenResolution",
      `${info.screen_width} × ${info.screen_height}${info.device_pixel_ratio ? ` @${info.device_pixel_ratio}x` : ""}`,
    )
  }
  if (info.screen_avail_width && info.screen_avail_height) {
    push("activity.screenAvailArea", `${info.screen_avail_width} × ${info.screen_avail_height}`)
  }
  if (info.window_outer_width && info.window_outer_height) {
    push("activity.windowOuterSize", `${info.window_outer_width} × ${info.window_outer_height}`)
  }
  if (info.window_inner_width && info.window_inner_height) {
    push("activity.windowInnerSize", `${info.window_inner_width} × ${info.window_inner_height}`)
  }
  push("activity.colorDepth", info.color_depth ? `${info.color_depth}-bit` : null)
  push("activity.pixelDepth", info.pixel_depth ? `${info.pixel_depth}-bit` : null)
  push("activity.screenOrientation", info.screen_orientation)
  push("activity.language", info.languages ?? info.language)
  push("activity.timezone", info.timezone_offset_minutes !== null && info.timezone_offset_minutes !== undefined ? `${info.timezone} (UTC${info.timezone_offset_minutes >= 0 ? "+" : ""}${info.timezone_offset_minutes / 60})` : info.timezone)
  push("activity.platform", info.uach_platform ?? info.platform)
  push("activity.hardwareConcurrency", info.hardware_concurrency)
  push("activity.deviceMemory", info.device_memory_gib ? `${info.device_memory_gib} GiB` : null)
  push("activity.touchSupport", yesNo(info.touch_support))
  push("activity.maxTouchPoints", info.max_touch_points)
  push(
    "activity.connectionType",
    [info.connection_type, info.connection_downlink_mbps ? `${info.connection_downlink_mbps} Mbps` : null, info.connection_rtt_ms ? `${info.connection_rtt_ms} ms RTT` : null]
      .filter(Boolean)
      .join(", "),
  )
  push("activity.dataSaver", yesNo(info.connection_save_data))
  push("activity.cookieEnabled", yesNo(info.cookie_enabled))
  push("activity.pdfViewerEnabled", yesNo(info.pdf_viewer_enabled))
  push("activity.prefersDarkColorScheme", yesNo(info.prefers_dark_color_scheme))
  push("activity.prefersReducedMotion", yesNo(info.prefers_reduced_motion))
  push("activity.probablyIncognito", yesNo(info.probably_incognito))
  return rows
}

/** Full detail view for one `ActivityEntry` — before/after diff, causal chain, and every field
 * the row itself only has room to summarize. `onDelete` is optional. */
export function ActivityDetailPanel({
  entry,
  onClose,
  onDelete,
}: {
  entry: ActivityEntry
  onClose: () => void
  onDelete?: () => void
}) {
  const { t } = useTranslation()
  const apiTokens = useApiTokens()
  const token = entry.actor.kind === "token" && entry.actor.id ? apiTokens.data?.find((tk) => tk.id === entry.actor.id) : undefined
  const { label: actorLabel, color: actorColor, tooltip: actorTooltip } = actorChipParts(t, entry, token)
  const showMetadataDiff = METADATA_DIFF_ACTION_TYPES.has(entry.action_type) && entry.after != null
  const showDownloadDetails = DOWNLOAD_DETAIL_ACTION_TYPES.has(entry.action_type) && entry.after != null
  const showLlmCallDetails = LLM_CALL_DETAIL_ACTION_TYPES.has(entry.action_type) && entry.after != null

  return (
    <Modal onClose={onClose} width={640} textAlign="left">
      <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 12 }}>
        <h2 style={{ margin: 0 }}>{actionTypeLabel(t, entry.action_type)}</h2>
        {onDelete && (
          <button type="button" className="stdbtn stdbtn-danger" style={{ minWidth: 0, width: "auto", padding: "0 12px" }} onClick={onDelete}>
            {t("common.delete")}
          </button>
        )}
      </div>
      <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, marginBottom: 12 }}>
        {new Date(entry.timestamp * 1000).toLocaleString()}
      </div>

      {/* alignItems: baseline (not grid's default stretch) lines up dt/dd text baselines; every dd
          gets margin: 0 to override the UA's 40px inline-start default. */}
      <dl
        style={{
          display: "grid",
          gridTemplateColumns: "auto 1fr",
          alignItems: "baseline",
          columnGap: 12,
          rowGap: 6,
          fontSize: FONT_SIZE_SM,
        }}
      >
        <dt style={{ opacity: 0.65 }}>{t("activity.outcome")}</dt>
        <dd style={{ margin: 0 }}>
          <ActivityChip color={outcomeColor(entry.outcome.status)}>{outcomeLabel(t, entry.outcome.status)}</ActivityChip>
        </dd>

        <dt style={{ opacity: 0.65 }}>{t("activity.actor")}</dt>
        <dd style={{ margin: 0 }}>
          <ActivityChip color={actorColor}>
            {actorTooltip ? (
              <Tooltip label={actorTooltip} wrapperStyle={{ alignItems: "center" }}>
                {actorLabel}
              </Tooltip>
            ) : (
              actorLabel
            )}
          </ActivityChip>
        </dd>

        <dt style={{ opacity: 0.65 }}>{t("activity.operationContent")}</dt>
        <dd style={{ margin: 0 }}>
          <OperationDescription entry={entry} />
        </dd>

        {entry.outcome.status === "failure" && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.failureReason")}</dt>
            <dd style={{ margin: 0, color: outcomeColor("failure").bg }}>{entry.outcome.reason}</dd>
          </>
        )}

        {entry.actor.kind === "session" && entry.device_name && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.deviceName")}</dt>
            <dd style={{ margin: 0 }}>{entry.device_name}</dd>
          </>
        )}

        {entry.client_ip && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.ipAddress")}</dt>
            <dd style={{ margin: 0 }}>
              <IpGeoLink ip={entry.client_ip} />
              {entry.device_info?.geo && (
                <span style={{ opacity: 0.75 }}> — {geoSummary(entry.device_info.geo)}</span>
              )}
            </dd>
          </>
        )}

        {entry.device_info?.user_agent && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.deviceInfo")}</dt>
            <dd style={{ margin: 0 }}>{userAgentSummary(entry.device_info.user_agent)}</dd>
          </>
        )}

        {entry.device_info?.client_reported &&
          clientReportedRows(t, entry.device_info.client_reported, entry.device_info.user_agent ?? null).map((row) => (
            <Fragment key={row.label}>
              <dt style={{ opacity: 0.65 }}>{row.label}</dt>
              <dd style={{ margin: 0 }}>{row.value}</dd>
            </Fragment>
          ))}

        {entry.caused_by && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.causedBy")}</dt>
            <dd style={{ margin: 0 }}>{entry.caused_by.description}</dd>
          </>
        )}
      </dl>

      {showDownloadDetails ? (
        <DownloadActivityDetails entry={entry} />
      ) : showLlmCallDetails ? (
        <LlmCallActivityDetails entry={entry} />
      ) : showMetadataDiff ? (
        <div style={{ marginTop: 16 }}>
          <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.changes")}</h3>
          <MetadataDiff
            before={(entry.before ?? {}) as MetadataUpdateBefore}
            after={entry.after as MetadataUpdateAfter}
          />
        </div>
      ) : (
        <>
          {entry.before != null && (
            <div style={{ marginTop: 16 }}>
              <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.before")}</h3>
              <CodeBlock code={JSON.stringify(entry.before, null, 2)} language="json" />
            </div>
          )}
          {entry.after != null && (
            <div style={{ marginTop: 16 }}>
              <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.after")}</h3>
              <CodeBlock code={JSON.stringify(entry.after, null, 2)} language="json" />
            </div>
          )}
        </>
      )}
    </Modal>
  )
}
