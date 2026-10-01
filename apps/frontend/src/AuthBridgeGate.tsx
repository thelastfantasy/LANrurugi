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
 *
 * Guest-accessible pages (`/`, `/reader/:archiveId`) bridge too: loopback aliases have to be fully
 * equivalent, so a returning user landing on `/` must end up signed in exactly as on the sibling
 * origin. They pass `guest_ok=1`, which makes the "no peer has a session" fallback return to that
 * same page instead of `/login` (see `no_session_fallback` in `auth_bridge.rs`); the resulting
 * `sso_guest=1` marker is consumed once here so the fallback's own reload cannot loop.
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

    // Guest mode makes `/` and `/reader/:archiveId` valid for an unauthenticated caller, but that
    // is no reason to leave a *signed-in* sibling origin unrecognised (loopback aliases must be
    // fully equivalent). So these paths bridge too, and are only spared the `/login` bounce by the
    // marker below plus `guest_ok=1`. Admin-only routes always bridged already.
    const guestPage = isGuestModeEnabled && isGuestAccessiblePath(location.pathname)

    // A guest page whose bridge found no session anywhere comes back here with `sso_guest=1` (see
    // `no_session_fallback` in `auth_bridge.rs`): consume the marker once so this very reload can't
    // re-trigger the round-trip, and strip it so a later refresh can try again. `inFlight` is armed
    // first, so even a re-render triggered by the URL rewrite cannot restart the bridge.
    const params = new URLSearchParams(location.search)
    if (params.get("sso_guest") === "1") {
      inFlight.current = true
      params.delete("sso_guest")
      const query = params.toString()
      window.history.replaceState(
        window.history.state,
        "",
        `${location.pathname}${query ? `?${query}` : ""}${location.hash}`,
      )
      return
    }

    inFlight.current = true

    const returnTo = `${location.pathname}${location.search}${location.hash}`
    const prepareUrl = `/api/auth/bridge/prepare?return_to=${encodeURIComponent(returnTo)}${
      guestPage ? "&guest_ok=1" : ""
    }`
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
