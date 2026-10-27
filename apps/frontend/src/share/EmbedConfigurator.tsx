import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { Tooltip } from "@/components/common-ui/Display"
import { NumberInput, RadioGroup, RadioItem, Select } from "@/components/common-ui/Form"
import { CARD_THEMES, THEME_COLORS, type ThemeColor } from "@/embed/theme"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"

import { copyWithToast } from "./clipboard"

type Kind = "iframe" | "element"
/** `instance`: no theme given, so the card follows the library's; `custom`: colours over a base. */
type ThemeMode = "instance" | "custom" | (typeof CARD_THEMES)[number]
type Lang = "auto" | "zh" | "en" | "ja"

const ABOVE_MODAL = Z_OVERLAY_ABOVE_LEGACY_MODAL + 100

/** Frame sizes offered before "manual". The card caps at 480px wide itself, so the presets stop
 * there; the heights are the card's own height plus a little (a 6-item card is ~370px tall). */
const SIZE_PRESETS = [
  { width: 260, height: 320 },
  { width: 320, height: 380 },
  { width: 360, height: 420 },
  { width: 480, height: 420 },
  { width: 480, height: 560 },
] as const

const MANUAL = "manual"
/** The frame fills its container instead of a fixed size. */
const FILL = "fill"

/** The preset a size corresponds to, or `manual` for anything else. */
function presetKey(width: number, height: number): string {
  const found = SIZE_PRESETS.find((p) => p.width === width && p.height === height)
  return found ? `${found.width}x${found.height}` : MANUAL
}

/** The `theme` value the settings give, or `null` to leave the card on the library's theme. */
function themeValue(mode: ThemeMode, base: string, colors: Partial<Record<ThemeColor, string>>): string | null {
  if (mode === "instance") return null
  if (mode !== "custom") return mode
  const set = Object.fromEntries(Object.entries(colors).filter(([, v]) => v))
  if (Object.keys(set).length === 0) return base === "instance" ? null : base
  return JSON.stringify(base === "instance" ? set : { base, ...set })
}

/** Builds the embed snippet for a share interactively: kind, colours, language and size, with a
 * live preview of the real card (017 FR-030). */
export function EmbedConfigurator({ origin, code }: { origin: string; code: string }) {
  const { t } = useTranslation()
  const [kind, setKind] = useState<Kind>("iframe")
  const [mode, setMode] = useState<ThemeMode>("instance")
  const [base, setBase] = useState("instance")
  const [colors, setColors] = useState<Partial<Record<ThemeColor, string>>>({})
  const [lang, setLang] = useState<Lang>("auto")
  const [width, setWidth] = useState(360)
  const [height, setHeight] = useState(420)
  // Which size row value is chosen: a preset key, `FILL` (the frame follows its container) or
  // `MANUAL` (the two number inputs, which keep whatever numbers are there). Picking a preset or
  // `FILL` hides the inputs again.
  const [size, setSize] = useState<string>(() => presetKey(360, 420))

  const theme = themeValue(mode, base, colors)
  const params = (extra: Record<string, string>) => {
    const query = new URLSearchParams(extra)
    if (theme) query.set("theme", theme)
    if (lang !== "auto") query.set("lang", lang)
    const text = query.toString()
    return text ? `?${text}` : ""
  }
  const snippet =
    kind === "iframe"
      ? size === FILL
        ? // Percentage attributes are legacy sizing hints whose height also needs the container to
          // have one; CSS is the honest form, and `border:0` keeps the frame from overflowing by
          // the default 2px border the way `width="100%"` would.
          `<iframe src="${origin}/embed/s/${code}${params({})}" style="border:0;width:100%;height:100%" loading="lazy" referrerpolicy="no-referrer"></iframe>`
        : `<iframe src="${origin}/embed/s/${code}${params({})}" width="${width}" height="${height}" style="border:0;max-width:100%" loading="lazy" referrerpolicy="no-referrer"></iframe>`
      : `<script src="${origin}/embed/share.js" defer></script>\n<lrrg-share code="${code}" src="${origin}"${
          theme ? (theme.startsWith("{") ? ` theme='${theme}'` : ` theme="${theme}"`) : ""
        }${lang !== "auto" ? ` lang="${lang}"` : ""}></lrrg-share>`

  // The preview is this instance's own card page; a Web Component preview sizes to the card the
  // way the element does.
  const previewSrc = `/embed/s/${code}${params(kind === "element" ? { fit: "1" } : {})}`
  const frame = useRef<HTMLIFrameElement>(null)
  const [autoHeight, setAutoHeight] = useState(360)
  useEffect(() => {
    if (kind !== "element") return
    const onMessage = (e: MessageEvent) => {
      if (e.source !== frame.current?.contentWindow) return
      const data = e.data as { type?: string; height?: number } | null
      if (data?.type === "lrrg-share-height" && typeof data.height === "number") setAutoHeight(data.height)
    }
    window.addEventListener("message", onMessage)
    return () => window.removeEventListener("message", onMessage)
  }, [kind])

  const presets = useMemo(
    () =>
      CARD_THEMES.map((value) => ({ value, label: t(`share.embed.theme.${value}`) })) as {
        value: ThemeMode
        label: string
      }[],
    [t],
  )

  return (
    <div className="share-embed">
      <h3 className="share-embed-heading">{t("share.embed.heading")}</h3>
      <div className="share-form">
        <span>{t("share.embed.kind")}</span>
        <RadioGroup className="share-form-checks" value={kind} onValueChange={setKind}>
          <RadioItem value="iframe">iframe</RadioItem>
          <RadioItem value="element">Web Component</RadioItem>
        </RadioGroup>

        <span>{t("share.embed.colors")}</span>
        <div className="share-embed-row">
          <Select
            value={mode}
            onValueChange={setMode}
            align="start"
            popupZIndex={ABOVE_MODAL}
            items={[
              { value: "instance", label: t("share.embed.theme.instance") },
              ...presets,
              { value: "custom", label: t("share.embed.theme.custom") },
            ]}
          />
          <Tooltip
            zIndex={ABOVE_MODAL}
            maxWidth={380}
            label={
              <div className="share-embed-tip">
                <p>{t("share.embed.customTip")}</p>
                <pre>{`const card = document.querySelector("lrrg-share")
card.theme = {
  base: "dark",       // ${t("share.embed.colorBase")}
  bg: "#101820",      // ${t("share.embed.color.bg")}
  fg: "#f5f5f5",      // ${t("share.embed.color.fg")}
  muted: "#9aa5b1",   // ${t("share.embed.color.muted")}
  border: "#2b3a4a",  // ${t("share.embed.color.border")}
  accent: "#f2aa4c",  // ${t("share.embed.color.accent")}
  tile: "#1c2a38",    // ${t("share.embed.color.tile")}
  qrFg: "#101820",    // ${t("share.embed.color.qrFg")}
  qrBg: "#f5f5f5",    // ${t("share.embed.color.qrBg")}
  qrFinder: "#f2aa4c", // ${t("share.embed.color.qrFinder")}
}`}</pre>
                <p>{t("share.embed.customTipIframe")}</p>
              </div>
            }
          >
            <span className="share-embed-info" tabIndex={0} aria-label={t("share.embed.customTip")}>
              <i className="fa fa-circle-info" aria-hidden="true"></i>
            </span>
          </Tooltip>
        </div>

        {mode === "custom" && (
          <>
            <span>{t("share.embed.colorBase")}</span>
            <Select
              value={base}
              onValueChange={setBase}
              align="start"
              popupZIndex={ABOVE_MODAL}
              items={[{ value: "instance", label: t("share.embed.theme.instance") }, ...presets]}
            />
            {THEME_COLORS.map((key) => (
              <ColorRow
                key={key}
                label={t(`share.embed.color.${key}`)}
                value={colors[key] ?? ""}
                onChange={(value) => setColors((c) => ({ ...c, [key]: value }))}
                clearLabel={t("share.embed.colorClear")}
              />
            ))}
          </>
        )}

        <span>{t("share.embed.lang")}</span>
        <Select
          value={lang}
          onValueChange={setLang}
          align="start"
          popupZIndex={ABOVE_MODAL}
          items={[
            { value: "auto", label: t("share.embed.langAuto") },
            { value: "zh", label: "中文" },
            { value: "en", label: "English" },
            { value: "ja", label: "日本語" },
          ]}
        />
        <span className="share-form-hint">{t("share.embed.langHint")}</span>

        {kind === "iframe" && (
          <>
            <span>{t("share.embed.size")}</span>
            <div className="share-embed-row">
              <Select
                value={size}
                onValueChange={(value) => {
                  if (value === MANUAL || value === FILL) {
                    setSize(value)
                    return
                  }
                  const preset = SIZE_PRESETS.find((p) => `${p.width}x${p.height}` === value)
                  if (!preset) return
                  setSize(value)
                  setWidth(preset.width)
                  setHeight(preset.height)
                }}
                align="start"
                popupZIndex={ABOVE_MODAL}
                items={[
                  ...SIZE_PRESETS.map((p) => ({ value: `${p.width}x${p.height}`, label: `${p.width} × ${p.height}` })),
                  { value: FILL, label: t("share.embed.sizeFill") },
                  { value: MANUAL, label: t("share.embed.sizeManual") },
                ]}
              />
              {size === MANUAL && (
                <>
                  <NumberInput
                    value={width}
                    onValueChange={setWidth}
                    min={260}
                    max={1200}
                    step={10}
                    style={{ width: 90 }}
                  />
                  <span>×</span>
                  <NumberInput
                    value={height}
                    onValueChange={setHeight}
                    min={200}
                    max={1200}
                    step={10}
                    style={{ width: 90 }}
                  />
                  <span className="share-form-hint" style={{ gridColumn: "auto" }}>
                    px
                  </span>
                </>
              )}
            </div>
            <span className="share-form-hint">
              {t(size === FILL ? "share.embed.sizeFillHint" : "share.embed.sizeHint")}
            </span>
          </>
        )}
      </div>

      <p className="share-embed-label">{t("share.embed.preview")}</p>
      <div className="share-embed-preview">
        {/* No `key={previewSrc}`: a changing `src` already navigates the frame, while a key would
            tear this element down and build a new one for every kind/theme/language change. (The
            scroll guard for that re-navigation is `.share-embed-preview`'s `min-height`.) */}
        <iframe
          ref={frame}
          src={previewSrc}
          title={t("share.embed.preview")}
          style={
            kind !== "iframe"
              ? { width: "100%", maxWidth: 480, minWidth: 260, height: autoHeight, border: 0 }
              : // A container-filling frame gets a representative height here: the dialog's preview
                // box has none of its own, and a percentage height would fall back to 150px.
                size === FILL
                ? { width: "100%", height: 420, border: 0 }
                : { width, height, maxWidth: "100%", border: 0 }
          }
        />
      </div>

      <div className="share-embed-code-head">
        <p className="share-embed-label">{t("share.embed.code")}</p>
        <button
          type="button"
          className="stdbtn"
          onClick={() => void copyWithToast(snippet, t("share.result.copied"), t("share.result.copyFailed"))}
        >
          {t("share.result.copy")}
        </button>
      </div>
      <pre className="share-embed-code">{snippet}</pre>
    </div>
  )
}

function ColorRow({
  label,
  value,
  onChange,
  clearLabel,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  clearLabel: string
}) {
  return (
    <>
      <span>{label}</span>
      <div className="share-embed-row">
        {/* No common-ui colour picker exists; the native one is the control here. */}
        <input type="color" value={value || "#808080"} onChange={(e) => onChange(e.target.value)} aria-label={label} />
        <code className={value ? undefined : "share-embed-unset"}>{value || "—"}</code>
        {value && (
          <button type="button" className="favtag-btn" onClick={() => onChange("")}>
            {clearLabel}
          </button>
        )}
      </div>
    </>
  )
}
