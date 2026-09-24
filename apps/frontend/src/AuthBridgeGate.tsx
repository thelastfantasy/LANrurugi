import { useEffect, useRef } from "react"
import { useLocation } from "react-router-dom"

import { useAuthConfig, useLoginStatus } from "@/api/hooks"

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
  const loginStatus = useLoginStatus()
  const inFlight = useRef(false)

  useEffect(() => {
    const bridge = config.data
    if (!bridge?.auto_redirect || inFlight.current) return
    // `/login` is the bridge's own fallback destination when no trusted peer has a session, so
    // auto-redirecting away from it would loop forever between peers. Let the local form render.
    if (location.pathname === "/login") return
    if (loginStatus.data?.logged_in !== false) return
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
  }, [config.data, location.hash, location.pathname, location.search, loginStatus.data])

  return null
}
