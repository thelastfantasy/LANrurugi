/** Shared TanStack Query key for the login/session status query.
 *
 * Kept in its own module (rather than exported from `SessionProvider.tsx` or `api/client.ts`) so
 * the provider, the refresh-failure invalidation path in `api/client.ts`, and login/logout
 * mutations in `api/hooks.ts` can all share one literal without depending on each other. */
export const SESSION_QUERY_KEY = ["login-status"] as const
