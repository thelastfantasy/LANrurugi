import { useEffect, useRef } from "react"

import { renderQr } from "./qr"

/** A QR code drawn in the browser; nothing about it is fetched from anywhere. */
export function ShareQr({ text, className = "share-qr-canvas", pad }: { text: string; className?: string; pad?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  useEffect(() => {
    if (canvas.current) void renderQr(text, canvas.current, pad)
  }, [text, pad])
  return <canvas ref={canvas} className={className} aria-label={text} />
}
