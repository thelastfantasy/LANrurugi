/**
 * QR codes, entirely in the browser (FR-009, FR-017). Both libraries are imported on demand so
 * they stay out of every other page's bundle.
 */

/** Black on white with a quiet zone, so a saved PNG scans on any background. */
const QR_STYLE = { on: [0, 0, 0, 255], off: [255, 255, 255, 255], pad: 4 } as const

/** Custom QR colours (any CSS colour): modules, background, and the three finder squares. */
export interface QrColors {
  fg?: string
  bg?: string
  finder?: string
}

/** Draws `text` as a QR code onto `canvas`, one pixel per module; `pad` overrides the quiet zone
 * in modules. */
export async function renderQr(
  text: string,
  canvas: HTMLCanvasElement,
  pad: number = QR_STYLE.pad,
  colors?: QrColors,
): Promise<void> {
  const { generate } = await import("lean-qr")
  const code = generate(text)
  if (!colors?.fg && !colors?.bg && !colors?.finder) {
    code.toCanvas(canvas, { ...QR_STYLE, pad })
    return
  }
  const size = code.size
  canvas.width = canvas.height = size + pad * 2
  const ctx = canvas.getContext("2d")
  if (!ctx) return
  ctx.fillStyle = colors.bg ?? "#ffffff"
  ctx.fillRect(0, 0, canvas.width, canvas.height)
  const fg = colors.fg ?? "#000000"
  const finder = colors.finder ?? fg
  // The three 7×7 position squares in the corners (all but bottom-right).
  const inFinder = (x: number, y: number) => (x < 7 || x >= size - 7) && (y < 7 || y >= size - 7) && !(x >= size - 7 && y >= size - 7)
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      if (!code.get(x, y)) continue
      ctx.fillStyle = inFinder(x, y) ? finder : fg
      ctx.fillRect(x + pad, y + pad, 1, 1)
    }
  }
}

/** The QR code for `text` as a PNG, scaled up for printing or sharing as an image. */
export async function qrPngBlob(text: string, scale = 8): Promise<Blob> {
  const { generate } = await import("lean-qr")
  const code = generate(text)
  const small = document.createElement("canvas")
  code.toCanvas(small, QR_STYLE)
  const canvas = document.createElement("canvas")
  canvas.width = small.width * scale
  canvas.height = small.height * scale
  const ctx = canvas.getContext("2d")
  if (!ctx) throw new Error("canvas unavailable")
  ctx.imageSmoothingEnabled = false
  ctx.drawImage(small, 0, 0, canvas.width, canvas.height)
  return new Promise((resolve, reject) =>
    canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("PNG encoding failed"))), "image/png"),
  )
}

type Detector = { detect(source: ImageBitmapSource | HTMLVideoElement): Promise<{ rawValue: string }[]> }

let detector: Promise<Detector> | null = null

/** The QR detector: the browser's own `BarcodeDetector` when it has one, else the zxing-wasm
 * fallback. The wasm is bundled with this instance and never fetched from a CDN, so decoding works
 * offline and tells no third party anything (FR-017). */
function getDetector(): Promise<Detector> {
  detector ??= (async () => {
    const { BarcodeDetector, prepareZXingModule } = await import("barcode-detector/ponyfill")
    const { default: wasmUrl } = await import("zxing-wasm/reader/zxing_reader.wasm?url")
    prepareZXingModule({
      overrides: { locateFile: (path: string, prefix: string) => (path.endsWith(".wasm") ? wasmUrl : prefix + path) },
    })
    return new BarcodeDetector({ formats: ["qr_code"] })
  })()
  return detector
}

/** Every QR payload found in an image or video frame. */
export async function detectQr(source: Blob | HTMLVideoElement): Promise<string[]> {
  const d = await getDetector()
  const found = await d.detect(source)
  return found.map((b) => b.rawValue)
}
