import { useTranslation } from "react-i18next"

import { Popover, PopupMenuItem, PopupMenuSeparator } from "@/components/common-ui/Display"
import { CheckboxField, IconButton, RadioGroup, RadioItem } from "@/components/common-ui/Form"

import { type HoverGridPageOrder, useHoverGridPageOrder } from "./useHoverGridPageOrder"
import { useOnlyMatchingBookmarksPreference } from "./useOnlyMatchingBookmarks"

const OPTIONS: HoverGridPageOrder[] = ["bookmarkedAtDesc", "bookmarkedAtAsc", "pageAsc", "pageDesc"]

/** Gear-icon popover controlling how `BookmarkHoverGrid` orders pages within its own popup — same
 * trigger/menu pattern as `Activity/RetentionSettingsInline.tsx`'s gear, distinct from `BookmarksPage.tsx`'s archive-level sort. */
export function HoverGridOrderSettingsMenu() {
  const { t } = useTranslation()
  const [order, setOrder] = useHoverGridPageOrder()
  const [onlyMatching, setOnlyMatching] = useOnlyMatchingBookmarksPreference()

  return (
    <Popover
      trigger={
        <IconButton
          icon={<i className="fa fa-cog" style={{ fontSize: 18 }}></i>}
          size={25}
          title={t("bookmarks.hoverGridOrderLabel") ?? undefined}
          style={{ border: "none", background: "transparent", position: "relative", top: 2 }}
        />
      }
    >
      {/* `PopupMenuItem`/`PopupMenuSeparator` render `<li>`s — same wrapping `<ul>` classes
          `PopupMenu` itself uses, since `Popover`'s children land directly inside its own `<div>`
          Popup rather than a list container. */}
      <ul className="m-[.3em] inline-block w-max list-none rounded-[.2em] py-[.25em] ps-0 text-left">
        <PopupMenuItem disabled>
          <i className="fa fa-cog" style={{ width: 18 }}></i> {t("bookmarks.hoverGridOrderLabel")}
        </PopupMenuItem>
        <PopupMenuSeparator />
        <RadioGroup value={order} onValueChange={setOrder}>
          {OPTIONS.map((option) => (
            <PopupMenuItem key={option} onClick={() => setOrder(option)}>
              <RadioItem value={option} size="inherit">
                {t(`bookmarks.hoverGridOrderOption.${option}`)}
              </RadioItem>
            </PopupMenuItem>
          ))}
        </RadioGroup>
        <PopupMenuSeparator />
        <PopupMenuItem onClick={() => setOnlyMatching(!onlyMatching)}>
          <CheckboxField checked={onlyMatching} onCheckedChange={setOnlyMatching} size="inherit">
            {t("bookmarks.onlyMatchingBookmarks")}
          </CheckboxField>
        </PopupMenuItem>
      </ul>
    </Popover>
  )
}
