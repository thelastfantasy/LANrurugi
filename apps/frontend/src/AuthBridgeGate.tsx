import { useEffect, useRef } from "react"
import { useLocation } from "react-router-dom"

import { useAuthConfig } from "@/api/hooks"
import { useSession } from "@/session/SessionProvider"

/**
 * The routes `App.tsx` wraps in `AllowGuest`: Library (`/`) and Reader (`/reader/:archiveId`).
 * A guest-eligible visitor already has a valid local page on these paths, so the SSO bridge must
 * not navigate them away from it. Kept as a small exported predicate so the path list can be unit
 * tested independently of the component's query/browser wiring.
 */
export function isGuestAccessiblePath(pathname: string): boolean {
  return pathname === "/" || /^\/reader\/[^/]+\/?$/.test(pathname)
}

/**
 * Cross-origin login bridge bootstrap. When the backend says this origin is a trusted SSO peer
 * (and not the auth origin) and the user has no local session, ask our own origin for a one-time
 * `state` cookie + start URL, then navigate to another trusted origin. The callback ultimately
 * returns here with local cookies, so the user lands back on this origin already logged in.
 *
 * Purely a browser-navigation concern: this never touches API-token requests and does nothing on
 * an origin that already has a local session, which is where the actual login form lives.
 */
export function AuthBridgeGate() {
  const location = useLocation()
  const config = useAuthConfig()
  const { status: loginStatus, isGuestModeEnabled } = useSession()
  const inFlight = useRef(false)

  useEffect(() => {
    const bridge = config.data
    if (!bridge?.auto_redirect || inFlight.current) return
    // `/login` is the bridge's own fallback destination when no trusted peer has a session, so
    // auto-redirecting away from it would loop forever between peers. Let the local form render.
    if (location.pathname === "/login") return
    if (loginStatus?.logged_in !== false) return
    // Guest mode makes `/` and `/reader/:archiveId` valid for an unauthenticated caller (the
    // `AllowGuest` route guard already lets them render). Auto-redirecting them to a peer origin —
    // which, when no peer has a session, falls back to `{origin}/login?next=...` — is what made an
    // eligible guest's `/` visit bounce to `/login` despite `guest_mode_enabled: true`. Admin-only
    // routes still bridge: guest mode grants no access there, so SSO remains useful.
    if (isGuestModeEnabled && isGuestAccessiblePath(location.pathname)) return
    inFlight.current = true

    const returnTo = `${location.pathname}${location.search}${location.hash}`
    const prepareUrl = `/api/auth/bridge/prepare?return_to=${encodeURIComponent(returnTo)}`
    void fetch(prepareUrl, { credentials: "include" })
      .then(async (response) => {
        if (!response.ok) throw new Error(`prepare failed: ${response.status}`)
        const body = (await response.json()) as { start_url?: string }
        if (!body.start_url) throw new Error("prepare returned no start_url")
        window.location.replace(body.start_url)
      })
      .catch(() => {
        // Keep the current origin usable if the auth origin is unreachable; the user can still use
        // this origin's own login form.
        inFlight.current = false
      })
  }, [config.data, location.hash, location.pathname, location.search, loginStatus, isGuestModeEnabled])

  return null
}
