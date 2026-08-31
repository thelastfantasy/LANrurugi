import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import type { ArchiveMetadata } from "@/api/types";
import {
  PopupMenu,
  PopupMenuItem,
  PopupMenuSeparator,
} from "@/components/common-ui/Display";
import { RatingWidget } from "@/components/common-ui/Form";
import { useMenuPalette } from "@/hooks/useMenuPalette";
import { routes } from "@/lib/routes";
import { splitTagsByNamespace } from "@/lib/tagFormat";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme";
import { toast } from "@/toast";

import { type ContextMenuState } from "./types";

/** Ports legacy's right-click menu — same action set and login-gating (Edit/Delete/Rating/
 * Category only shown when logged in). Closes on any outside click or right-click. */
export function ArchiveContextMenu({
  state,
  categories,
  loggedIn,
  liveArchives,
  onClose,
  onToggleCategory,
  onDelete,
  onOpen,
  onRatingChange,
  onToggleSelection,
  isSelected,
  onSetProgress,
}: {
  state: ContextMenuState;
  categories:
    | { id: string; name: string; search: string | null; archives: string[] }[]
    | undefined;
  loggedIn: boolean;
  /** The live, refetch-synced search results — `state.archive` is a stale right-click-time
   * snapshot; looked up by id so the rating row reflects the just-saved value. */
  liveArchives: ArchiveMetadata[];
  onClose: () => void;
  onToggleCategory: (
    categoryId: string,
    archiveId: string,
    currentlyIn: boolean,
  ) => void;
  onDelete: (archiveId: string, isTank: boolean) => void;
  onOpen: (id: string) => void;
  onRatingChange: (
    archiveId: string,
    isTank: boolean,
    rating: string | null,
  ) => void;
  onToggleSelection: (id: string) => void;
  isSelected: boolean;
  /** Mutation lives in the parent, not here — `onClose()` unmounts this menu immediately, which
   * would tear down a locally-owned mutation before its callback ever fires. */
  onSetProgress: (archiveId: string, page: number) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { x, y } = state;
  const archive =
    liveArchives.find((a) => a.arcid === state.archive.arcid) ?? state.archive;
  const isTank = isTankoubonId(archive.arcid);
  const staticCategories = (categories ?? []).filter((c) => !c.search);
  const [categoryMenuOpen, setCategoryMenuOpen] = useState(false);
  const palette = useMenuPalette();

  // Submenu opens on hover; a short close delay absorbs the mouse crossing the gap into it.
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  function openSubmenu(which: "category") {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    setCategoryMenuOpen(which === "category");
  }
  function scheduleCloseSubmenus() {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    closeTimerRef.current = setTimeout(() => {
      setCategoryMenuOpen(false);
    }, 200);
  }

  function copyLink() {
    const url = `${window.location.origin}${routes.reader(archive.arcid)}`;
    navigator.clipboard
      .writeText(url)
      .then(() =>
        toast({
          heading: t("library.linkCopiedToClipboard") ?? undefined,
          icon: "info",
          hideAfter: 3000,
        }),
      )
      .catch(() =>
        toast({
          heading: t("library.failedToCopyLink") ?? undefined,
          icon: "error",
        }),
      );
  }

  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          onClose();
        }}
      />
      <PopupMenu
        style={{
          position: "absolute",
          top: y,
          left: x,
          zIndex: Z_OVERLAY_CONTENT,
        }}
      >
        {loggedIn && (
          <>
            <li
              className="flex items-center justify-center gap-1 px-2 pt-1"
              style={{
                paddingBottom: ".45em",
                borderBottom: `1px solid ${palette.separator}`,
                marginBottom: ".35em",
              }}
            >
              <RatingWidget
                archiveId={archive.arcid}
                tags={archive.tags}
                size={16}
                onChange={(nextTags) => {
                  const tagsByNamespace = splitTagsByNamespace(nextTags);
                  const rating = tagsByNamespace.rating?.[0] ?? null;
                  onRatingChange(archive.arcid, isTank, rating);
                }}
              />
            </li>
          </>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose();
            onOpen(archive.arcid);
          }}
        >
          <i className="fa fa-book-open" style={{ width: 18 }}></i>{" "}
          {t("common.read")}
        </PopupMenuItem>
        {loggedIn && !isTank && archive.pagecount > 0 && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              onSetProgress(
                archive.arcid,
                archive.progress > 0 ? 0 : archive.pagecount,
              );
            }}
          >
            <i
              className={`fa ${archive.progress > 0 ? "fa-eye-slash" : "fa-eye"}`}
              style={{ width: 18 }}
            ></i>{" "}
            {archive.progress > 0
              ? t("library.markAsUnread")
              : t("library.markAsRead")}
          </PopupMenuItem>
        )}
        {loggedIn && !isTank && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              window.location.assign(`/api/archives/${archive.arcid}/download`);
            }}
          >
            <i className="fa fa-download" style={{ width: 18 }}></i>{" "}
            {t("library.download")}
          </PopupMenuItem>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose();
            copyLink();
          }}
        >
          <i className="fa fa-link" style={{ width: 18 }}></i>{" "}
          {t("library.copyLink")}
        </PopupMenuItem>
        {loggedIn && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              onToggleSelection(archive.arcid);
            }}
          >
            <i className="fa fa-check-square" style={{ width: 18 }}></i>{" "}
            {isSelected
              ? t("library.removeFromSelection")
              : t("library.addToSelection")}
          </PopupMenuItem>
        )}
        {loggedIn && (
          <>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose();
                navigate(
                  isTank
                    ? routes.tankoubonEdit(archive.arcid)
                    : routes.edit(archive.arcid),
                );
              }}
            >
              <i className="fa fa-pen" style={{ width: 18 }}></i>{" "}
              {isTank ? t("common.editTankoubon") : t("library.editMetadata")}
            </PopupMenuItem>
            <PopupMenuItem
              style={{ position: "relative" }}
              onMouseEnter={() => openSubmenu("category")}
              onMouseLeave={scheduleCloseSubmenus}
            >
              <i className="fa fa-search-plus" style={{ width: 18 }}></i>{" "}
              {t("library.addToCategory")}
              {categoryMenuOpen && (
                <PopupMenu
                  portal={false}
                  style={{
                    position: "absolute",
                    left: "100%",
                    top: 0,
                    maxHeight: 220,
                    overflowY: "auto",
                  }}
                  onMouseEnter={() => openSubmenu("category")}
                  onMouseLeave={scheduleCloseSubmenus}
                >
                  {staticCategories.length === 0 && (
                    <PopupMenuItem disabled>
                      {t("library.noCategoriesFound")}
                    </PopupMenuItem>
                  )}
                  {staticCategories.map((c) => {
                    const currentlyIn = c.archives.includes(archive.arcid);
                    return (
                      <PopupMenuItem
                        key={c.id}
                        onClick={() =>
                          onToggleCategory(c.id, archive.arcid, currentlyIn)
                        }
                      >
                        <input
                          type="checkbox"
                          readOnly
                          checked={currentlyIn}
                          style={{ verticalAlign: "middle" }}
                        />{" "}
                        {c.name}
                      </PopupMenuItem>
                    );
                  })}
                </PopupMenu>
              )}
            </PopupMenuItem>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose();
                onDelete(archive.arcid, isTank);
              }}
            >
              <i className="fa fa-trash" style={{ width: 18 }}></i>{" "}
              {t("common.delete")}
            </PopupMenuItem>
          </>
        )}
      </PopupMenu>
    </>
  );
}
