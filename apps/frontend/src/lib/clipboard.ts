/** The app-wide copy helpers: `copyText` for the primitive, `copyWithToast` for the usual pair of
 *  success/failure toasts. Use these instead of `navigator.clipboard` — see `./copyText.ts` for why
 *  the raw API silently fails on a plain-HTTP instance. */

import { toast } from "@/toast"

import { copyText } from "./copyText"

export { copyText }

/** `copyText` with the usual success/failure toast. */
export async function copyWithToast(text: string, copied: string, failed: string): Promise<void> {
  const ok = await copyText(text)
  toast({ heading: ok ? copied : failed, icon: ok ? "info" : "error", hideAfter: 3000 })
}
