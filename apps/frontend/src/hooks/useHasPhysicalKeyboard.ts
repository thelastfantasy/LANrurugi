import { useEffect, useState } from "react"

import { useSupportsHover } from "./useSupportsHover"

/**
 * Whether a keyboard is plausibly available for the keyboard shortcuts the reader documents.
 *
 * **There is no API that reports "a hardware keyboard is attached."** `navigator.keyboard` only
 * exposes layout, and pairing a Bluetooth keyboard changes nothing observable. This combines the
 * two signals that do exist, each strong for the case that matters: a hover-capable pointer
 * (mouse/trackpad — see `useSupportsHover`) and `(any-pointer: fine)`. Phones and tablets report
 * neither, because they have neither.
 *
 * Deliberately **not** tracking `keydown`: a phone's soft keyboard dispatches ordinary keydown
 * events, so "the user typed something" would re-show the button on exactly the devices this
 * exists to hide it on.
 *
 * Known gap, accepted: a tablet in a keyboard case with no pointing device still reports no hover
 * and a coarse pointer, so the hint stays hidden there. Under-reporting a hint is the harmless
 * direction; showing keyboard-shortcut help to a phone user is the annoying one.
 */
export function useHasPhysicalKeyboard(): boolean {
  const supportsHover = useSupportsHover()
  const [finePointer, setFinePointer] = useState(() => finePointing())

  useEffect(() => {
    const mql = window.matchMedia("(any-pointer: fine)")
    const onChange = () => setFinePointer(mql.matches)
    mql.addEventListener("change", onChange)
    return () => mql.removeEventListener("change", onChange)
  }, [])

  return supportsHover || finePointer
}

function finePointing(): boolean {
  return window.matchMedia("(any-pointer: fine)").matches
}
