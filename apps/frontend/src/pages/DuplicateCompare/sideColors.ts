/** A/B identity colors inside the compare overlay, which is always dark regardless of theme
 *  (it is an image viewer, like the reader). Shared by the badges, the band and the touch toggle. */
export const SIDE_COLORS = {
  a: "rgba(58,124,199,0.9)",
  b: "rgba(46,125,79,0.9)",
} as const

/** Member colors for the multi-archive view: A and B match the pair view, the rest continue. */
export const MEMBER_COLORS = [
  SIDE_COLORS.a,
  SIDE_COLORS.b,
  "rgba(142,91,181,0.9)",
  "rgba(181,101,29,0.9)",
  "rgba(42,138,138,0.9)",
  "rgba(163,58,91,0.9)",
  "rgba(107,125,42,0.9)",
  "rgba(90,90,90,0.9)",
] as const

export function memberLetter(index: number): string {
  return String.fromCharCode(65 + (index % 26))
}

export function memberColor(index: number): string {
  return MEMBER_COLORS[index % MEMBER_COLORS.length]
}
