/** Entry of `embed.html`: the share card on its own, for iframes and the `<lrrg-share>` element. */
import "../share/card.css"

import { formatShareCode, normalizeShareCode } from "../share/code"
import { type CardData, renderCard } from "./card"
import { cardText } from "./text"
import { applyCardTheme, parseCardTheme } from "./theme"

/** Tells a framing `<lrrg-share>` how tall the card is (only the height), rounded up so a
 * fractional height never leaves the frame a pixel short and scrollable. Returns the reporter:
 * the browser pauses rendering, and with it `ResizeObserver`, in an off-screen cross-origin frame,
 * so the card also reports right after it renders and as each image loads — measuring forces the
 * layout, paused or not. */
function heightReporter(): () => void {
  if (window.parent === window) return () => {}
  const report = () =>
    window.parent.postMessage({ type: "lrrg-share-height", height: Math.ceil(document.documentElement.getBoundingClientRect().height) }, "*")
  new ResizeObserver(report).observe(document.body)
  // `load` does not bubble; capturing catches every image in the card.
  document.addEventListener("load", report, true)
  window.addEventListener("load", report)
  report()
  return report
}

async function main() {
  const root = document.getElementById("card-root")
  if (!root) return
  applyCardTheme(root, parseCardTheme(new URLSearchParams(window.location.search).get("theme"), document.documentElement.dataset.theme))
  const t = cardText()
  const raw = decodeURIComponent(window.location.pathname.split("/").filter(Boolean).at(-1) ?? "")
  const code = normalizeShareCode(raw)
  // Framed by `<lrrg-share>`, which sizes the frame to the card: a scrollbar there only narrows the
  // card (its covers scale with width), so the height measured under it is short and the scrollbar
  // stays. Without one, the width and the height measured are the frame's own.
  const fit = new URLSearchParams(window.location.search).has("fit")
  if (fit) document.documentElement.style.overflow = "hidden"
  const report = heightReporter()
  if (!code) {
    renderCard(root, "", null, t)
    report()
    return
  }
  const data = await fetch(`/api/share/r/card?code=${encodeURIComponent(formatShareCode(code))}`, { credentials: "omit" })
    .then((r) => (r.ok ? (r.json() as Promise<CardData>) : null))
    .catch(() => null)

  /** Draws the card, and for a share of eight items or more decides between one row of covers and
   * two by measuring: a fixed-height frame that would end up scrolling gets one row. `fit` frames
   * grow to the card instead, so they always keep the two rows. Reading `scrollHeight` forces the
   * layout synchronously (the covers' aspect ratios reserve their space before any image loads), so
   * the taller two-row layout is never painted and then replaced. */
  const draw = () => {
    renderCard(root, code, data, t)
    if (!fit && data && (data.item_count ?? 0) >= 8 && document.documentElement.scrollHeight > window.innerHeight + 1) {
      renderCard(root, code, data, t, { coverRows: 1 })
    }
    report()
  }
  draw()

  // The embedding page can resize the frame at any time; two rows may fit afterwards.
  let pending = 0
  window.addEventListener("resize", () => {
    window.clearTimeout(pending)
    pending = window.setTimeout(draw, 150)
  })
}

void main()
