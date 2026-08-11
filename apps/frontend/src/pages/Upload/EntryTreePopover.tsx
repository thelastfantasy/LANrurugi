import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { ArchiveEntryInfo } from "@/api/types";
import { PopupMenu, Tooltip } from "@/components/Display";
import { useMenuPalette } from "@/hooks/useMenuPalette";
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme";

/** Tree built from a flat `ArchiveEntryInfo[]`, splitting each `name` on `/`. `path` is the full
 * path from the root — used as the collapse-state key since two folders can share a leaf name. */
interface EntryTreeNode {
  name: string;
  path: string;
  isPage: boolean;
  isDirectory: boolean;
  children: Map<string, EntryTreeNode>;
}

function buildEntryTree(entries: ArchiveEntryInfo[]): EntryTreeNode {
  const root: EntryTreeNode = { name: "", path: "", isPage: false, isDirectory: true, children: new Map() };
  for (const entry of entries) {
    const parts = entry.name.split("/").filter((p) => p.length > 0);
    if (parts.length === 0) continue;
    let node = root;
    parts.forEach((part, i) => {
      const isLast = i === parts.length - 1;
      let child = node.children.get(part);
      if (!child) {
        child = {
          name: part,
          path: node.path ? `${node.path}/${part}` : part,
          isPage: false,
          isDirectory: !isLast,
          children: new Map(),
        };
        node.children.set(part, child);
      }
      if (isLast) {
        child.isDirectory = !entry.is_regular_file;
        child.isPage = entry.is_page;
      }
      node = child;
    });
  }
  return root;
}

/** Not `PopupMenuItem` — its fixed `px-4` padding stacks with this row's own depth-based indent.
 * Reimplements just the hover highlight, scoped to directory rows only (files aren't clickable). */
function EntryTreeRow({
  child,
  depth,
  isCollapsed,
  onToggle,
}: {
  child: EntryTreeNode;
  depth: number;
  isCollapsed: boolean;
  onToggle: (path: string) => void;
}) {
  const palette = useMenuPalette();
  return (
    <div
      onClick={child.isDirectory ? () => onToggle(child.path) : undefined}
      onMouseEnter={(e) => {
        if (!child.isDirectory) return;
        e.currentTarget.style.background = palette.hoverBg;
        e.currentTarget.style.color = palette.hoverText;
      }}
      onMouseLeave={(e) => {
        if (!child.isDirectory) return;
        e.currentTarget.style.background = "transparent";
        e.currentTarget.style.color = palette.text;
      }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        paddingLeft: 8 + depth * 16,
        paddingRight: 8,
        paddingTop: "0.3em",
        paddingBottom: "0.3em",
        whiteSpace: "nowrap",
        cursor: child.isDirectory ? "pointer" : "default",
      }}
    >
      {child.isDirectory && (
        <i
          className={`fa ${isCollapsed ? "fa-caret-right" : "fa-caret-down"}`}
          aria-hidden="true"
          style={{ opacity: 0.6, width: 10, textAlign: "center" }}
        ></i>
      )}
      <i
        className={
          child.isDirectory ? "fa fa-folder" : child.isPage ? "fa fa-image" : "fa fa-file-o"
        }
        aria-hidden="true"
        style={{ opacity: 0.7, width: 14, textAlign: "center" }}
      ></i>
      <span style={{ opacity: child.isDirectory ? 0.85 : 1 }}>{child.name}</span>
    </div>
  );
}

/** Directories default to collapsed to avoid dumping every nested filename on first open. */
function EntryTreeList({
  node,
  depth,
  collapsed,
  onToggle,
}: {
  node: EntryTreeNode;
  depth: number;
  collapsed: Set<string>;
  onToggle: (path: string) => void;
}) {
  const children = Array.from(node.children.values()).sort((a, b) => {
    if (a.isDirectory !== b.isDirectory) return a.isDirectory ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  return (
    <>
      {children.map((child) => {
        const isCollapsed = child.isDirectory && collapsed.has(child.path);
        return (
          <li key={child.name} style={{ listStyle: "none" }}>
            <EntryTreeRow child={child} depth={depth} isCollapsed={isCollapsed} onToggle={onToggle} />
            {child.children.size > 0 && !isCollapsed && (
              <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
                <EntryTreeList node={child} depth={depth + 1} collapsed={collapsed} onToggle={onToggle} />
              </ul>
            )}
          </li>
        );
      })}
    </>
  );
}

/** Every directory `path` in `tree` — used to seed `collapsed` on open. */
function collectDirectoryPaths(node: EntryTreeNode, out: Set<string>) {
  for (const child of node.children.values()) {
    if (child.isDirectory) {
      out.add(child.path);
      collectDirectoryPaths(child, out);
    }
  }
}

/** Popover showing a side's full internal archive structure (tree), triggered from a button next
 * to its filename. */
export function EntryTreePopover({ entries }: { entries: ArchiveEntryInfo[] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number; width: number }>({
    top: 0,
    left: 0,
    width: 320,
  });
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const tree = useMemo(() => buildEntryTree(entries), [entries]);

  const toggle = () => {
    if (!open && buttonRef.current) {
      const rect = buttonRef.current.getBoundingClientRect();
      // Width capped to the viewport to avoid overflow on narrow/mobile screens.
      const width = Math.min(320, window.innerWidth - 16);
      const maxLeft = window.innerWidth - width - 8;
      setPos({
        top: Math.min(rect.bottom + 4, window.innerHeight - 8),
        left: Math.max(8, Math.min(rect.left, maxLeft)),
        width,
      });
      // Reset to all-collapsed on every fresh open.
      const allDirs = new Set<string>();
      collectDirectoryPaths(tree, allDirs);
      setCollapsed(allDirs);
    }
    setOpen((v) => !v);
  };

  function toggleDirectory(path: string) {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const pageCount = entries.filter((e) => e.is_page).length;
  const fileCount = entries.filter((e) => e.is_regular_file).length;

  return (
    <>
      <Tooltip
        label={
          <div>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>
              {t("Show internal file structure")}
            </div>
            <div style={{ opacity: 0.8 }}>
              {t(
                "{{pages}} page image(s) out of {{files}} total file(s) in this archive (non-page files may include readme/torrent/etc).",
                { pages: pageCount, files: fileCount },
              )}
            </div>
          </div>
        }
      >
        <button
          ref={buttonRef}
          type="button"
          className="stdbtn"
          onClick={toggle}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
            height: 18,
            padding: "0 6px",
            fontSize: FONT_SIZE_XS,
            marginLeft: 6,
            verticalAlign: "middle",
          }}
        >
          <i className="fa fa-sitemap" aria-hidden="true"></i>
          {t("{{pages}}/{{files}} files", { pages: pageCount, files: fileCount })}
        </button>
      </Tooltip>
      {open && (
        <>
          <div
            style={{ position: "fixed", inset: 0, zIndex: 9600 }}
            onClick={() => setOpen(false)}
          ></div>
          <PopupMenu
            style={{
              position: "fixed",
              top: pos.top,
              left: pos.left,
              width: pos.width,
              maxHeight: "60vh",
              overflowY: "auto",
              zIndex: 9601,
              padding: 10,
              fontSize: FONT_SIZE_SM,
            }}
          >
            <EntryTreeList node={tree} depth={0} collapsed={collapsed} onToggle={toggleDirectory} />
          </PopupMenu>
        </>
      )}
    </>
  );
}
