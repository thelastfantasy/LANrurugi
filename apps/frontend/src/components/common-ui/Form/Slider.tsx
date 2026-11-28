import { Slider as BaseSlider } from "@base-ui/react/slider"
import type { ReactNode } from "react"

/** A single-value slider on Base UI's `Slider`: click anywhere on the track or drag the thumb.
 * `onValueChange` follows the drag; `onValueCommitted` fires once, on release or on a click, which is
 * where an expensive action (a page jump) belongs. Drawn in `currentColor`, so the caller's text
 * colour themes it. `thumbLabel` floats above the thumb while the slider is hovered or dragged. */
export function Slider({
  value,
  min,
  max,
  step = 1,
  onValueChange,
  onValueCommitted,
  thumbLabel,
  className,
  "aria-label": ariaLabel,
}: {
  value: number
  min: number
  max: number
  step?: number
  onValueChange?: (value: number) => void
  onValueCommitted?: (value: number) => void
  thumbLabel?: ReactNode
  className?: string
  "aria-label"?: string
}) {
  return (
    <BaseSlider.Root
      value={value}
      min={min}
      max={max}
      step={step}
      onValueChange={(next) => onValueChange?.(next)}
      onValueCommitted={(next) => onValueCommitted?.(next)}
      className={`group relative w-full touch-none select-none ${className ?? ""}`}
    >
      <BaseSlider.Control className="flex w-full cursor-pointer items-center py-2.5">
        <BaseSlider.Track className="relative h-1.5 w-full rounded-full bg-current/25 transition-[height] group-hover:h-2 group-data-[dragging]:h-2">
          <BaseSlider.Indicator className="rounded-full bg-current" />
          <BaseSlider.Thumb aria-label={ariaLabel} className="size-3.5 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-current">
            {/* The dot grows on hover; the label is a sibling so it keeps its size. */}
            <span className="absolute inset-0 rounded-full bg-current shadow-[0_0_0_3px_rgba(0,0,0,0.35)] transition-transform group-hover:scale-125 group-data-[dragging]:scale-125" />
            {thumbLabel != null && (
              <span className="pointer-events-none absolute bottom-full left-1/2 mb-2.5 -translate-x-1/2 whitespace-nowrap rounded px-1.5 py-0.5 text-xs tabular-nums opacity-0 transition-opacity group-hover:opacity-100 group-data-[dragging]:opacity-100 bg-black/80 text-white">
                {thumbLabel}
              </span>
            )}
          </BaseSlider.Thumb>
        </BaseSlider.Track>
      </BaseSlider.Control>
    </BaseSlider.Root>
  )
}
