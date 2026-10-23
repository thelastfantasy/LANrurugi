import { useTranslation } from "react-i18next"

import { Popover, PopupMenuItem, PopupMenuSeparator } from "@/components/common-ui/Display"
import { CheckboxField, IconButton, RadioGroup, RadioItem } from "@/components/common-ui/Form"

/** Settings gear menu (legacy's `#settings-menu`) — Display Mode, Crop Thumbnails, Hide
 * Completed, Group Tankoubons, each persisted to the same `localStorage` keys legacy uses. */
export function SettingsMenu({
  viewMode,
  setViewMode,
  cropThumbs,
  setCropThumbs,
  hideCompleted,
  setHideCompleted,
  groupbyTanks,
  setGroupbyTanks,
}: {
  viewMode: "thumbnail" | "compact"
  setViewMode: (v: "thumbnail" | "compact") => void
  cropThumbs: boolean
  setCropThumbs: (v: boolean) => void
  hideCompleted: boolean
  setHideCompleted: (v: boolean) => void
  groupbyTanks: boolean
  setGroupbyTanks: (v: boolean) => void
}) {
  const { t } = useTranslation()

  return (
    <Popover
      trigger={
        <IconButton
          icon={<i className="fa fa-cog" style={{ fontSize: 18 }}></i>}
          size={25}
          title={t("library.indexSettings") ?? undefined}
          style={{ border: "none", background: "transparent", marginLeft: 6 }}
        />
      }
    >
      {/* `PopupMenuItem`/`PopupMenuSeparator` render `<li>`s — same wrapping `<ul>` classes
          `PopupMenu` itself uses, since `Popover`'s children land directly inside its own
          `<div>` Popup rather than a list container. */}
      <ul className="m-[.3em] inline-block w-max list-none rounded-[.2em] py-[.25em] ps-0 text-left">
        <PopupMenuItem disabled>
          <i className="fa fa-cog" style={{ width: 18 }}></i> {t("library.indexSettings")}
        </PopupMenuItem>
        <PopupMenuSeparator />
        <PopupMenuItem disabled>
          <i className="fas fa-table" style={{ width: 18 }}></i> {t("library.displayMode")}
        </PopupMenuItem>
        <RadioGroup value={viewMode} onValueChange={setViewMode}>
          <PopupMenuItem onClick={() => setViewMode("thumbnail")}>
            <RadioItem value="thumbnail" size="inherit">
              {t("library.thumbnail")}
            </RadioItem>
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setViewMode("compact")}>
            <RadioItem value="compact" size="inherit">
              {t("library.compact")}
            </RadioItem>
          </PopupMenuItem>
        </RadioGroup>
        <PopupMenuSeparator />
        <PopupMenuItem onClick={() => setCropThumbs(!cropThumbs)}>
          <CheckboxField checked={cropThumbs} onCheckedChange={setCropThumbs} size="inherit">
            {t("library.cropThumbnails")}
          </CheckboxField>
        </PopupMenuItem>
        <PopupMenuItem onClick={() => setHideCompleted(!hideCompleted)}>
          <CheckboxField checked={hideCompleted} onCheckedChange={setHideCompleted} size="inherit">
            {t("library.hideCompletedArchives")}
          </CheckboxField>
        </PopupMenuItem>
        <PopupMenuItem onClick={() => setGroupbyTanks(!groupbyTanks)}>
          <CheckboxField checked={groupbyTanks} onCheckedChange={setGroupbyTanks} size="inherit">
            {t("library.groupTankoubons")}
          </CheckboxField>
        </PopupMenuItem>
      </ul>
    </Popover>
  )
}
