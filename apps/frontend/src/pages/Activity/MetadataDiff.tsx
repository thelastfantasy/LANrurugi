import { diffWords } from "diff"
import { useTranslation } from "react-i18next"

/** Word-level text diff (title/summary) rendered GitHub-commit-style: an unchanged span in plain
 * text, an added span in green, a removed span in red with a strikethrough — `diff`'s own
 * `diffWords` (word/punctuation-tokenized, not raw character diffing, so a single edited word
 * inside a longer sentence highlights as just that word, not the whole surrounding line) is the
 * real diff algorithm computing which spans are which; this only maps its `ChangeObject[]` output
 * onto colored `<span>`s. */
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

/** Tag set diff (from the backend's own precomputed `tags_added`/`tags_removed` — see
 * `archives.rs::update_archive_metadata`'s own docs on why this is a real `HashSet` difference
 * computed server-side, not two raw comma-joined strings for the frontend to parse and diff
 * itself) — each added tag renders as a green chip, each removed tag as a red
 * strikethrough chip, GitHub-commit-diff style matching {@link TextFieldDiff}'s own color
 * convention for the title/summary fields. */
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

/** The `after` shape `archives.rs::update_archive_metadata` writes — `before` carries only
 * `title`/`summary` (the pre-edit values), `after` carries the post-edit `title`/`summary` plus
 * the precomputed tag diff (see `TagSetDiff`'s own docs on why tags are a set difference, not
 * plain strings). */
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
 * diffs, tags as an added/removed chip set. Renders only the fields that actually changed (a
 * request that only touched `tags`, say, has `before.title === after.title` and
 * `TextFieldDiff` already collapses an unchanged pair to plain unstyled text, but the row is
 * still shown for consistency — hiding whole rows based on equality would make the layout jump
 * around depending on which fields a given edit happened to touch). */
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
