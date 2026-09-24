import { useServerInfo } from "@/api/hooks"

/** Mirrors legacy's `footer.html.tt2`: an optional `descstr` tagline above the "Powered by" link. */
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
