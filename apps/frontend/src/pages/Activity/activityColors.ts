// Fixed, hand-picked palette per action-type namespace/actor kind — not theme-adaptive or hashed,
// so colors stay stable and distinguishable across renders/themes.

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
  plugin_wizard: { bg: "#818cf8", text: "#1f2937" },
  scanner: { bg: "#64748b", text: "#ffffff" },
  metadata_plugin: { bg: "#0ea5e9", text: "#ffffff" },
  auto_download: { bg: "#84cc16", text: "#1f2937" },
  // Not gray — gray reads as automated/system here, but bookmarking is a manual action.
  bookmark: { bg: "#f43f5e", text: "#ffffff" },
}

const FALLBACK_NAMESPACE_COLOR: ChipColor = { bg: "#6b7280", text: "#ffffff" }

export function actionTypeColor(actionType: string): ChipColor {
  const namespace = actionType.includes(".") ? actionType.slice(0, actionType.indexOf(".")) : actionType
  return NAMESPACE_COLORS[namespace] ?? FALLBACK_NAMESPACE_COLOR
}

const ACTOR_KIND_COLORS: Record<string, ChipColor> = {
  session: { bg: "#22c55e", text: "#ffffff" },
  system: { bg: "#64748b", text: "#ffffff" },
  anonymous: { bg: "#9ca3af", text: "#1f2937" },
}

/** Per-token color rotation, hashed from the token's own id (stable, no server assignment needed). */
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

export function actorKindColor(kind: string, tokenId?: string): ChipColor {
  if (kind === "token") {
    if (!tokenId) return TOKEN_ROTATION[0]
    return TOKEN_ROTATION[hashString(tokenId) % TOKEN_ROTATION.length]
  }
  return ACTOR_KIND_COLORS[kind] ?? FALLBACK_NAMESPACE_COLOR
}

const OUTCOME_COLORS: Record<string, ChipColor> = {
  success: { bg: "#22c55e", text: "#ffffff" },
  failure: { bg: "#dc2626", text: "#ffffff" },
}

export function outcomeColor(status: string): ChipColor {
  return OUTCOME_COLORS[status] ?? FALLBACK_NAMESPACE_COLOR
}
