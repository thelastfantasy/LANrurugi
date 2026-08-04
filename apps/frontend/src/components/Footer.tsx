import { useServerInfo } from "../api/hooks"

// Mirrors legacy's own `~/LANraragi/templates/footer.html.tt2`, included at the bottom of every
// page there — `p.ip` with an optional `descstr` line (a fixed, per-release tagline, not user
// configurable; see `misc.rs`'s own `version_desc` docs) above the "Powered by" credit link.
export function Footer() {
  const info = useServerInfo()
  const descstr = info.data?.version_desc

  return (
    <p className="ip">
      {descstr && (
        <>
          {descstr}
          <br />
        </>
      )}
      Powered by <a href="https://github.com/thelastfantasy/LANrurugi">LANrurugi.</a>
    </p>
  )
}
