/** The build a browser remembers having seen, so the next change can say what happened in
 *  between.
 *
 *  `localStorage`, matching legacy's own `lrrVersion`: "the version I last looked at" is a property
 *  of this browser, not of the instance — a second admin who never opened the app between two
 *  releases should still get their own notes, and one admin reading them should not silence
 *  everyone else's. */
const SEEN_BUILD_KEY = "lanrurugi_seen_build"

/** Just the two fields of `ServerInfo` that identify a build. */
export interface BuildIdentity {
  /** `ServerInfo.version` — `CARGO_PKG_VERSION`. */
  version: string
  /** `ServerInfo.version_name` — the short commit, or `""` when the binary knows no SHA. */
  sha: string
}

export function readSeenBuild(): BuildIdentity | null {
  const raw = localStorage.getItem(SEEN_BUILD_KEY)
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as Partial<BuildIdentity> | null
    if (typeof parsed?.version !== "string" || typeof parsed?.sha !== "string") return null
    return { version: parsed.version, sha: parsed.sha }
  } catch {
    // A hand-edited or half-written value means the same as none. This is a convenience marker;
    // refusing to run because of it would be absurd.
    return null
  }
}

export function writeSeenBuild(build: BuildIdentity): void {
  try {
    localStorage.setItem(SEEN_BUILD_KEY, JSON.stringify(build))
  } catch {
    // Private mode / quota. The only consequence is this browser being asked again next load.
  }
}

/** What the backend is handed as `?since=`: the commit when the build knows one — unambiguous, and
 *  the backend maps it back to a release tag through its own tags list — else the version string.
 *  `null` when the build identifies itself as nothing at all, in which case there is no window to
 *  ask about. */
export function sinceIdentity(build: BuildIdentity): string | null {
  return build.sha || build.version || null
}

/** Whether the browser is looking at a different build than the one it last saw. A first visit
 *  (`seen === null`) is not a change — there is no "before" to report on. */
export function buildChanged(seen: BuildIdentity | null, running: BuildIdentity): boolean {
  if (!seen) return false
  return seen.version !== running.version || seen.sha !== running.sha
}
