/**
 * Shown while a route's lazily-loaded chunk is on its way.
 *
 * Kept to the legacy spinner idiom the pages themselves use, so a slow chunk looks like the app
 * loading rather than a broken page. Lives in its own module rather than next to the routes: both
 * `App` (the shell) and `Layout` (the inner boundary that keeps a page chunk from blanking the
 * navigation bar) render it, and exporting it from `App` would make those two import each other.
 */
export function RouteFallback() {
  return (
    <p style={{ padding: 16 }}>
      <i className="fa fa-spinner fa-spin" aria-hidden="true"></i>
    </p>
  )
}
