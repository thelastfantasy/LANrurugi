/**
 * Legacy's `footer.html.tt2` printed an optional tagline (`descstr`, sourced from the project's
 * `package.json` description) above the "Powered by" link. Deliberately dropped here: it is a fixed
 * joke string with no bearing on the library, so the footer is just the attribution line.
 */
export function Footer() {
  return (
    <p className="ip">
      Powered by <a href="https://github.com/thelastfantasy/LANrurugi">LANrurugi.</a>
    </p>
  )
}
