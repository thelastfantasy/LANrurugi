import { useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { sendForm, sendJson } from "../api/client"
import { useArchives, useCategories, useCreateCategory, useTankoubons } from "../api/hooks"
import type { TankoubonMetadata } from "../api/types"
import { ArchiveChecklistItem } from "../components/ArchiveChecklistItem"
import { Tooltip } from "../components/Tooltip"
import { confirmDialog, newCategoryDialog } from "../dialog"
import { routes } from "../routes"
import { FONT_SIZE_9PT, FONT_SIZE_10PT, useApplyTheme } from "../theme"
import { toast } from "../toast"
import { useDocumentTitle } from "../useDocumentTitle"

const BOOKMARK_CATEGORY_STORAGE_KEY = "bookmarkCategoryId"

// Mirrors legacy's `~/LANraragi/templates/category.html.tt2` + `public/js/category.js` — a
// select-then-edit-in-place form (not a create/delete-only list): picking a category from the
// combobox populates Name/Predicate/Pin/Bookmark-link fields, each of which auto-saves on
// change (matching `category.js`'s own `change` event bindings) with a "Saving.../Saved!"
// status indicator, plus a full library-wide archive checklist for static categories. Uses
// `promptDialog`/`confirmDialog` (a real themed popup, `dialog.tsx` — legacy's own real
// equivalent is SweetAlert2's `LRR.showPopUp`/`Swal.fire`) for the new-category/delete flows,
// not the plain `window.prompt`/`window.confirm` an earlier version of this file used: those are
// unstyled native OS dialogs outside the page's own DOM/CSS entirely, not something this app's
// own theme ever actually controls (a coincidental resemblance on some Linux desktop themes was
// mistaken for real theming during an earlier pass — corrected app-wide, see `dialog.tsx`'s own
// docs for the full list of call sites this affected).
// The Tankoubons sub-list is a real, functioning checklist: legacy's own `add_to_category`
// (`$redis->exists($arc_id)`, a generic key-existence check — verified against
// `~/LANraragi/lib/LANraragi/Model/Category.pm`) accepts a `TANK_`-prefixed id just as readily as
// a real archive id, so legacy genuinely supports static categories containing Tankoubons. The
// host-side gap that used to make this permanently empty (`add_archive_to_category` only checking
// `state.repos.archives`) is fixed; this list reuses the exact same
// `PUT`/`DELETE /categories/{id}/{archiveId}` toggle endpoint as the Archives checklist below,
// just passing a tank id instead of an archive id.
export function Categories() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const categories = useCategories()
  const archives = useArchives()
  const tankoubons = useTankoubons()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()

  const [selectedId, setSelectedId] = useState("")
  const [name, setName] = useState("")
  const [search, setSearch] = useState("")
  const [pinned, setPinned] = useState(false)
  const [bookmarkLinked, setBookmarkLinked] = useState(false)
  const [status, setStatus] = useState<"idle" | "saving" | "saved">("idle")

  useApplyTheme()
  useDocumentTitle(t("Modify Categories") ?? undefined)

  const selected = categories.data?.find((c) => c.id === selectedId)
  const isStatic = !!selected && !selected.search

  // Which Tankoubon(s) (if any) each archive is currently folded into — an archive shows here
  // fine even while it's "hidden" from the default grouped Library view for exactly that reason,
  // since this checklist reflects the category's real membership, not a grouped search result
  // (see this file's own module doc comment). Surfaced as a hover tooltip so that isn't
  // mysterious: "this archive's checkbox is checked but I don't see it on the homepage" is
  // otherwise a real, reported point of confusion, not something this checklist should silently
  // paper over by hiding rows or changing what the category itself contains.
  const tanksByArchiveId = new Map<string, TankoubonMetadata[]>()
  for (const tank of tankoubons.data?.result ?? []) {
    for (const memberId of tank.archives) {
      const existing = tanksByArchiveId.get(memberId)
      if (existing) {
        existing.push(tank)
      } else {
        tanksByArchiveId.set(memberId, [tank])
      }
    }
  }

  // Re-syncs the editable fields whenever the *selection itself* changes — the React-recommended
  // "adjusting state when a prop changes" pattern (calling setState directly during render, not
  // inside a `useEffect`, which the project's lint rules flag as cascading-render-prone). Not
  // re-run on every keystroke since `syncedId` only changes when `selectedId` does.
  const [syncedId, setSyncedId] = useState(selectedId)
  if (syncedId !== selectedId) {
    setSyncedId(selectedId)
    setName(selected?.name ?? "")
    setSearch(selected?.search ?? "")
    setPinned(selected?.pinned === 1)
    setBookmarkLinked(!!selected && localStorage.getItem(BOOKMARK_CATEGORY_STORAGE_KEY) === selected.id)
  }

  function refresh() {
    return queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  // Always sends the *full* current name/search/pinned triple, not just the one field that
  // changed — `update_category`'s `pinned` has no "leave as-is" sentinel server-side (a bare
  // `#[serde(default)]` bool, not an `Option`), so omitting it on a plain name edit would
  // silently un-pin an already-pinned category.
  async function saveDetails(next: { name?: string; search?: string; pinned?: boolean }) {
    if (!selectedId) return
    setStatus("saving")
    try {
      await sendForm("PUT", `/categories/${selectedId}`, {
        name: next.name ?? name,
        search: next.search ?? search,
        // `pinned` deserializes as a plain Rust `bool` on the backend
        // (`crates/lanrurugi-api/src/categories.rs::UpdateCategoryParams`) via axum's Form
        // extractor (serde_urlencoded), which only accepts the literal strings "true"/"false" —
        // "1"/"0" fail deserialization with a 422.
        pinned: (next.pinned ?? pinned) ? "true" : "false",
      })
      setStatus("saved")
      await refresh()
    } catch {
      toast({ heading: t("Error modifying category") ?? undefined, icon: "error" })
      setStatus("idle")
    }
  }

  async function handleNewCategory() {
    const result = await newCategoryDialog()
    if (result === null) return
    try {
      const data = await createCategory.mutateAsync(result)
      setSelectedId(data.category_id)
    } catch {
      toast({ heading: t("Error modifying category") ?? undefined, icon: "error" })
    }
  }

  async function handleDelete() {
    if (!selectedId) return
    if (!(await confirmDialog(t("The category will be deleted permanently.") ?? ""))) return
    try {
      await sendJson("DELETE", `/categories/${selectedId}`)
      toast({ text: t("Category deleted!") ?? undefined, icon: "success" })
      setSelectedId("")
      await refresh()
    } catch {
      toast({ heading: t("Error deleting category") ?? undefined, icon: "error" })
    }
  }

  async function handleBookmarkLinkChange(checked: boolean) {
    if (!selectedId) return
    setBookmarkLinked(checked)
    setStatus("saving")
    try {
      if (checked) {
        await sendJson("PUT", `/categories/bookmark_link/${selectedId}`)
        localStorage.setItem(BOOKMARK_CATEGORY_STORAGE_KEY, selectedId)
      } else {
        await sendJson("DELETE", "/categories/bookmark_link")
        localStorage.removeItem(BOOKMARK_CATEGORY_STORAGE_KEY)
      }
      setStatus("saved")
    } catch {
      toast({ heading: t("Error linking bookmark button:") ?? undefined, icon: "error" })
      setStatus("idle")
    }
  }

  async function handleArchiveToggle(archiveId: string, checked: boolean) {
    if (!selectedId) return
    setStatus("saving")
    try {
      await sendJson(checked ? "PUT" : "DELETE", `/categories/${selectedId}/${archiveId}`)
      setStatus("saved")
      await refresh()
    } catch {
      toast({ heading: t("Error modifying category") ?? undefined, icon: "error" })
      setStatus("idle")
    }
  }

  function predicateHelp() {
    toast({
      toastId: "predicateHelp",
      heading: t("Writing a Predicate") ?? undefined,
      text:
        t(
          "Predicates follow the same syntax as searches in the Archive Index. Check the Documentation for more information.",
        ) ?? undefined,
      icon: "info",
      hideAfter: 20000,
    })
  }

  return (
    <div className="ido" style={{ textAlign: "center" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("Categories")}
      </h2>
      <br />
      <br />
      <div style={{ marginLeft: "auto", marginRight: "auto" }}>
        <div className="left-column" style={{ textAlign: "left", fontSize: FONT_SIZE_10PT, width: 400 }}>
          {t("Categories appear at the top of your window when browsing the Library.")}
          <br />
          {t("There are two distinct kinds:")}
          <ul>
            <li>
              <i className="fas fa-2x fa-folder-open" style={{ marginLeft: -30, width: 30 }}></i>{" "}
              {t("Static Categories are arbitrary collections of Archives, where you can add as many items as you want.")}
            </li>
            <li>
              <i className="fas fa-2x fa-bolt" style={{ marginLeft: -25, width: 25 }}></i>{" "}
              {t("Dynamic Categories contain all archives matching a given predicate, and automatically update alongside your library.")}
            </li>
          </ul>
          {t("You can create new categories here or edit existing ones.")}
          <br />
          <br />
          <div style={{ textAlign: "center" }}>
            <input type="button" id="new-category" className="stdbtn" value={t("New Category") ?? undefined} onClick={() => void handleNewCategory()} />
          </div>
          <br />
          {t("Select a category in the combobox below to edit its name, the archives it contains, or its predicate.")}
          <br />
          <b>{t("All your modifications are saved automatically.")}</b>
          <br />
          <br />

          <table>
            <tbody>
              <tr>
                <td>
                  <h2>{t("Category:")}</h2>
                </td>
                <td>
                  <select
                    className="favtag-btn"
                    style={{ fontSize: 20, height: 30 }}
                    value={selectedId}
                    onChange={(e) => setSelectedId(e.target.value)}
                  >
                    <option value="">{t(" -- No Category -- ")}</option>
                    {categories.data?.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </td>
              </tr>
              {selected && (
                <>
                  <tr className="tag-options">
                    <td style={{ textAlign: "right" }}>{t("Name:")}</td>
                    <td>
                      <input value={name} onChange={(e) => setName(e.target.value)} onBlur={() => void saveDetails({ name })} />
                    </td>
                  </tr>
                  {!isStatic && (
                    <tr id="predicatefield" className="tag-options">
                      <td style={{ textAlign: "right" }}>{t("Predicate:")}</td>
                      <td>
                        <input value={search} onChange={(e) => setSearch(e.target.value)} onBlur={() => void saveDetails({ search })} />{" "}
                        <i
                          id="predicate-help"
                          style={{ cursor: "pointer" }}
                          className="fas fa-question-circle"
                          onClick={predicateHelp}
                        ></i>
                      </td>
                    </tr>
                  )}
                  <tr className="tag-options">
                    <td></td>
                    <td>
                      <input
                        id="pinned"
                        name="pinned"
                        className="fa"
                        type="checkbox"
                        checked={pinned}
                        onChange={(e) => {
                          setPinned(e.target.checked)
                          void saveDetails({ pinned: e.target.checked })
                        }}
                      />
                      <label htmlFor="pinned">{t("Pin this Category")}</label>
                    </td>
                  </tr>
                  {isStatic && (
                    <tr id="bookmarklinkfield" className="tag-options">
                      <td></td>
                      <td>
                        <input
                          id="bookmark-link"
                          name="bookmark-link"
                          className="fa"
                          type="checkbox"
                          checked={bookmarkLinked}
                          onChange={(e) => void handleBookmarkLinkChange(e.target.checked)}
                        />
                        <label htmlFor="bookmark-link">{t("Store Bookmarks in this Category")}</label>
                      </td>
                    </tr>
                  )}
                  <tr className="tag-options">
                    <td></td>
                    <td>
                      <input
                        id="delete"
                        type="button"
                        value={t("Delete Category") ?? undefined}
                        className="stdbtn"
                        onClick={() => void handleDelete()}
                      />
                    </td>
                  </tr>
                  <tr className="tag-options">
                    <td></td>
                    <td id="status" style={{ fontSize: FONT_SIZE_9PT }}>
                      {status === "saving" && (
                        <>
                          <i className="fas fa-spin fa-2x fa-compact-disc"></i> {t("Saving your modifications...")}
                        </>
                      )}
                      {status === "saved" && (
                        <>
                          <i className="fas fa-2x fa-check-circle"></i> {t("Saved!")}
                        </>
                      )}
                    </td>
                  </tr>
                </>
              )}
            </tbody>
          </table>
        </div>

        <div className="category-panel right-column" style={{ textAlign: "center", minWidth: 400, width: "60%", height: 500 }}>
          {!selected || !isStatic ? (
            <div
              id="dynamicplaceholder"
              style={{ alignContent: "center", top: 150, position: "relative", marginLeft: "auto", marginRight: "auto", width: "90%" }}
            >
              <i className="fas fa-8x fa-air-freshener"></i>
              <br />
              <br />
              <h2>{t("If you select a Static Category, your archives will appear here so you can add/remove them from the category.")}</h2>
            </div>
          ) : (
            <div id="staticcontent" className="checklist">
              <div id="tankoubonsection" style={{ marginBottom: 10 }}>
                <h3 style={{ marginTop: 0 }}>{t("Tankoubons")}</h3>
                <ul id="tankoubonlist" style={{ listStyle: "none", paddingLeft: 0, margin: 0 }}>
                  {tankoubons.data?.result.length ? (
                    tankoubons.data.result.map((tank) => (
                      <ArchiveChecklistItem
                        key={tank.id}
                        title={tank.name}
                        checked={selected.archives.includes(tank.id)}
                        onChange={(checked) => void handleArchiveToggle(tank.id, checked)}
                      />
                    ))
                  ) : (
                    <li style={{ fontStyle: "italic" }}>{t("No Tankoubons in your library yet.")}</li>
                  )}
                </ul>
              </div>

              <div id="archivesection">
                <h3>{t("Archives")}</h3>
                <ul id="archivelist" style={{ listStyle: "none", paddingLeft: 0, margin: 0 }}>
                  {archives.data?.length ? (
                    archives.data.map((a) => {
                      const memberTanks = tanksByArchiveId.get(a.arcid)
                      const title = memberTanks?.length ? (
                        // Not `anchor="cursor"`: this label has real, clickable links in it, and
                        // cursor-following repositions the bubble on every mousemove over the
                        // trigger — moving the pointer down toward the link (through these very
                        // short, tightly-packed checklist rows) crosses into the next row before
                        // it gets there, closing the tooltip out from under the cursor. The default
                        // `'element'` anchor keeps the bubble in one fixed spot instead, so the
                        // pointer can actually travel to and click the link.
                        <Tooltip
                          label={
                            <>
                              {t("This archive belongs to the following Tankoubon(s):")}
                              {memberTanks.map((tank) => (
                                <div key={tank.id}>
                                  {tank.name} (
                                  <a
                                    href={routes.tankoubonEdit(tank.id)}
                                    onClick={(e) => {
                                      e.preventDefault()
                                      e.stopPropagation()
                                      navigate(routes.tankoubonEdit(tank.id))
                                    }}
                                  >
                                    {tank.id}
                                  </a>
                                  )
                                </div>
                              ))}
                            </>
                          }
                        >
                          {a.title}
                        </Tooltip>
                      ) : (
                        a.title
                      )
                      return (
                        <ArchiveChecklistItem
                          key={a.arcid}
                          title={title}
                          checked={selected.archives.includes(a.arcid)}
                          onChange={(checked) => void handleArchiveToggle(a.arcid, checked)}
                          // `.tankoubon-member-row` — a real per-theme class (each of the 5 real
                          // theme files under `public/legacy/themes/`), not a hardcoded color: this
                          // page is written directly against legacy's own classnames, so a color
                          // needs to swap correctly when the active theme changes, the same way
                          // every other themed color here does (see those files' own docs on this
                          // class for the full reasoning).
                          className={memberTanks?.length ? "tankoubon-member-row" : undefined}
                        />
                      )
                    })
                  ) : (
                    <li style={{ fontStyle: "italic" }}>{t("No Archives in your library yet.")}</li>
                  )}
                </ul>
              </div>
            </div>
          )}
        </div>
        <br />
        <br />
      </div>

      <input type="button" id="return" className="stdbtn" value={t("Return to Library") ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
