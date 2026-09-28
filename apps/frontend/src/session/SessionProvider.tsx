import { useQuery } from "@tanstack/react-query"
import { createContext, type ReactNode, useContext, useMemo } from "react"

import { fetchLoginStatusWithRefresh } from "@/api/client"
import type { LoginStatus } from "@/api/types"

import { SESSION_QUERY_KEY } from "./queryKey"

/** One place for every session-derived decision the UI makes. Components should ask this rather
 * than each re-deriving `logged_in` / `guest_mode_enabled` from the raw login-status payload. */
export interface SessionContextValue {
  /** Raw status, for callers that genuinely need a field not yet exposed as a derived boolean. */
  status: LoginStatus | undefined
  /** Whether the login-status query has resolved successfully at least once. */
  isSuccess: boolean
  /** Whether the login-status query settled with an error and no usable status. */
  isError: boolean
  /** Whether no cached status is available yet. */
  isPending: boolean
  /** Whether a request is currently in flight (including a background refetch). */
  isFetching: boolean
  /** True only for a real administrator session. */
  isAuthenticated: boolean
  /** Site-wide guest-mode switch as reported by the backend. */
  isGuestModeEnabled: boolean
  /** Unauthenticated but allowed to browse the guest-scoped library/reader. */
  isGuest: boolean
  /** Drives the default-password warning toast. */
  usingDefaultPassword: boolean
  /** Force a fresh login-status check (and therefore a silent token refresh attempt). */
  refresh: () => Promise<unknown>
}

const SessionContext = createContext<SessionContextValue | undefined>(undefined)

/** Owns the single `login-status` query and derives every session/guest boolean from it. */
export function SessionProvider({ children }: { children: ReactNode }) {
  const query = useQuery({
    queryKey: SESSION_QUERY_KEY,
    queryFn: () => fetchLoginStatusWithRefresh<LoginStatus>(),
  })

  const value = useMemo<SessionContextValue>(() => {
    const status = query.data
    const isAuthenticated = status?.logged_in === true
    const isGuestModeEnabled = status?.guest_mode_enabled === true
    return {
      status,
      isSuccess: query.isSuccess,
      isError: query.isError,
      isPending: query.isPending,
      isFetching: query.isFetching,
      isAuthenticated,
      isGuestModeEnabled,
      isGuest: !isAuthenticated && isGuestModeEnabled,
      usingDefaultPassword: status?.using_default_password === true,
      refresh: query.refetch,
    }
  }, [query.data, query.isSuccess, query.isError, query.isPending, query.isFetching, query.refetch])

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
}

/** Session state accessor. Throws when used outside `SessionProvider`, which is a wiring bug
 * rather than a recoverable state — every route/component should sit inside the provider. */
export function useSession(): SessionContextValue {
  const context = useContext(SessionContext)
  if (!context) throw new Error("useSession must be used within a SessionProvider")
  return context
}
