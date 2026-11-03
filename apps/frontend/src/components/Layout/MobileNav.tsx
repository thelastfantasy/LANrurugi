import { useState } from "react"
import { useTranslation } from "react-i18next"
import { NavLink } from "react-router-dom"

import { Drawer } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"

export type NavItem = { to: string; label: string; end?: boolean }

/** The phone form of the top nav: a menu button floating at the page's top-right corner (it takes
 * no row of its own and scrolls away with the page), opening the links in a right-side drawer —
 * finger-sized rows, current page highlighted by each theme's own `.nav-drawer-link.active`. */
export function MobileNav({ links }: { links: NavItem[] }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

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
      <Drawer open={open} onClose={() => setOpen(false)} title={t("app.menu")} width={300}>
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
