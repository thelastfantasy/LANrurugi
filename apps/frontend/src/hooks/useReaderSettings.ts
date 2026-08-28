import { useState } from "react";

// Mirrors legacy reader.js's own localStorage-backed settings exactly (key names, string
// serialization of booleans as "true"/"false", fitMode absent = container mode) — verified
// against `~/LANraragi/public/js/reader.js`. Analogous to `useSettings`/`useUpdateSettings` for
// server-side config, but localStorage-backed (no network, so a plain custom hook, not
// react-query) since these are genuinely per-browser reader preferences in legacy too.

/** Keys SCREAMING_SNAKE_CASE, values the real `localStorage` strings (kebab-case, matching this
 * setting's own pre-existing on-disk format) — same reasoning/precedent as `K_BEHAVIOR` further
 * down this file: comparisons read `FIT_MODE.FIT_WIDTH` etc., not a bare string literal, so a typo
 * is a compile error with autocomplete instead of a silently-never-matching new string. */
export const FIT_MODE = {
  CONTAINER: "container",
  FIT_WIDTH: "fit-width",
  FIT_HEIGHT: "fit-height",
} as const;

export type FitMode = (typeof FIT_MODE)[keyof typeof FIT_MODE];

/** `J_SCROLL_UNIT.PERCENT` scales with the viewport (`vh` is exactly this, expressed as a CSS unit
 * rather than a plain number — the two aren't different concepts, just different ways of writing
 * the same relative-to-viewport-height fraction) so `j`'s scroll step stays proportionally the
 * same whether it's a phone or an ultrawide monitor; `PX` is a fixed distance regardless of
 * viewport size, for a reader who wants the *exact same* jump every time. */
export const J_SCROLL_UNIT = {
  PERCENT: "percent",
  PX: "px",
} as const;

export type JScrollUnit = (typeof J_SCROLL_UNIT)[keyof typeof J_SCROLL_UNIT];

/** The full set of `KBehavior` values, named — the single source of truth both the exported type
 * below and `readFromLocalStorage`'s own validation derive from, so adding/renaming/removing an
 * option only ever needs a change in one place instead of the type literal and a hand-written
 * `raw === "..." || ...` validation chain silently drifting apart from each other. Comparisons
 * elsewhere in the reader (`Reader.tsx`'s own `case "k"` and `goTo`/`goToInfiniteScrollPage`)
 * read `K_BEHAVIOR.BACK_BOTTOM` etc. rather than a bare string literal — a typo in a hand-typed
 * `"backBotom"` would silently compile as a *new*, never-matching string (TypeScript doesn't
 * reject an arbitrary string literal being compared against a wider string-typed value), where
 * the same typo referencing this object is a real `Property 'BACK_BOTOM' does not exist` compile
 * error, and the editor's own autocomplete lists the three real property names as soon as
 * `K_BEHAVIOR.` is typed instead of requiring the exact spelling to already be known. Keys are
 * SCREAMING_SNAKE_CASE (this codebase's own constant-naming convention, e.g. `MAX_COLUMNS`/
 * `FRAME_PADDING` in `BookmarkHoverGrid.tsx`) — only the *values* are the real `localStorage`
 * strings (`fitMode`/`jScrollUnit`'s own existing values are lowercase/kebab-case, not
 * SCREAMING_SNAKE_CASE, so the keys and values deliberately don't share one casing here). */
export const K_BEHAVIOR = {
  BACK: "back",
  BACK_BOTTOM: "backBottom",
  BACK_TOP: "backTop",
} as const;

/** What `k` does in non-infinite-scroll mode (infinite-scroll mode always just jumps a whole page
 * via `goToInfiniteScrollPage`, unaffected by this setting — there every page already shares one
 * continuously-scrolling document a plain wheel/trackpad scroll already reads top-to-bottom or
 * back, so there's no incremental-scroll-then-turn behavior to configure in the first place):
 * - `K_BEHAVIOR.BACK`: no incremental scrolling at all — `k` always jumps straight to the previous
 *   page immediately, landing whatever the browser leaves in place with no explicit `scrollTo`
 *   call at all (this reader's original `k` behavior, matching Google Reader's own `k`).
 * - `K_BEHAVIOR.BACK_BOTTOM`: mirrors `j`'s own "scroll down, then advance a page" behavior in the
 *   opposite direction — scroll *up* incrementally (same `jScrollUnit`/`jScrollAmount` step) until
 *   there's nothing further up to scroll, then turn back a page and land at its *bottom*,
 *   continuing a backward read the same way `j` continues a forward one. The default value — most
 *   requests for this setting are for reading a page bottom-to-top symmetrically with `j`, not the
 *   original direct-jump `k`.
 * - `K_BEHAVIOR.BACK_TOP`: same incremental-scroll-then-turn mirroring as `BACK_BOTTOM`, but lands
 *   at the *top* of the previous page instead. */
export type KBehavior = (typeof K_BEHAVIOR)[keyof typeof K_BEHAVIOR];

export interface ReaderSettings {
  hideHeader: boolean;
  mangaMode: boolean;
  doublePageMode: boolean;
  ignoreProgress: boolean;
  showOverlayByDefault: boolean;
  fitMode: FitMode;
  containerWidth: string;
  markersVisible: boolean;
  preloadCount: number;
  autoNextPageInterval: number;
  infiniteScroll: boolean;
  jScrollUnit: JScrollUnit;
  jScrollAmount: number;
  kBehavior: KBehavior;
}

const DEFAULTS: ReaderSettings = {
  hideHeader: false,
  mangaMode: false,
  doublePageMode: false,
  ignoreProgress: false,
  // Deliberately `true`, unlike legacy's own `false` default — a real, requested product
  // decision to always land on the archive overview first, not a port of legacy's own behavior.
  showOverlayByDefault: true,
  fitMode: FIT_MODE.CONTAINER,
  containerWidth: "",
  markersVisible: true,
  preloadCount: 2,
  autoNextPageInterval: 10,
  infiniteScroll: false,
  // Matches the `j`/`" "` scroll step's original hardcoded value (`window.innerHeight * 0.8`)
  // exactly, so introducing this setting doesn't change anyone's existing scroll feel by default.
  jScrollUnit: J_SCROLL_UNIT.PERCENT,
  jScrollAmount: 80,
  kBehavior: K_BEHAVIOR.BACK_BOTTOM,
};

function readBool(key: string, fallback: boolean): boolean {
  const raw = localStorage.getItem(key);
  return raw === null ? fallback : raw === "true";
}

function readFromLocalStorage(): ReaderSettings {
  const fitModeRaw = localStorage.getItem("fitMode");
  const fitMode: FitMode =
    fitModeRaw === FIT_MODE.FIT_WIDTH || fitModeRaw === FIT_MODE.FIT_HEIGHT
      ? fitModeRaw
      : FIT_MODE.CONTAINER;

  return {
    hideHeader: readBool("hideHeader", DEFAULTS.hideHeader),
    mangaMode: readBool("mangaMode", DEFAULTS.mangaMode),
    doublePageMode: readBool("doublePageMode", DEFAULTS.doublePageMode),
    ignoreProgress: readBool("ignoreProgress", DEFAULTS.ignoreProgress),
    showOverlayByDefault: readBool(
      "showOverlayByDefault",
      DEFAULTS.showOverlayByDefault,
    ),
    fitMode,
    containerWidth:
      localStorage.getItem("containerWidth") ?? DEFAULTS.containerWidth,
    markersVisible: readBool("markersVisible", DEFAULTS.markersVisible),
    preloadCount:
      Number(localStorage.getItem("preloadCount")) || DEFAULTS.preloadCount,
    autoNextPageInterval:
      Number(localStorage.getItem("AutoNextPageInterval")) ||
      DEFAULTS.autoNextPageInterval,
    infiniteScroll: readBool("infiniteScroll", DEFAULTS.infiniteScroll),
    jScrollUnit:
      localStorage.getItem("jScrollUnit") === J_SCROLL_UNIT.PX
        ? J_SCROLL_UNIT.PX
        : DEFAULTS.jScrollUnit,
    jScrollAmount:
      Number(localStorage.getItem("jScrollAmount")) || DEFAULTS.jScrollAmount,
    kBehavior: (() => {
      const raw = localStorage.getItem("kBehavior");
      return (Object.values(K_BEHAVIOR) as string[]).includes(raw ?? "")
        ? (raw as KBehavior)
        : DEFAULTS.kBehavior;
    })(),
  };
}

function writeToLocalStorage(partial: Partial<ReaderSettings>) {
  if (partial.hideHeader !== undefined)
    localStorage.setItem("hideHeader", String(partial.hideHeader));
  if (partial.mangaMode !== undefined)
    localStorage.setItem("mangaMode", String(partial.mangaMode));
  if (partial.doublePageMode !== undefined) {
    localStorage.setItem("doublePageMode", String(partial.doublePageMode));
  }
  if (partial.ignoreProgress !== undefined) {
    localStorage.setItem("ignoreProgress", String(partial.ignoreProgress));
  }
  if (partial.showOverlayByDefault !== undefined) {
    localStorage.setItem(
      "showOverlayByDefault",
      String(partial.showOverlayByDefault),
    );
  }
  if (partial.fitMode !== undefined) {
    if (partial.fitMode === FIT_MODE.CONTAINER) {
      localStorage.removeItem("fitMode");
    } else {
      localStorage.setItem("fitMode", partial.fitMode);
    }
  }
  if (partial.containerWidth !== undefined) {
    localStorage.setItem("containerWidth", partial.containerWidth);
  }
  if (partial.markersVisible !== undefined) {
    localStorage.setItem("markersVisible", String(partial.markersVisible));
  }
  if (partial.preloadCount !== undefined) {
    localStorage.setItem("preloadCount", String(partial.preloadCount));
  }
  if (partial.autoNextPageInterval !== undefined) {
    localStorage.setItem(
      "AutoNextPageInterval",
      String(partial.autoNextPageInterval),
    );
  }
  if (partial.infiniteScroll !== undefined) {
    localStorage.setItem("infiniteScroll", String(partial.infiniteScroll));
  }
  if (partial.jScrollUnit !== undefined) {
    localStorage.setItem("jScrollUnit", partial.jScrollUnit);
  }
  if (partial.jScrollAmount !== undefined) {
    localStorage.setItem("jScrollAmount", String(partial.jScrollAmount));
  }
  if (partial.kBehavior !== undefined) {
    localStorage.setItem("kBehavior", partial.kBehavior);
  }
}

export function useReaderSettings(): [
  ReaderSettings,
  (partial: Partial<ReaderSettings>) => void,
] {
  const [settings, setSettings] =
    useState<ReaderSettings>(readFromLocalStorage);

  function update(partial: Partial<ReaderSettings>) {
    writeToLocalStorage(partial);
    setSettings((prev) => ({ ...prev, ...partial }));
  }

  return [settings, update];
}
