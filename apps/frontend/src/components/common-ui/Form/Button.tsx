import { Button as BaseButton } from "@base-ui/react/button"
import type { ComponentProps } from "react"

/** Site-wide `.stdbtn`-styled button, built on Base UI's `Button` (a real `<button>`) rather than
 * legacy's own `<input type="button">` — this is what makes it a fair visual-parity partner for
 * `Select` below: both render an actual `<button>` as their outer box, so there's no native
 * `<select>`-vs-`<input>` platform rendering gap to fight (the exact gap that made a plain
 * `<select className="favtag-btn">` sit at a different height/vertical position than a
 * `<input className="stdbtn">` even with every CSS metric forced to match — confirmed live,
 * 2026-08-27). `.stdbtn`/`.favtag-btn` (via the `variant` prop) are legacy's own theme-defined
 * classes already present in all 5 real theme files — no new colors introduced here. */
export function Button({
  variant = "stdbtn",
  ...props
}: { variant?: "stdbtn" | "favtag-btn" } & Omit<ComponentProps<typeof BaseButton>, "className">) {
  return <BaseButton className={variant} {...props} />
}
