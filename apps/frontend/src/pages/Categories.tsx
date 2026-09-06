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

/** A select-then-edit-in-place form: picking a category populates Name/Predicate/Pin/Bookmark-link
 * fields, each auto-saving with a "Saving.../Saved!" indicator, plus an archive checklist. */
export function Categories() {
  const { t } = useTranslation()
  const navigate = useNavigate()
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

  /** Always sends the full name/search/pinned/visible_to_guest quadruple — neither bool field has
   * a "leave as-is" server-side sentinel, so omitting one would silently reset it to false. */
  async function saveDetails(next: { name?: string; search?: string; pinned?: boolean; visibleToGuest?: boolean }) {
    if (!selectedId) return
    setStatus("saving")
    try {
      await sendForm("PUT", `/categories/${selectedId}`, {
        name: next.name ?? name,
        search: next.search ?? search,
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
                        label: c.search ? (
                          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
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
