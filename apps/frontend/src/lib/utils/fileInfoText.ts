import type { Spread } from "@/hooks/useReaderNavigation"

interface PageDimensions {
  width: number
  height: number
}

/** Legacy's `.file-info` (`updateMetadata`, reader.js:1281): "filename :: WxH :: sizeKB", and
 * "fileA - fileB :: (WA+WB)xH :: (sizeA+sizeB)KB" for a double-page spread. Extracted as a pure
 * function (out of Reader.tsx's closure) so it's unit-testable without a full component render. */
export function fileInfoText(
  pageUrls: { url: string }[],
  spread: Spread,
  pageDimensions: Record<number, PageDimensions>,
  pageSizesKb: Record<number, number>,
  origin: string,
): string {
  const nameFromUrl = (url: string | undefined) =>
    url ? (new URL(url, origin).searchParams.get("path") ?? "") : ""

  const leftUrl = pageUrls[spread.left - 1]?.url
  const leftName = nameFromUrl(leftUrl)
  const leftDim = pageDimensions[spread.left]
  const leftSize = pageSizesKb[spread.left]

  if (spread.right === null) {
    if (!leftDim || leftSize === undefined) return leftName
    return `${leftName} :: ${leftDim.width} x ${leftDim.height} :: ${leftSize} KB`
  }

  const rightUrl = pageUrls[spread.right - 1]?.url
  const rightName = nameFromUrl(rightUrl)
  const rightDim = pageDimensions[spread.right]
  const rightSize = pageSizesKb[spread.right]
  if (!leftDim || !rightDim || leftSize === undefined || rightSize === undefined) {
    return `${leftName} - ${rightName}`
  }
  return `${leftName} - ${rightName} :: ${leftDim.width + rightDim.width} x ${leftDim.height} :: ${leftSize + rightSize} KB`
}
