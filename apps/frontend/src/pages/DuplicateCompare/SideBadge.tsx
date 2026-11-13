import type { ComparisonSide } from "@/api/types"

import { SIDE_COLORS } from "./sideColors"

/** Round lettered marker. Drawn as SVG so the letter is centered on its cap height: as HTML text it
 *  sat by the line box instead, which CJK theme fonts and inherited letter-spacing pushed up/left. */
export function LetterBadge({
  letter,
  color,
  size = 20,
  active,
}: {
  letter: string
  color: string
  size?: number
  active?: boolean
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      aria-label={letter}
      role="img"
      style={{ flexShrink: 0, display: "block" }}
    >
      <circle cx="10" cy="10" r="10" fill={color} />
      {active && <circle cx="10" cy="10" r="9" fill="none" stroke="#fff" strokeWidth="2" />}
      <text
        x="10"
        // Caps are ~0.7em tall in sans-serif faces: a baseline 0.35em below center centers them.
        y={10 + 0.35 * 11}
        textAnchor="middle"
        fontSize="11"
        fontWeight="700"
        fontFamily="system-ui, -apple-system, 'Segoe UI', Roboto, Arial, sans-serif"
        letterSpacing="0"
        fill="#fff"
      >
        {letter}
      </text>
    </svg>
  )
}

/** The A/B marker of the two-archive view. */
export function SideBadge({ side, size, active }: { side: ComparisonSide; size?: number; active?: boolean }) {
  return <LetterBadge letter={side.toUpperCase()} color={SIDE_COLORS[side]} size={size} active={active} />
}
