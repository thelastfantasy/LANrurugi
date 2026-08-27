import { useSettings } from "@/api/hooks"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import {
  BARE_TAG_NAMESPACE,
  displayNamespace,
  formatTagValue,
  getTagSearchURL,
  splitTagsByNamespace,
  tagValueForSearch,
} from "@/lib/tagFormat"
import { parseRating } from "@/lib/utils/rating"

/** Per-namespace tag table — the *content* legacy's own `buildTagsDiv`
 * (`~/LANraragi/public/js/mod/common.js`) renders inside a hover tooltip: one row per namespace
 * (sorted alphabetically), each value its own chip (search-filter for most namespaces, external
 * link for `source`). Deliberately NOT legacy's own `<table class="itg">` row/cell markup — that
 * CSS predates flexbox and fights it (fixed `width: 100px !important` columns, etc.) — this is a
 * plain flex layout instead. The chip/color *appearance* is still legacy's own, verified against
 * `~/LANraragi/public/themes/*.css`: every chip is a real `.gt` div (the vendored theme CSS
 * already gives it a background + border, differing per theme — `useApplyTheme`, already called
 * by every page this renders on, keeps that in sync with the active theme with zero extra work
 * here), and the namespace-color class (`.artist-tag`, `.series-tag`, etc. from `lrr.css`) goes on
 * the row *label*, matching legacy's own `<td class='caption-namespace ${key}-tag'>` — NOT on each
 * chip, which legacy always renders in the same neutral `.gt` style regardless of namespace.
 * Meant to be the `label` of a `Tooltip` (the shared, portaled hover-bubble component) rather than
 * rendering its own positioning/outer-border/shadow — callers own that outer chrome. */
export function TagTable({
  tags,
  onSearchTag,
}: {
  tags: string
  onSearchTag?: (namespace: string, value: string) => void
}) {
  // Server timezone for timestamp-namespace display + search-URL conversion — see
  // `ArchiveOverviewOverlay.tsx`'s own `TagsTable` for the same pattern.
  const settings = useSettings()
  const timezone = settings.data?.timezone ?? ""
  const byNamespace = splitTagsByNamespace(tags)
  const namespaces = Object.keys(byNamespace).sort()
  if (namespaces.length === 0) return null

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {namespaces.map((namespace) => {
        // A bare (non-namespaced) tag collects under the internal `BARE_TAG_NAMESPACE` sentinel
        // (see that constant's own docs) — remapped to the literal `"other"` label here, same as
        // `colorCodeTags` already does, matching legacy's own real `splitTagsByNamespace`/
        // `buildTagsDiv` behavior (`~/LANraragi/public/js/mod/common.js`: an untagged tag is
        // literally bucketed as `"other"` and rendered with an ordinary `"Other:"` row, no special
        // casing). Used for BOTH the CSS class and the visible label below — the class needs it
        // too, or the sentinel's own raw bytes still leak into the `className` string even once
        // the visible text is fixed.
        const displayKey = namespace === BARE_TAG_NAMESPACE ? "other" : namespace
        return (
        <div key={namespace} style={{ display: "flex", gap: 6, alignItems: "flex-start", minWidth: 0 }}>
          <div
            className={`caption-namespace ${displayKey.toLowerCase()}-tag`}
            style={{ fontWeight: "bold", flex: "0 0 auto", whiteSpace: "nowrap", padding: 0 }}
          >
            {displayNamespace(displayKey)}:
          </div>
          {/* `minWidth: 0` — a flex item's implicit `min-width: auto` default sizes it to its
              content's intrinsic width regardless of the row's own constraint, so a long unbroken
              string (a `download:`/`source:` URL, which is `word-break: break-all` further down
              but that alone can't shrink the *flex item itself*) overflowed the tooltip bubble's
              own `max-width` instead of wrapping within it — confirmed live via the homepage's
              hover tooltip on a chaika.moe `download:` tag. */}
          <div style={{ display: "flex", flexWrap: "wrap", minWidth: 0 }}>
            {displayKey.toLowerCase() === "rating" ? (
              // Still a real, working search-link chip (legacy's own real rating chip *is*
              // clickable — see `TagsTable`'s own docs in `ArchiveOverviewOverlay.tsx` for the
              // live-verified detail) — just no underline on it specifically, which reads like a
              // broken/dead link at a glance and the star icons alone don't need to invite.
              <div className="gt">
                <a
                  href={getTagSearchURL(displayKey, byNamespace[namespace][0] ?? "", timezone)}
                  onClick={(e) => {
                    if (!onSearchTag) return
                    e.preventDefault()
                    e.stopPropagation()
                    onSearchTag(displayKey, byNamespace[namespace][0] ?? "")
                  }}
                  style={{ textDecoration: "none", cursor: onSearchTag ? "pointer" : undefined }}
                >
                  <StarRatingDisplay rating={parseRating(byNamespace[namespace][0]) ?? 0} size={14} />
                </a>
              </div>
            ) : (
              byNamespace[namespace].map((value, i) => (
                // `.gt` (legacy, vendored per-theme — see g.css's own rule) hardcodes
                // `white-space: nowrap; overflow: hidden; text-overflow: ellipsis`, meant for
                // short single-word tag chips — on a long unbroken value (a `download:`/`source:`
                // URL) that clips the *middle* of the string at a fixed pixel width, which for a
                // URL means the meaningful part (the path/gallery id at the end) disappears while
                // the domain stays visible — confirmed live: a `panda.chaika.moe` download URL
                // ellipsized right after the domain, hiding the actual archive id entirely. These
                // three overrides don't touch the shared `.gt` class other short chips still use
                // unmodified — only this one instance, so the value wraps across lines and stays
                // fully readable/copyable instead of being silently truncated.
                <div
                  key={i}
                  className="gt"
                  style={{ maxWidth: "100%", whiteSpace: "normal", overflow: "visible", textOverflow: "clip" }}
                >
                  {displayKey === "source" ? (
                    <a
                      href={/^https?:\/\//i.test(value) ? value : `https://${value}`}
                      target="_blank"
                      rel="noreferrer"
                      onClick={(e) => e.stopPropagation()}
                      style={{ wordBreak: "break-all" }}
                    >
                      {value}
                    </a>
                  ) : (
                    <a
                      href={getTagSearchURL(displayKey, value, timezone)}
                      onClick={(e) => {
                        // A real `href` (rather than legacy's placeholder `href="#"`) so
                        // middle-click/right-click/hover-preview all resolve to the actual search
                        // URL, matching legacy's own `buildTagsDiv` — but a plain left-click still
                        // applies the filter in-app (no full navigation/reload) when a handler is
                        // given, same as before.
                        //
                        // `tagValueForSearch`, not the raw `value` — a timestamp-namespace tag's
                        // raw stored value is a bare Unix-seconds number, but its search semantics
                        // are now the `yyyy-mm-dd` day-range syntax (see `getTagSearchURL`'s own
                        // docs); the `href` above already does this conversion, but this in-app
                        // `onSearchTag` path is a *separate* code path that bypassed it entirely —
                        // a real, live-confirmed bug where left-click search used the untranslated
                        // raw timestamp (never matching anything, since `date_added` isn't
                        // indexed for exact/fuzzy tag search) while middle-click/right-click
                        // (which actually navigate via `href`) got the correct date-range query.
                        if (!onSearchTag) return
                        e.preventDefault()
                        e.stopPropagation()
                        onSearchTag(displayKey, tagValueForSearch(displayKey, value, timezone))
                      }}
                      style={{ wordBreak: "break-all", cursor: onSearchTag ? "pointer" : undefined }}
                    >
                      {formatTagValue(displayKey, value, timezone)}
                    </a>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
        )
      })}
    </div>
  )
}
