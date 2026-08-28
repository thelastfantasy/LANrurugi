import { useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { useTranslation } from "react-i18next"
import { FaBolt } from "react-icons/fa6"
import { useNavigate, useParams } from "react-router-dom"

import { sendForm, sendJson } from "@/api/client"
import { useArchives, useCategories, useCreateCategory, useTankoubons } from "@/api/hooks"
import type { TankoubonMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { Button, Checkbox, Select } from "@/components/common-ui/Form"
import { ArchiveChecklistItem, SearchSyntaxHelp } from "@/components/Display"
import { confirmDialog, newCategoryDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { sortCategories } from "@/lib/utils/sortCategories"
import { FONT_SIZE_MD, FONT_SIZE_SM, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

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
  // The URL is the source of truth for which category is selected (`/config/categories/:categoryId`)
  // — `setSelectedId` further down is a thin wrapper that navigates instead of touching local state
  // directly, so a direct link/bookmark/browser-back to a specific category's URL opens straight
  // into it instead of always landing on "no category selected".
  const { categoryId } = useParams<{ categoryId?: string }>()
  const selectedId = categoryId ?? ""
  function setSelectedId(id: string) {
    navigate(routes.categories(id || undefined))
  }
  const categories = useCategories()
  const archives = useArchives()
  const tankoubons = useTankoubons()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()

  const [name, setName] = useState("")
  const [search, setSearch] = useState("")
  const [pinned, setPinned] = useState(false)
  const [visibleToGuest, setVisibleToGuest] = useState(false)
  const [status, setStatus] = useState<"idle" | "saving" | "saved">("idle")

  useApplyTheme()
  useDocumentTitle(t("app.modifyCategories") ?? undefined)

  const selected = categories.data?.find((c) => c.id === selectedId)
  const isStatic = !!selected && !selected.search
  const sortedCategoryOptions = sortCategories(categories.data ?? [])

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
  // inside a `useEffect`, which the project's lint rules flag as cascading-render-prone). Keyed on
  // `selectedId` plus whether `selected` has actually resolved yet (not just `selectedId` alone) —
  // opening a `/config/categories/:categoryId` URL directly starts with `selectedId` already set
  // from the URL param while `categories.data` is still loading, so `selected` is `undefined` on
  // that first render; without also re-checking once the query resolves, `syncedId` matches
  // `selectedId` from the very first render and the fields never populate at all.
  const syncKey = `${selectedId}:${selected ? "loaded" : "pending"}`
  const [syncedKey, setSyncedKey] = useState(syncKey)
  if (syncedKey !== syncKey) {
    setSyncedKey(syncKey)
    setName(selected?.name ?? "")
    setSearch(selected?.search ?? "")
    setPinned(selected?.pinned === 1)
    setVisibleToGuest(selected?.visible_to_guest === 1)
  }

  function refresh() {
    return queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  // Always sends the *full* current name/search/pinned/visible_to_guest quadruple, not just the
  // one field that changed — neither `pinned` nor `visible_to_guest` has a "leave as-is" sentinel
  // server-side (both bare `#[serde(default)]` bools, not `Option`s), so omitting either on a
  // plain name edit would silently reset it to false.
  async function saveDetails(next: { name?: string; search?: string; pinned?: boolean; visibleToGuest?: boolean }) {
    if (!selectedId) return
    setStatus("saving")
    try {
      await sendForm("PUT", `/categories/${selectedId}`, {
        name: next.name ?? name,
        search: next.search ?? search,
        // `pinned`/`visible_to_guest` deserialize as plain Rust `bool`s on the backend
        // (`crates/lanrurugi-api/src/categories.rs::UpdateCategoryParams`) via axum's Form
        // extractor (serde_urlencoded), which only accepts the literal strings "true"/"false" —
        // "1"/"0" fail deserialization with a 422.
        pinned: (next.pinned ?? pinned) ? "true" : "false",
        visible_to_guest: (next.visibleToGuest ?? visibleToGuest) ? "true" : "false",
      })
      setStatus("saved")
      await refresh()
    } catch {
      toast({ heading: t("common.errorModifyingCategory") ?? undefined, icon: "error" })
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
      toast({ heading: t("common.errorModifyingCategory") ?? undefined, icon: "error" })
    }
  }

  async function handleDelete() {
    if (!selectedId) return
    if (!(await confirmDialog(t("categories.theCategoryWillBeDeleted") ?? "", true))) return
    try {
      await sendJson("DELETE", `/categories/${selectedId}`)
      toast({ text: t("categories.categoryDeleted") ?? undefined, icon: "success" })
      setSelectedId("")
      await refresh()
    } catch {
      toast({ heading: t("categories.errorDeletingCategory") ?? undefined, icon: "error" })
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
      toast({ heading: t("common.errorModifyingCategory") ?? undefined, icon: "error" })
      setStatus("idle")
    }
  }

  return (
    <div className="ido" style={{ textAlign: "center" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("categories.categories")}
      </h2>
      <br />
      <br />
      <div style={{ marginLeft: "auto", marginRight: "auto" }}>
        <div className="left-column" style={{ textAlign: "left", fontSize: FONT_SIZE_SM, width: 400 }}>
          {t("categories.categoriesAppearAtTheTop")}
          <br />
          {t("categories.thereAreTwoDistinctKinds")}
          <ul>
            <li>
              <i className="fas fa-2x fa-folder-open" style={{ marginLeft: -30, width: 30 }}></i>{" "}
              {t("categories.staticCategoriesAreArbitraryCollections")}
            </li>
            <li>
              <i className="fas fa-2x fa-bolt" style={{ marginLeft: -25, width: 25 }}></i>{" "}
              {t("categories.dynamicCategoriesContainAllArchives")}
            </li>
          </ul>
          {t("categories.youCanCreateNewCategories")}
          <br />
          <br />
          <div style={{ textAlign: "center" }}>
            <Button id="new-category" onClick={() => void handleNewCategory()}>
              {t("common.newCategory")}
            </Button>
          </div>
          <br />
          {t("categories.selectACategoryInThe")}
          <br />
          <b>{t("categories.allYourModificationsAreSaved")}</b>
          <br />
          <br />

          <table>
            <tbody>
              <tr>
                <td>
                  <h2>{t("categories.category")}</h2>
                </td>
                <td>
                  <Select
                    size="lg"
                    value={selectedId}
                    onValueChange={setSelectedId}
                    items={[
                      { value: "", label: t("common.NoCategory") },
                      ...sortedCategoryOptions.map((c) => ({
                        value: c.id,
                        // Dynamic categories (search-defined, real `c.search`) get the same
                        // lightning-bolt icon this page's own icon legend above already uses to
                        // mark them — a real SVG glyph reads unambiguously at this small size,
                        // unlike bold text (tried first, dropped: barely legible weight
                        // difference at this font size, easy to miss entirely) or an emoji (no
                        // real "bold" rendering to fall back on either). Guest-visibility below
                        // is an independent axis (a category can be dynamic *and* guest-visible
                        // at once) so it still owns the background-color channel separately.
                        label: c.search ? (
                          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                            {/* `-translate-y-px` — same real-browser "SVG glyph's own visual
                                center sits slightly above adjacent text's x-height even inside a
                                `align-items: center` flex row" gap `Checkbox.tsx`'s own docs
                                cover in more detail; a plain flex `align-items: center` centers
                                the SVG's *box*, not its visually-perceived ink, so a small nudge
                                is still needed on top of that. */}
                            <FaBolt aria-hidden="true" className="relative top-px" />
                            {c.name}
                          </span>
                        ) : (
                          c.name
                        ),
                        itemClassName: c.visible_to_guest === 1 ? "select-item-guest-visible" : undefined,
                      })),
                    ]}
                  />
                </td>
              </tr>
              {selected && (
                <>
                  <tr className="tag-options">
                    <td style={{ textAlign: "right" }}>{t("categories.name")}</td>
                    <td>
                      <input value={name} onChange={(e) => setName(e.target.value)} onBlur={() => void saveDetails({ name })} />
                    </td>
                  </tr>
                  {!isStatic && (
                    <tr id="predicatefield" className="tag-options">
                      <td style={{ textAlign: "right" }}>{t("categories.predicate")}</td>
                      <td>
                        <input value={search} onChange={(e) => setSearch(e.target.value)} onBlur={() => void saveDetails({ search })} />{" "}
                        {/* Same rich React-node syntax reference `dialog.tsx`'s own new-category
                            dialog and the Library page's top search bar both use (`SearchSyntaxHelp`)
                            — this used to be a plain-text `toast()` call with much thinner content
                            (a stale, pre-existing `categories.predicatesFollowTheSameSyntax` key),
                            confirmed live, 2026-08-27, to read as noticeably less helpful side by
                            side with the other two. `Tooltip` (hover, not click-to-toggle) matches
                            this icon's existing `cursor: help`-style affordance better than
                            `ClickPopover` would. */}
                        <Tooltip label={<SearchSyntaxHelp />}>
                          <i
                            id="predicate-help"
                            style={{ cursor: "pointer" }}
                            className="fas fa-question-circle"
                          ></i>
                        </Tooltip>
                      </td>
                    </tr>
                  )}
                  <tr className="tag-options">
                    <td></td>
                    <td>
                      <Checkbox
                        id="pinned"
                        name="pinned"
                        checked={pinned}
                        onCheckedChange={(checked) => {
                          setPinned(checked)
                          void saveDetails({ pinned: checked })
                        }}
                      />{" "}
                      <label htmlFor="pinned">{t("categories.pinThisCategory")}</label>
                    </td>
                  </tr>
                  <tr className="tag-options">
                    <td></td>
                    <td>
                      <Checkbox
                        id="visible-to-guest"
                        name="visible_to_guest"
                        checked={visibleToGuest}
                        onCheckedChange={(checked) => {
                          setVisibleToGuest(checked)
                          void saveDetails({ visibleToGuest: checked })
                        }}
                      />{" "}
                      <label htmlFor="visible-to-guest">{t("categories.visibleToGuest")}</label>
                    </td>
                  </tr>
                  <tr className="tag-options">
                    <td></td>
                    <td>
                      <Button id="delete" onClick={() => void handleDelete()}>
                        {t("categories.deleteCategory")}
                      </Button>
                    </td>
                  </tr>
                  <tr className="tag-options">
                    <td></td>
                    <td id="status" style={{ fontSize: FONT_SIZE_MD }}>
                      {status === "saving" && (
                        <>
                          <i className="fas fa-spin fa-2x fa-compact-disc"></i> {t("categories.savingYourModifications")}
                        </>
                      )}
                      {status === "saved" && (
                        <>
                          <i className="fas fa-2x fa-check-circle"></i> {t("categories.saved")}
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
              <h2>{t("categories.ifYouSelectAStatic")}</h2>
            </div>
          ) : (
            <div id="staticcontent" className="checklist">
              <div id="tankoubonsection" style={{ marginBottom: 10 }}>
                <h3 style={{ marginTop: 0 }}>{t("categories.tankoubons")}</h3>
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
                    <li style={{ fontStyle: "italic" }}>{t("categories.noTankoubonsInYourLibrary")}</li>
                  )}
                </ul>
              </div>

              <div id="archivesection">
                <h3>{t("categories.archives")}</h3>
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
                              {t("categories.thisArchiveBelongsToThe")}
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
                    <li style={{ fontStyle: "italic" }}>{t("categories.noArchivesInYourLibrary")}</li>
                  )}
                </ul>
              </div>
            </div>
          )}
        </div>
        <br />
        <br />
      </div>

      <Button id="return" onClick={() => navigate(routes.library())}>
        {t("common.returnToLibrary")}
      </Button>
    </div>
  )
}
