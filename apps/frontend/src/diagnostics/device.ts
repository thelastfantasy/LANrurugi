/**
 * A stable, per-browser diagnostics id.
 *
 * The forwarding window is instance-wide, so every admin browser contributes events while it is
 * open. Without an id on each event there is no way to tell two devices apart once they are in the
 * tracker (the user agent and screen size collide for identical phones). This is deliberately a
 * random value in localStorage — it identifies the installation, never the person — and it is
 * cleared on logout so the next session does not inherit it.
 */

const STORAGE_KEY = "lanrurugi_diagnostics_device"

/**
 * `crypto.randomUUID` is only available in a secure context, and this app is usually served over
 * plain http on a LAN. `getRandomValues` works there too; `Math.random` is the last resort for a
 * locked-down browser, where a slightly weaker id still beats no id.
 */
function randomId(): string {
  try {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return crypto.randomUUID()
    }
  } catch {
    // Fall through to the byte-based path.
  }
  const bytes = new Uint8Array(16)
  try {
    crypto.getRandomValues(bytes)
  } catch {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256)
    }
  }
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")
}

/** The id, created on first use. Empty when storage is blocked (private mode, hardened browser). */
export function diagnosticsDeviceId(): string {
  try {
    const existing = localStorage.getItem(STORAGE_KEY)
    if (existing) return existing
    const id = randomId()
    localStorage.setItem(STORAGE_KEY, id)
    return id
  } catch {
    return ""
  }
}

export function clearDiagnosticsDevice(): void {
  try {
    localStorage.removeItem(STORAGE_KEY)
  } catch {
    // Nothing to clear when storage itself is unavailable.
  }
}
