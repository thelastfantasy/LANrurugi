import { Tooltip } from "@/components/common-ui/Display"
import { TagTable } from "@/components/Display"
import { colorCodeTags } from "@/lib/tagFormat"

/** Timestamps say nothing about the content, so share pages leave them out. `source:` stays: it is
 * part of what the archive is, the same way the library shows it. A plugin's `download:` URL stays
 * out — it can carry the sharer's own access key, which a recipient has no business seeing. */
const HIDDEN_NAMESPACES = /^\s*(download|date_added|timestamp):/i

/** The tags a share page shows, as one comma-separated string. */
export function visibleShareTags(tags: string): string {
  return tags
    .split(",")
    .filter((t) => t.trim() && !HIDDEN_NAMESPACES.test(t))
    .join(",")
}

/** Read-only tag line for share pages: one truncated line, the full table in a hover tooltip like the
 * library's `TagLine`, which reads instance settings a recipient without an account cannot load. */
export function ShareTags({ tags }: { tags: string }) {
  const shown = visibleShareTags(tags)
  const coded = colorCodeTags(shown)
  if (coded.length === 0) return null
  return (
    <Tooltip
      label={<TagTable tags={shown} links={false} loadSettings={false} />}
      wrapperStyle={{ display: "block", minWidth: 0 }}
    >
      <span className="tags share-tags">
        {coded.map((tag, i) => (
          <span key={`${tag.namespace}:${tag.text}:${i}`} className={`${tag.namespace}-tag`}>
            {tag.text}
            {i < coded.length - 1 ? ", " : ""}
          </span>
        ))}
      </span>
    </Tooltip>
  )
}
