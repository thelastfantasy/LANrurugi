/** Fixed color per action-type namespace (`archive.*`, `settings.*`, ...) and per actor kind
 * (`session`/`token`/`system`/`anonymous`) — a real, deliberately-picked palette (not a
 * hash-string-to-hue function): a hash gives an inconsistent, uncontrolled, sometimes muddy color
 * for the same namespace across sessions/renders depending on what happens to hash where, and
 * offers no way to keep e.g. `database.*` reading as "destructive" (matching this page's existing
 * `.activity-chip-danger` convention for that one namespace) or to keep two visually-similar
 * namespaces from landing on near-identical hues by coincidence. Each entry is a `{ bg, text }`
 * pair chosen for contrast (WCAG-AA-ish at this chip's small size), not theme-adaptive — unlike
 * `.activity-row-automatic`/`.activity-chip-danger` (CSS classes per theme file), these are a
 * fixed categorical palette in the same spirit as `Jobs/JobProgress.tsx`'s own `STATE_COLOR`
 * (state → fixed color, not derived from the active theme) — a colored action/actor badge reads
 * the same regardless of which of the 5 UI themes is active, the same way GitHub's own labels
 * don't restyle themselves per OS dark/light mode. */

export interface ChipColor {
  bg: string
  text: string
}

const NAMESPACE_COLORS: Record<string, ChipColor> = {
  archive: { bg: "#3b82f6", text: "#ffffff" },
  settings: { bg: "#8b5cf6", text: "#ffffff" },
  category: { bg: "#f59e0b", text: "#1f2937" },
  token: { bg: "#06b6d4", text: "#ffffff" },
  download_queue: { bg: "#10b981", text: "#ffffff" },
  tankoubon: { bg: "#ec4899", text: "#ffffff" },
  database: { bg: "#dc2626", text: "#ffffff" },
  plugin: { bg: "#6366f1", text: "#ffffff" },
  scanner: { bg: "#64748b", text: "#ffffff" },
  metadata_plugin: { bg: "#0ea5e9", text: "#ffffff" },
  auto_download: { bg: "#84cc16", text: "#1f2937" },
}

const FALLBACK_NAMESPACE_COLOR: ChipColor = { bg: "#6b7280", text: "#ffffff" }

/** `"archive.delete"` → the `archive` namespace's fixed color. Unknown/future namespaces fall
 * back to a neutral gray rather than crashing or going unstyled — a new `action_types` constant
 * added to the backend later still renders a real (if generic) chip immediately, no frontend
 * change required to avoid a broken-looking badge. */
export function actionTypeColor(actionType: string): ChipColor {
  const namespace = actionType.includes(".") ? actionType.slice(0, actionType.indexOf(".")) : actionType
  return NAMESPACE_COLORS[namespace] ?? FALLBACK_NAMESPACE_COLOR
}

const ACTOR_KIND_COLORS: Record<string, ChipColor> = {
  session: { bg: "#22c55e", text: "#ffffff" },
  system: { bg: "#64748b", text: "#ffffff" },
  anonymous: { bg: "#9ca3af", text: "#1f2937" },
}

/** A fixed, hand-picked rotation for `token`-kind actors specifically — unlike `session`/`system`/
 * `anonymous` (each a single fixed concept with exactly one real-world instance, so one fixed
 * color per kind is enough to tell them apart), an instance can have many distinct API tokens, and
 * two different tokens both rendering as the same flat "token cyan" would make them
 * indistinguishable at a glance in a mixed activity feed — the whole point of a colored chip.
 * Deterministically picked by hashing the token's own id (stable across renders/reloads, no
 * server-side color assignment needed) rather than by creation order (would shift as tokens are
 * added/removed, an already-memorized color changing out from under a user for no visible reason). */
const TOKEN_ROTATION: ChipColor[] = [
  { bg: "#06b6d4", text: "#ffffff" },
  { bg: "#f97316", text: "#ffffff" },
  { bg: "#a855f7", text: "#ffffff" },
  { bg: "#14b8a6", text: "#ffffff" },
  { bg: "#eab308", text: "#1f2937" },
  { bg: "#f43f5e", text: "#ffffff" },
  { bg: "#0ea5e9", text: "#ffffff" },
  { bg: "#22c55e", text: "#ffffff" },
]

function hashString(value: string): number {
  let hash = 0
  for (let i = 0; i < value.length; i++) {
    hash = (hash * 31 + value.charCodeAt(i)) | 0
  }
  return Math.abs(hash)
}

/** `kind` alone for `session`/`system`/`anonymous`; `tokenId` (the token's own record id, not its
 * display name — a rename must not shift which color it's already known by) picks this token's
 * spot in `TOKEN_ROTATION` for the `token` kind. */
export function actorKindColor(kind: string, tokenId?: string): ChipColor {
  if (kind === "token") {
    if (!tokenId) return TOKEN_ROTATION[0]
    return TOKEN_ROTATION[hashString(tokenId) % TOKEN_ROTATION.length]
  }
  return ACTOR_KIND_COLORS[kind] ?? FALLBACK_NAMESPACE_COLOR
}
