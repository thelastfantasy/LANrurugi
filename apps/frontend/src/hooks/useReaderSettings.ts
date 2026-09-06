import { useState } from "react";

/** Mirrors legacy reader.js's localStorage-backed settings exactly (key names, "true"/"false"
 * string booleans, fitMode absent = container mode). */
export const FIT_MODE = {
  CONTAINER: "container",
  FIT_WIDTH: "fit-width",
  FIT_HEIGHT: "fit-height",
} as const;

export type FitMode = (typeof FIT_MODE)[keyof typeof FIT_MODE];

/** `PERCENT` scales `j`'s scroll step with viewport height; `PX` is a fixed distance regardless
 * of viewport size. */
export const J_SCROLL_UNIT = {
  PERCENT: "percent",
  PX: "px",
} as const;

export type JScrollUnit = (typeof J_SCROLL_UNIT)[keyof typeof J_SCROLL_UNIT];

/** Single source of truth for valid `kBehavior` values, referenced by `readFromLocalStorage`'s
 * validation instead of a hand-written string-literal check. */
export const K_BEHAVIOR = {
  BACK: "back",
  BACK_BOTTOM: "backBottom",
  BACK_TOP: "backTop",
} as const;

/** What `k` does in non-infinite-scroll mode: `BACK` jumps back immediately; `BACK_BOTTOM`/
 * `BACK_TOP` scroll incrementally to the edge first, mirroring `j`'s forward behavior. */
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
  // Deliberately `true`, unlike legacy's `false` default.
  showOverlayByDefault: true,
  fitMode: FIT_MODE.CONTAINER,
  containerWidth: "",
  markersVisible: true,
  preloadCount: 2,
  autoNextPageInterval: 10,
  infiniteScroll: false,
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
