/** Choosing what goes into a feedback bundle, previewing it, then downloading and copying the text. */

import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { CheckboxField } from "@/components/common-ui/Form"
import {
  DateTimeField,
  formatLocal,
  parseLocal,
} from "@/components/common-ui/Form/DateTimeField"
import { toast } from "@/toast"

import {
  type BundleSelection,
  type BundleText,
  useBundleText,
  useDownloadBundle,
  usePreviewBundle,
} from "./api"
import { useIssueSelection } from "./selection"

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ["KB", "MB", "GB"]
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(1)} ${units[unit]}`
}

/** The picker edits formatted text, so the state and the filter are both `YYYY-MM-DD HH:MM`. */
function toLocalInput(millis: number): string {
  return formatLocal(new Date(millis))
}

/// Quick ranges, since the fields are typed now rather than picked from a calendar.
const RANGE_PRESETS = [
  { id: "hour", span: 3_600_000, labelKey: "diagnostics.bundle.lastHour" },
  { id: "day", span: 86_400_000, labelKey: "diagnostics.bundle.lastDay" },
  { id: "week", span: 604_800_000, labelKey: "diagnostics.bundle.lastWeek" },
] as const

export function BundleView() {
  const { t } = useTranslation()
  // Defaults are computed once, at mount: reading the clock during render is not pure.
  const [from, setFrom] = useState(() => toLocalInput(Date.now() - 3_600_000))
  const [to, setTo] = useState(() => toLocalInput(Date.now()))
  const [levels, setLevels] = useState<string[]>(["error", "warning"])
  const [categories, setCategories] = useState({
    server_logs: true,
    frontend_events: true,
    environment: true,
    config: false,
  })
  const [truncate, setTruncate] = useState(false)
  const [language, setLanguage] = useState<"zh" | "en">("zh")
  const [title, setTitle] = useState<string | null>(null)
  const [rendered, setRendered] = useState<BundleText | null>(null)

  const preview = usePreviewBundle()
  const text = useBundleText()
  const download = useDownloadBundle()

  const picked = useIssueSelection((state) => state.selected)
  // The fields are local time; the exported text and the server's own log lines are UTC. Saying so
  // once here is cheaper than a user comparing 10:19 against 01:19 and assuming a bug.
  const localZone = Intl.DateTimeFormat().resolvedOptions().timeZone
  // `null` means "no explicit choice yet": arriving with a selection defaults the switch on, and
  // an empty selection means there is nothing to restrict to.
  const [onlyPicked, setOnlyPicked] = useState<boolean | null>(null)
  const restrictToPicked = picked.size > 0 && (onlyPicked ?? true)
  const selection = useMemo<BundleSelection>(
    () => ({
      from: parseLocal(from)?.getTime() ?? 0,
      to: parseLocal(to)?.getTime() ?? 0,
      levels,
      sources: [],
      issues: restrictToPicked ? [...picked] : [],
      include_ids: [],
      exclude_ids: [],
      categories,
      truncate,
      resolve_stacks: true,
    }),
    [from, to, levels, categories, truncate, restrictToPicked, picked],
  )

  const onError = (error: unknown) =>
    toast({ icon: "error", text: error instanceof Error ? error.message : String(error) })

  const toggleLevel = (level: string) =>
    setLevels((current) =>
      current.includes(level) ? current.filter((value) => value !== level) : [...current, level],
    )

  const generateText = () =>
    text.mutate(selection, {
      onSuccess: (value) => {
        setRendered(value)
        setTitle(value[language].title)
      },
      onError,
    })

  const copy = () => {
    if (!rendered) return
    const body = `${title ?? rendered[language].title}\n\n${rendered[language].body}`
    void navigator.clipboard.writeText(body).then(
      () => toast({ icon: "success", text: t("diagnostics.bundle.copied") }),
      () => toast({ icon: "error", text: t("diagnostics.bundle.copyFailed") }),
    )
  }

  return (
    <div className="diag-bundle">
      <div className="diag-inline">
        <DateTimeField
          label={t("diagnostics.bundle.from")}
          value={from}
          onChange={setFrom}
        />
        <DateTimeField label={t("diagnostics.bundle.to")} value={to} onChange={setTo} />
      </div>
      <p className="diag-note">{t("diagnostics.bundle.timezone", { zone: localZone })}</p>
      <div className="diag-range-presets">
        {RANGE_PRESETS.map((preset) => (
          <button
            key={preset.id}
            type="button"
            className="diag-chip"
            onClick={() => {
              const now = Date.now()
              setFrom(toLocalInput(now - preset.span))
              setTo(toLocalInput(now))
            }}
          >
            {t(preset.labelKey)}
          </button>
        ))}
      </div>
      <div className="diag-chips">
        {(["error", "warning", "info"] as const).map((level) => (
          <button
            key={level}
            type="button"
            className={`diag-chip diag-level-${level}${
              levels.includes(level) ? " is-on" : ""
            }`}
            onClick={() => toggleLevel(level)}
          >
            {t(`diagnostics.level.${level}`)}
          </button>
        ))}
      </div>

      {picked.size > 0 && (
        <div className="diag-bundle-picked">
          <CheckboxField checked={restrictToPicked} onCheckedChange={setOnlyPicked}>
            {t("diagnostics.bundle.onlyPicked", { count: picked.size })}
          </CheckboxField>
        </div>
      )}
      <div className="diag-inline diag-categories">
        <CheckboxField
          checked={categories.server_logs}
          onCheckedChange={(checked) =>
            setCategories((current) => ({ ...current, server_logs: checked }))
          }
        >
          {t("diagnostics.bundle.serverLogs")}
        </CheckboxField>
        <CheckboxField
          checked={categories.frontend_events}
          onCheckedChange={(checked) =>
            setCategories((current) => ({ ...current, frontend_events: checked }))
          }
        >
          {t("diagnostics.bundle.frontendEvents")}
        </CheckboxField>
        <CheckboxField
          checked={categories.environment}
          onCheckedChange={(checked) =>
            setCategories((current) => ({ ...current, environment: checked }))
          }
        >
          {t("diagnostics.bundle.environment")}
        </CheckboxField>
        <CheckboxField
          checked={categories.config}
          onCheckedChange={(checked) =>
            setCategories((current) => ({ ...current, config: checked }))
          }
        >
          {t("diagnostics.bundle.config")}
        </CheckboxField>
        <CheckboxField checked={truncate} onCheckedChange={setTruncate}>
          {t("diagnostics.bundle.truncate")}
        </CheckboxField>
      </div>

      <div className="diag-actions">
        <button
          type="button"
          className="stdbtn"
          disabled={preview.isPending}
          onClick={() => preview.mutate(selection, { onError })}
        >
          {t("diagnostics.bundle.preview")}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={download.isPending}
          onClick={() =>
            download.mutate(selection, {
              onSuccess: ({ blob, filename }) => {
                const url = URL.createObjectURL(blob)
                const anchor = document.createElement("a")
                anchor.href = url
                anchor.download = filename ?? "lrrg-diagnostics.zip"
                anchor.click()
                URL.revokeObjectURL(url)
              },
              onError,
            })
          }
        >
          {t("diagnostics.bundle.download")}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={text.isPending}
          onClick={generateText}
        >
          {t("diagnostics.bundle.generateText")}
        </button>
      </div>

      {preview.data && (
        <div className="diag-preview">
          <div className="diag-preview-head">
            {t("diagnostics.bundle.previewHead", {
              size: formatBytes(preview.data.total_bytes),
              events: preview.data.events,
              lines: preview.data.log_lines,
            })}
            {preview.data.over_cap && (
              <span className="diag-over-cap">
                {t("diagnostics.bundle.overCap", {
                  cap: formatBytes(preview.data.cap_bytes),
                })}
              </span>
            )}
          </div>
          <ul className="diag-preview-files">
            {preview.data.files.map((file) => (
              <li key={file.path}>
                <code>{file.path}</code>
                <span>{formatBytes(file.bytes)}</span>
              </li>
            ))}
          </ul>
          {preview.data.truncations.length > 0 && (
            <ul className="diag-preview-truncations">
              {preview.data.truncations.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      {rendered && (
        <div className="diag-text">
          <div className="diag-inline">
            {(["zh", "en"] as const).map((lang) => (
              <button
                key={lang}
                type="button"
                className={`stdbtn${language === lang ? " is-on" : ""}`}
                onClick={() => {
                  setLanguage(lang)
                  setTitle(rendered[lang].title)
                }}
              >
                {lang === "zh" ? "中文" : "English"}
              </button>
            ))}
            <button type="button" className="stdbtn" onClick={copy}>
              {t("diagnostics.bundle.copy")}
            </button>
          </div>
          <input
            className="stdinput"
            value={title ?? rendered[language].title}
            onChange={(event) => setTitle(event.target.value)}
          />
          <textarea className="stdinput diag-text-body" readOnly value={rendered[language].body} />
        </div>
      )}
    </div>
  )
}
