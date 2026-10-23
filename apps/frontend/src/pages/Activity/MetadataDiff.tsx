import { diffWords } from "diff"
import { useTranslation } from "react-i18next"

/** Word-level text diff rendered GitHub-commit-style: unchanged plain text, added in green,
 * removed in red with strikethrough. */
export function TextFieldDiff({ before, after }: { before: string; after: string }) {
  if (before === after) return <>{after}</>
  const parts = diffWords(before, after)
  return (
    <>
      {parts.map((part, i) => {
        if (part.added) {
          return (
            <span key={i} style={{ background: "rgba(46, 160, 67, 0.25)", color: "#2ea043" }}>
              {part.value}
            </span>
          )
        }
        if (part.removed) {
          return (
            <span key={i} style={{ background: "rgba(248, 81, 73, 0.2)", color: "#f85149", textDecoration: "line-through" }}>
              {part.value}
            </span>
          )
        }
        return <span key={i}>{part.value}</span>
      })}
    </>
  )
}

/** Tag set diff (backend-precomputed `tags_added`/`tags_removed`) — added tags render as green
 * chips, removed as red strikethrough chips. */
export function TagSetDiff({ added, removed }: { added: string[]; removed: string[] }) {
  const { t } = useTranslation()
  if (added.length === 0 && removed.length === 0) return <>{t("activity.noTagChanges")}</>
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
      {added.map((tag) => (
        <span
          key={`added-${tag}`}
          style={{
            display: "inline-flex",
            alignItems: "center",
            padding: "1px 8px",
            borderRadius: 999,
            fontSize: "0.8125rem",
            background: "rgba(46, 160, 67, 0.2)",
            color: "#2ea043",
          }}
        >
          + {tag}
        </span>
      ))}
      {removed.map((tag) => (
        <span
          key={`removed-${tag}`}
          style={{
            display: "inline-flex",
            alignItems: "center",
            padding: "1px 8px",
            borderRadius: 999,
            fontSize: "0.8125rem",
            background: "rgba(248, 81, 73, 0.15)",
            color: "#f85149",
            textDecoration: "line-through",
          }}
        >
          {tag}
        </span>
      ))}
    </div>
  )
}

/** `before` carries pre-edit `title`/`summary`; `after` carries post-edit values plus the tag diff. */
export interface MetadataUpdateBefore {
  title?: string
  summary?: string
}

export interface MetadataUpdateAfter {
  title?: string
  summary?: string
  tags_added?: string[]
  tags_removed?: string[]
}

/** Full diff view for one `archive.metadata_update` entry — title/summary as word-level text
 * diffs, tags as an added/removed chip set. */
export function MetadataDiff({ before, after }: { before: MetadataUpdateBefore; after: MetadataUpdateAfter }) {
  const { t } = useTranslation()
  return (
    <dl style={{ display: "grid", gridTemplateColumns: "auto 1fr", columnGap: 12, rowGap: 8, fontSize: "0.875rem" }}>
      {after.title != null && (
        <>
          <dt style={{ opacity: 0.65 }}>{t("edit.title")}</dt>
          <dd style={{ margin: 0 }}>
            <TextFieldDiff before={before.title ?? ""} after={after.title} />
          </dd>
        </>
      )}
      {after.summary != null && (
        <>
          <dt style={{ opacity: 0.65 }}>{t("edit.summary")}</dt>
          <dd style={{ margin: 0 }}>
            <TextFieldDiff before={before.summary ?? ""} after={after.summary} />
          </dd>
        </>
      )}
      {(after.tags_added != null || after.tags_removed != null) && (
        <>
          <dt style={{ opacity: 0.65 }}>{t("common.tags")}</dt>
          <dd style={{ margin: 0 }}>
            <TagSetDiff added={after.tags_added ?? []} removed={after.tags_removed ?? []} />
          </dd>
        </>
      )}
    </dl>
  )
}
