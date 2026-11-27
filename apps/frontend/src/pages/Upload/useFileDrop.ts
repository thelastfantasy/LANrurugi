import { useEffect, useRef, useState } from "react"

/** Whether a drag carries files at all — dragging selected text or a link over the page must not
 *  light up the drop zone. During a drag the browser exposes the *types*, never the files. */
function carriesFiles(event: DragEvent): boolean {
  const types = event.dataTransfer?.types
  return !!types && Array.from(types).includes("Files")
}

/** Window-wide file drop for the upload page: `dragging` drives a drop zone overlay, and a drop
 *  hands the files to `onFiles`.
 *
 *  Listening on `window` rather than one element is deliberate — the drop zone covers the page, so
 *  a file released anywhere on it lands, including over the queue list or an empty column below the
 *  form. `dragenter`/`dragleave` fire for every element crossed, hence the depth counter; without it
 *  the overlay flickers as the pointer passes over a child node. */
export function useFileDrop(onFiles: (files: File[]) => void): boolean {
  const [dragging, setDragging] = useState(false)
  const depth = useRef(0)
  const latest = useRef(onFiles)
  latest.current = onFiles

  useEffect(() => {
    function reset() {
      depth.current = 0
      setDragging(false)
    }
    function onDragEnter(event: DragEvent) {
      if (!carriesFiles(event)) return
      event.preventDefault()
      depth.current += 1
      setDragging(true)
    }
    function onDragOver(event: DragEvent) {
      if (!carriesFiles(event)) return
      // Without this the browser refuses the drop and opens the file in a new tab instead.
      event.preventDefault()
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy"
    }
    function onDragLeave(event: DragEvent) {
      if (!carriesFiles(event)) return
      // Leaving the window reports no related target; child-to-child moves do, and must not close.
      if (event.relatedTarget === null) {
        reset()
        return
      }
      depth.current = Math.max(0, depth.current - 1)
      if (depth.current === 0) setDragging(false)
    }
    function onDrop(event: DragEvent) {
      if (!carriesFiles(event)) return
      event.preventDefault()
      reset()
      const files = Array.from(event.dataTransfer?.files ?? [])
      if (files.length > 0) latest.current(files)
    }

    window.addEventListener("dragenter", onDragEnter)
    window.addEventListener("dragover", onDragOver)
    window.addEventListener("dragleave", onDragLeave)
    window.addEventListener("drop", onDrop)
    // A drag abandoned with Escape ends without a leave OR a drop.
    window.addEventListener("dragend", reset)
    return () => {
      window.removeEventListener("dragenter", onDragEnter)
      window.removeEventListener("dragover", onDragOver)
      window.removeEventListener("dragleave", onDragLeave)
      window.removeEventListener("drop", onDrop)
      window.removeEventListener("dragend", reset)
    }
  }, [])

  return dragging
}
