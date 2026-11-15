import { useState } from "react"
import { useTranslation } from "react-i18next"
import { NavLink } from "react-router-dom"

import { useServerInfo } from "@/api/hooks"
import { Drawer } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { routes } from "@/lib/routes"

export type NavItem = { to: string; label: string; end?: boolean }

/** The phone form of the top nav: a menu button floating at the page's top-right corner (it takes
 * no row of its own and scrolls away with the page), opening the links in a right-side drawer —
 * finger-sized rows, current page highlighted by each theme's own `.nav-drawer-link.active`. */
export function MobileNav({ links }: { links: NavItem[] }) {
  const { t } = useTranslation()
  const info = useServerInfo()
  const [open, setOpen] = useState(false)
  // The drawer's header is the same brand mark the library page shows, not the word "menu": it is
  // also the fastest way home from a phone, so it links to the library itself.
  const brand = info.data?.name || "LANrurugi"

  return (
    <>
      <IconButton
        icon="fa fa-bars"
        size={30}
        title={t("app.menu")}
        // The same solid accent circle as the drawer's own close button, in every theme.
        className="modal-close-btn"
        onClick={() => setOpen(true)}
        // Inside the page card's top-right corner, level with the library title.
        style={{ position: "absolute", top: 5, right: 14, zIndex: 10, borderRadius: "50%" }}
      />
      <Drawer
        open={open}
        onClose={() => setOpen(false)}
        width={300}
        title={
          <NavLink
            to={routes.library()}
            className="nav-drawer-brand"
            onClick={() => setOpen(false)}
          >
            <img src="/favicon.svg" alt="" className="nav-drawer-brand-logo" />
            <span className="nav-drawer-brand-name">{brand}</span>
          </NavLink>
        }
      >
        <nav style={{ display: "flex", flexDirection: "column", margin: "0 -20px" }}>
          {links.map((link) => (
            <NavLink
              key={link.to}
              to={link.to}
              end={link.end ?? true}
              className="nav-drawer-link"
              onClick={() => setOpen(false)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                minHeight: 48,
                padding: "0 20px",
                fontWeight: 700,
                textDecoration: "none",
                borderBottom: "1px solid rgba(128,128,128,0.2)",
              }}
            >
              <i className="fa fa-caret-right" aria-hidden="true" />
              {link.label}
            </NavLink>
          ))}
        </nav>
      </Drawer>
    </>
  )
}
