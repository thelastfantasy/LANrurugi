import { toast } from "@/toast"

import { copyText } from "./copyText"

export { copyText }

/** `copyText` with the usual success/failure toast. */
export async function copyWithToast(text: string, copied: string, failed: string): Promise<void> {
  const ok = await copyText(text)
  toast({ heading: ok ? copied : failed, icon: ok ? "info" : "error", hideAfter: 3000 })
}
