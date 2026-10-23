import { create } from "zustand";

/** Which bookmarked archive (if any) has its `BookmarkHoverGrid` preview open — a global
 * singleton since only one preview should be open app-wide at once. */
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
