import { Tooltip } from "@/components/common-ui/Display"
import { TagTable } from "@/components/Display"
import { colorCodeTags } from "@/lib/tagFormat"

/** Where and when the sharer got an archive (`source:`, a plugin's `download:` URL, `date_added:`)
 * says nothing about its content, so share pages leave it out. */
const HIDDEN_NAMESPACES = /^\s*(source|download|date_added):/i

/** Read-only tag line for share pages: one truncated line, the full table in a hover tooltip like the
 * library's `TagLine`, which reads instance settings a recipient without an account cannot load. */
export function ShareTags({ tags }: { tags: string }) {
  const shown = tags
    .split(",")
    .filter((t) => t.trim() && !HIDDEN_NAMESPACES.test(t))
    .join(",")
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
