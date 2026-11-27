/** A member of the tankoubon that matched the query, as `search.rs` reports it. */
export interface MatchReasonMember {
  arcid?: string
  name: string
  /** The volume's own title. */
  title?: string
  /** The name the tankoubon gives this volume; null when it has none. */
  chapter?: string | null
}

/** The same split, for rendering: `lead` is the archive's own name (always printed, and the part a
 * reader scans for), `rest` is the tankoubon's chapter name when it differs from it. Callers bold
 * `lead` and leave `rest` plain. */
export function memberParts(member: MatchReasonMember): { lead: string; rest: string | null } {
  const title = member.title?.trim() ?? ""
  const chapter = member.chapter?.trim() ?? ""
  // `includes`, not `!==`: plenty of tanks name the volume exactly the way the archive's own title
  // already ends ("Yuikai Chan - Sparxie" / "Sparxie"), and repeating it reads like a duplication bug.
  const alreadyNamed = title.toLowerCase().includes(chapter.toLowerCase())
  if (title) return { lead: title, rest: chapter && !alreadyNamed ? chapter : null }
  const lead = chapter || member.name || "?"
  return { lead, rest: null }
}

/** What to print on one line of the tooltip.
 *
 * The chapter name alone is not enough: a tankoubon that renamed its volumes calls them "巻の拾",
 * which says nothing about which manga it is. Both halves are shown when the tank renamed it, and
 * the title alone when it did not — printing "Title — Title" twice would be worse than one. */
export function memberLabel(member: MatchReasonMember): string {
  const { lead, rest } = memberParts(member)
  return rest ? `${lead} — ${rest}` : lead
}
