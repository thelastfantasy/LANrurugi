import type { Spread } from "@/hooks/useReaderNavigation"

interface PageDimensions {
  width: number
  height: number
}

/** Legacy's `.file-info` format (`updateMetadata`, reader.js:1281): "filename :: WxH :: sizeKB",
 * or "fileA - fileB :: (WA+WB)xH :: (sizeA+sizeB)KB" for a double-page spread. */
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
    if (leftSize === undefined) return leftName
    if (!leftDim) return `${leftName} :: ${leftSize} KB`
    return `${leftName} :: ${leftDim.width} x ${leftDim.height} :: ${leftSize} KB`
  }

  const rightUrl = pageUrls[spread.right - 1]?.url
  const rightName = nameFromUrl(rightUrl)
  const rightDim = pageDimensions[spread.right]
  const rightSize = pageSizesKb[spread.right]
  if (leftSize === undefined || rightSize === undefined) {
    return `${leftName} - ${rightName}`
  }
  if (!leftDim || !rightDim) {
    return `${leftName} - ${rightName} :: ${leftSize + rightSize} KB`
  }
  return `${leftName} - ${rightName} :: ${leftDim.width + rightDim.width} x ${leftDim.height} :: ${leftSize + rightSize} KB`
}
