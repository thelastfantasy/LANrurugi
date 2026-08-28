import { create } from "zustand";

/** Which bookmarked archive (if any) currently has its `BookmarkHoverGrid` preview open — a
 * global singleton (plain module-level `create()`, unlike `store.ts`'s own per-instance
 * `createStore`+Context pattern, which exists so independent Modal instances don't share state;
 * this needs the opposite: exactly one open preview across the *entire* app at once, regardless
 * of whether it's rendered from the standalone `/bookmarks` page or the Library carousel's
 * "bookmark" mode).
 *
 * Only matters on a touch-only device (`useSupportsHover()` false) — a hover-capable device
 * doesn't use this store's `close()` at all; it manages its own `hoverRect` local state directly
 * (including the scroll-driven re-anchor/close case — see `BookmarkedArchiveHoverCard`'s own
 * docs). On touch, there is no `mouseleave` equivalent: tapping a second card's cover would
 * otherwise leave the first card's preview open forever (nothing ever fires to close it), stacking
 * multiple `BookmarkHoverGrid` overlays. Keying by `archive.arcid` (not a per-instance id) means
 * the same archive appearing in two places at once (e.g. both the carousel and the page below it)
 * shares one open/closed state — a real but harmless edge case, not worth a more elaborate
 * per-instance identity scheme for. */
export interface BookmarkHoverState {
  openArchiveId: string | null;
  open: (archiveId: string) => void;
  close: () => void;
}

export const useBookmarkHoverStore = create<BookmarkHoverState>((set) => ({
  openArchiveId: null,
  open: (archiveId) => set({ openArchiveId: archiveId }),
  close: () => set({ openArchiveId: null }),
}));
