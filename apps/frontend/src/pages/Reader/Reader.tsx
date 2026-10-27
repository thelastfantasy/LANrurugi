import { useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router-dom";

import { ApiError } from "@/api/client";
import {
  fetchRandomArchiveId,
  useAddBookmark,
  useArchiveMetadata,
  useArchivePages,
  useBookmarksForArchive,
  useBookmarksForTankoubon,
  useCategories,
  useClearArchiveNew,
  useDeleteTankoubon,
  useGenerateThumbnails,
  useGenerateThumbnailsForArchives,
  usePageDimensions,
  useRemoveBookmark,
  useSettings,
  useTankoubonFull,
  useUpdateProgress,
  useUpdateSettings,
  useUpdateTankoubonProgress,
} from "@/api/hooks";
import { Menu, MenuItem, MenuSeparator, Tooltip } from "@/components/common-ui/Display";
import { IconButton, ToggleButtonMenu } from "@/components/common-ui/Form";
import {
  ForbiddenPage,
  FormatBadge,
  NotFoundPage,
  StatusIcon,
} from "@/components/Display";
import { Footer } from "@/components/Layout";
import { confirmDialog, promptDialog } from "@/dialog";
import { useDocumentTitle } from "@/hooks/useDocumentTitle";
import { useIsNarrowViewport } from "@/hooks/useIsNarrowViewport";
import {
  clamp,
  computeNextPage,
  computeSpread,
} from "@/hooks/useReaderNavigation";
import {
  FIT_MODE,
  J_SCROLL_UNIT,
  K_BEHAVIOR,
  useReaderSettings,
} from "@/hooks/useReaderSettings";
import { useSupportsHover } from "@/hooks/useSupportsHover";
import { useTankoubonReading } from "@/hooks/useTankoubonReading";
import { routes } from "@/lib/routes";
import { getTagSearchURL } from "@/lib/tagFormat";
import { isBookExtension } from "@/lib/utils/bookFormats";
import { fileInfoText } from "@/lib/utils/fileInfoText";
import {
  getImageCapabilityHeader,
  IMAGE_FORMAT_SUPPORT_HEADER,
} from "@/lib/utils/imageFormatSupport";
import type { PageMeta } from "@/lib/utils/imageMeta";
import {
  fetchContentLengthKb,
  fetchPageMeta,
  fetchResizedPageInfo,
  type ResizedPageInfo,
} from "@/lib/utils/imageMeta";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { useSession } from "@/session/SessionProvider";
import { FONT_SIZE_XS, useApplyTheme } from "@/theme";
import { toast } from "@/toast";
import { TranslationOverlay } from "@/translation/components/TranslationOverlay";
import { TRANSLATION_TARGET_LANGUAGES } from "@/translation/settings";
// Phase 2 on-page translation (`specs/004-ocr-manga-translation`). Rendered as an overlay layer so
// the reader's own image pipeline below is untouched — with translation off, this renders nothing.
import { useTranslationSettings } from "@/translation/useTranslationSettings";

import { ArchiveOverviewOverlay } from "./ArchiveOverviewOverlay";
import { BookmarkButton } from "./BookmarkButton";
import {
  type ArchiveNavState,
  resolveAdjacentArchive,
  setupArchiveNavigation,
} from "./crossArchiveNav";
import {
  type FitAction,
  FitMenuButton,
  readStoredFitAction,
  storeFitAction,
} from "./FitMenuButton";
import { MarkerLayer } from "./MarkerLayer";
import { SettingsOverlay } from "./SettingsOverlay";
import { TankoubonSplitModal } from "./TankoubonSplitModal";

// Loaded only when someone shares, keeping the share dialog out of the reader chunk.
const ShareDialog = lazy(() => import("@/share/ShareDialog").then((m) => ({ default: m.ShareDialog })));

// Port of legacy's reader page (`reader.html.tt2` + `reader.js`) — real DOM structure
// (`#i1`-`#i7`) and CSS classnames from `/legacy/lrr.css`, not Tailwind.

/** Uniform icon size (rem, not em) for every paginator nav link. */
const PAGINATOR_ICON_FONT_SIZE = "1.75rem";

/** Device-derived short-edge cap for the full WebP. Uses DPR and viewport width so a phone can ask
 * for a sharper full image now that tiny handles the first paint, while still bounding the server
 * encode size. Values are clamped server-side too. */
function readerFullShortEdge(): number {
  if (typeof window === "undefined") return 960;
  const dpr = window.devicePixelRatio || 1;
  // Use the larger of viewport-derived and screen-reported width. On some Android Firefox
  // configurations `devicePixelRatio` is reported as 1 while `screen.width` still carries the real
  // physical width; max() avoids falling back to the old 960 default in that case.
  const viewportWidth = window.innerWidth || 0;
  const physicalFromViewport = Math.round(viewportWidth * dpr);
  const physicalFromScreen = Math.max(window.screen.width || 0, window.screen.availWidth || 0);
  const physical = Math.max(physicalFromViewport, physicalFromScreen);
  return Math.max(960, Math.min(1440, physical || 960));
}
/** `.pagecount`'s font-size, scaled up alongside the paginator icons above. */
const PAGINATOR_PAGECOUNT_FONT_SIZE = "1.25rem";
/** Matches `toast.tsx`'s own `AUTO_CLOSE_TIME.info` default. */
const TOAST_DURATION_MS = 5000;

type OverlayKind = "archive" | "settings" | "help" | null;

interface WakeLockSentinelLike {
  release(): Promise<void>;
  addEventListener(type: "release", listener: () => void): void;
}

/** TanStack Query key for a reader's recommendation shortlist — prefetched ahead of time so the
 * boundary panel opens with data already cached (LLM rerank takes seconds). */
const RECS_QUERY_KEY = (id: string) => ["reader-recommendations", id] as const;

/** Fixed 16x16 square badge chip on recommendation-card thumbnails, neutral dark overlay chrome. */
const badgeChipStyle: React.CSSProperties = {
  width: 16,
  height: 16,
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  fontSize: 11,
  lineHeight: 1,
  background: "rgba(0,0,0,0.55)",
  color: "#fff",
  borderRadius: 4,
};

/** Whether reading progress for this session belongs in `localStorage` (`${archiveId}-reader`)
 * rather than the server: not logged in at all, or `localprogress` is on but this isn't a real
 * authenticated+`authprogress` session. Shared by both the write side (below) and the read side
 * (`currentPage`'s own initial-value fallback) so they can't drift apart on the condition. */
function usesLocalReaderProgress(
  loggedIn: boolean,
  settings: { localprogress?: boolean; authprogress?: boolean } | undefined,
): boolean {
  return !loggedIn || (!!settings?.localprogress && !(settings?.authprogress && loggedIn));
}

/** Inline key-cap styling for the help panel's keyboard shortcuts. */
function Key({ children }: { children: React.ReactNode }) {
  return (
    <code
      style={{
        padding: "1px 5px",
        background: "rgba(0,0,0,0.08)",
        borderRadius: 3,
        fontWeight: "bold",
      }}
    >
      {children}
    </code>
  );
}

export function Reader() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { archiveId = null } = useParams<{ archiveId: string }>();
  useApplyTheme();

  const isTank = isTankoubonId(archiveId ?? "");
  const singleMetadata = useArchiveMetadata(isTank ? null : archiveId);
  const singlePages = useArchivePages(isTank ? null : archiveId);
  const tankReading = useTankoubonReading(isTank ? archiveId : null);
  const tankFull = useTankoubonFull(isTank ? archiveId : null);
  const metadata = isTank ? tankReading.metadata : singleMetadata;
  const tankSplitCandidates = tankFull.data?.result.full_data.filter((m) => m.has_split_suggestion) ?? []
  const pages = isTank ? tankReading.pages : singlePages;
  useDocumentTitle(metadata.data?.title);
  const settings = useSettings();
  const updateSettings = useUpdateSettings();
  const { isAuthenticated: loggedIn } = useSession();
  const categories = useCategories();
  const singleBookmarks = useBookmarksForArchive(isTank ? null : archiveId);
  const tankBookmarks = useBookmarksForTankoubon(isTank ? archiveId : null);
  const bookmarks = isTank ? tankBookmarks : singleBookmarks;
  const addBookmark = useAddBookmark();
  const removeBookmark = useRemoveBookmark();
  const updateProgress = useUpdateProgress(isTank ? null : archiveId);
  const updateTankoubonProgress = useUpdateTankoubonProgress(
    isTank ? archiveId : null,
  );
  const deleteTankoubon = useDeleteTankoubon();
  const generateThumbnails = useGenerateThumbnails(
    isTank ? "" : (archiveId ?? ""),
  );
  const generateThumbnailsForArchives = useGenerateThumbnailsForArchives();
  const [readerSettings, updateReaderSettings] = useReaderSettings();
  const totalPages = pages.data?.pages.length ?? 0;
  // `startPage`/`startWithOverview` are also read further down (query-string cleanup effect,
  // overlay initial state) — declared here, ahead of everything below that needs `currentPage`,
  // rather than duplicated, since JS scoping only requires the declaration precede every use, not
  // that every use sit immediately after it.
  const params = new URLSearchParams(window.location.search);
  const startPage = Number(params.get("p")) || null;
  const startWithOverview = params.get("overview") === "1";
  const [pageOverride, setPageOverride] = useState<number | null>(startPage);
  const localReaderProgress =
    archiveId && usesLocalReaderProgress(loggedIn, settings.data)
      ? Number(localStorage.getItem(`${archiveId}-reader`))
      : 0;
  // Moved ahead of `useTranslationSettings` below specifically so the Tankoubon case (next) can
  // resolve the real archive a page belongs to synchronously, during render, rather than needing
  // an effect to sync it a render late (react-hooks' `set-state-in-effect` correctly flags that
  // pattern when the value is actually available synchronously, as it is here).
  const currentPage = clamp(
    pageOverride ??
      (readerSettings.ignoreProgress
        ? 1
        : Math.max(localReaderProgress || metadata.data?.progress || 1, 1)),
    1,
    totalPages || 1,
  );
  // The archive the currently-displayed left page actually belongs to, for scoping the translation
  // enabled switch. A single-archive read is immediate (`archiveId` already *is* the real archive).
  // Tankoubon reading resolves through `getArchiveForPage` (global page → real member archive) —
  // see `markerTarget`'s own note further down for why the route's `archiveId` itself (a tank id
  // while reading a Tankoubon) can't be used directly for translation.
  const translationArchiveId = isTank
    ? tankReading.getArchiveForPage(currentPage)?.arcId ?? null
    : archiveId;
  // `null` whenever translation is off, unconfigured, or the settings fetch failed — the overlay
  // renders nothing in all three cases (`specs/004-ocr-manga-translation` FR-007).
  const [translationSettings, translationScope, toggleTranslationEnabled, setTranslationTargetLanguage] =
    useTranslationSettings(translationArchiveId);
  // These two `useCallback`s MUST live before any of this component's early `return`s below
  // (Rules of Hooks — every hook must run unconditionally on every render). The URLs they need
  // are only computable once `pages.data` exists, which is exactly what the early-return guards
  // wait for — so the values live in a ref, kept in sync (a plain assignment, not a hook) from
  // wherever they're actually computed further down, rather than being passed as `useCallback`
  // dependencies the way an earlier version of this code did (that version was reachable only
  // sometimes, which is what made React see a different hook count between renders and crash the
  // whole page — see the fix note near `originalLeftUrl`/`originalRightUrl` below).
  const originalPageUrlsRef = useRef<{ left?: string; right: string | null }>({
    left: undefined,
    right: null,
  });
  const loadLeftPageImage = useCallback(async () => {
    const url = originalPageUrlsRef.current.left;
    if (!url) throw new Error("left page image is not available");
    const img = new Image();
    img.decoding = "async";
    img.src = url;
    await img.decode();
    return img;
  }, []);
  const loadRightPageImage = useCallback(async () => {
    const url = originalPageUrlsRef.current.right;
    if (!url) throw new Error("right page image is not available");
    const img = new Image();
    img.decoding = "async";
    img.src = url;
    await img.decode();
    return img;
  }, []);

  const clearArchiveNew = useClearArchiveNew();
  const clearArchiveNewRef = useRef(clearArchiveNew.mutate);
  clearArchiveNewRef.current = clearArchiveNew.mutate;
  const newBadgeMode = settings.data?.newbadgemode;
  useEffect(() => {
    if (!archiveId || isTank || !newBadgeMode) return;
    if (newBadgeMode === "until_opened") clearArchiveNewRef.current(archiveId);
  }, [archiveId, isTank, newBadgeMode]);

  const openedByDefaultSetting = useRef(
    readerSettings.showOverlayByDefault || startWithOverview,
  );
  const [showTankSplit, setShowTankSplit] = useState(false)
  const [overlay, setOverlay] = useState<OverlayKind>(
    readerSettings.showOverlayByDefault || startWithOverview ? "archive" : null,
  );

  useEffect(() => {
    if (!startPage && !startWithOverview) return;
    const url = new URL(window.location.href);
    url.searchParams.delete("p");
    url.searchParams.delete("overview");
    window.history.replaceState(null, "", url);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [widespreads, setWidespreads] = useState<Record<number, boolean>>({});
  // 017: what the share dialog was opened for — the archive on screen, or the whole tankoubon.
  const [shareItem, setShareItem] = useState<{ kind: "archive" | "tankoubon"; id: string } | null>(null);
  const [pageDimensions, setPageDimensions] = useState<
    Record<number, { width: number; height: number }>
  >({});
  const [pageSizesKb, setPageSizesKb] = useState<Record<number, number>>({});
  // Bytes actually delivered for the displayed (converted) page, from the browser's own resource
  // timing — the file-info HEAD deliberately reports the original entry's size instead.
  const [servedSizesKb, setServedSizesKb] = useState<Record<number, number>>({});
  useEffect(() => {
    // Resource timing keeps 250 entries by default; a long reading session would fill it and
    // leave later pages without a served size.
    const clear = () => performance.clearResourceTimings();
    performance.addEventListener("resourcetimingbufferfull", clear);
    return () => performance.removeEventListener("resourcetimingbufferfull", clear);
  }, []);
  const [resizedPageInfo, setResizedPageInfo] = useState<
    Record<number, ResizedPageInfo | null>
  >({});
  const [markerPlacementMode, setMarkerPlacementMode] = useState(false);
  const supportsHover = useSupportsHover();
  // Shared by both toolbars, so changing the primary action in one updates the other too.
  const [fitAction, setFitAction] = useState<FitAction>(readStoredFitAction);
  /**
   * Fit-height used to be a flat `90vh` (98vh with the header hidden), which ignores the reader's
   * own chrome — title, toolbar, the page-info line, the footer — so the page still ended up taller
   * than the viewport and kept the scrollbar that this mode exists to remove. Measure the space that
   * is actually free instead: everything above the image area plus everything below it, subtracted
   * from the viewport. Both of those are independent of the image's own height, so the measurement
   * cannot chase itself.
   */
  const [fitHeightPx, setFitHeightPx] = useState<number | null>(null);
  const isMobileToolbarViewport = useIsNarrowViewport("(max-width: 768px)");
  const [navState, setNavState] = useState<ArchiveNavState>({
    ids: [],
    index: -1,
  });
  const [autoNextActive, setAutoNextActive] = useState(
    () => sessionStorage.getItem("autoNextPage") === "true",
  );
  const [autoNextCountdown, setAutoNextCountdown] = useState(
    () => Math.trunc(readerSettings.autoNextPageInterval) || 10,
  );
  const containerRef = useRef<HTMLDivElement>(null);
  const leftImgRef = useRef<HTMLImageElement>(null);
  const rightImgRef = useRef<HTMLImageElement>(null);
  const wakeLockRef = useRef<WakeLockSentinelLike | null>(null);
  const infiniteScrollRootRef = useRef<HTMLDivElement>(null);
  const infiniteScrollCurrentPageRef = useRef<number | null>(null);
  const infiniteScrollResumedRef = useRef(false);
  const infiniteScrollResumePageRef = useRef<number | null>(null);
  const [archiveTransition, setArchiveTransition] = useState<{
    direction: "prev" | "next";
    /** Fetched from `/api/reader/recommendations/{id}` while the overlay shows. `null` = still
     * loading / model not ready / fetch failed (the panel then shows nothing to pick). */
    recommendations:
      | {
          archive_id: string;
          title: string;
          score: number;
          isnew: boolean;
          is_read: boolean;
          is_tank: boolean;
          extension?: string;
        }[]
      | null;
  } | null>(null);
  const imageAreaRef = useRef<HTMLDivElement>(null);
  /**
   * Fit-height used to be a flat `90vh` (98vh with the header hidden), which ignores the reader's own
   * chrome — title, toolbar, the page-info line, the footer — so the page still ended up taller than
   * the viewport and kept the scrollbar this mode exists to remove. Measure the space that is
   * actually free instead: everything above the image area plus everything below it, subtracted from
   * the viewport. Neither depends on the image's own height, so the measurement cannot chase itself.
   */
  useEffect(() => {
    if (readerSettings.fitMode !== FIT_MODE.FIT_HEIGHT || isFullscreen) {
      return;
    }
    let frame = 0;
    /** Height of everything painted below the image area — the page info line, the bottom toolbar,
     *  the footer — measured element by element.
     *
     *  `document.documentElement.scrollHeight - areaBottom` is the obvious formula and it is wrong:
     *  once the content fits, `scrollHeight` is clamped up to the viewport, so the "space below"
     *  degenerates into the image's own height and every re-measure (the ResizeObserver fires on
     *  each size change) shrinks the page by another few pixels — the endless shrink loop readers
     *  saw in fit-height. Summing the siblings that actually sit below the area has no such
     *  feedback: it does not depend on the image's height at all.
     */
    const bottomChrome = (area: HTMLElement): number => {
      let total = 0;
      let node: HTMLElement | null = area;
      while (node && node !== document.body) {
        const parent: HTMLElement | null = node.parentElement;
        if (!parent) break;
        const nodeBottom = node.getBoundingClientRect().bottom;
        for (const sibling of Array.from(parent.children)) {
          if (sibling === node) continue;
          const box = sibling.getBoundingClientRect();
          if (box.height > 0 && box.top >= nodeBottom - 1) {
            // `getBoundingClientRect` excludes margins, and the toolbar/footer margins are exactly
            // what made the sum a few pixels short (page still scrollable by ~7px).
            const style = getComputedStyle(sibling);
            total +=
              box.height +
              parseFloat(style.marginTop || "0") +
              parseFloat(style.marginBottom || "0");
          }
        }
        node = parent;
      }
      return total;
    };
    const measure = () => {
      const area = imageAreaRef.current;
      if (!area) return;
      const box = area.getBoundingClientRect();
      // The slack covers what a siblings-only walk cannot see: the ancestors' bottom padding and
      // the footer's own margins (~15px measured, 20 leaves a few pixels of headroom so rounding
      // never reintroduces a scrollbar).
      const available = Math.round(
        window.innerHeight - box.top - bottomChrome(area) - 20,
      );
      setFitHeightPx(available > 160 ? available : null);
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    schedule();
    const settled = window.setTimeout(schedule, 250);
    window.addEventListener("resize", schedule);
    const observer = new ResizeObserver(schedule);
    if (imageAreaRef.current) observer.observe(imageAreaRef.current);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(settled);
      window.removeEventListener("resize", schedule);
      observer.disconnect();
    };
  }, [readerSettings.fitMode, readerSettings.hideHeader, readerSettings.infiniteScroll, isFullscreen]);

  const lastSpreadHeightRef = useRef<number | null>(null);
  const lastFileInfoRef = useRef<string | null>(null);
  // Tracks pages whose Content-Length has already been requested, so the UI can show the file-size
  // part of the file-info bar while the actual image is still transferring (it used to wait for
  // `img.onload`, then issue a HEAD request on top of that — on mobile that made the text below a
  // page appear only after the whole large image had finished downloading).
  const prefetchedPageSizesRef = useRef<Set<number>>(new Set());
  const [pageTinyBlobs, setPageTinyBlobs] = useState<Record<number, string>>({});
  // Per page: whether the visible `<img>` has decoded pixels yet. Until it has, the streaming
  // element keeps showing the *previous* page (or nothing, if its box is still unknown) — which on
  // a slow link looked like paging had stopped working. The tiny preview is stacked on top until
  // this flips, so the reader always sees the right page, just at preview resolution first.
  const [mainReady, setMainReady] = useState<Record<number, boolean>>({});
  const lastDimsRef = useRef<{ width: number; height: number } | null>(null);
  const [pageTinyDims, setPageTinyDims] = useState<
    Record<number, { width: number; height: number }>
  >({});
  /** False until the first page bitmap has painted: only then is there a previous bitmap the
   *  reader should keep on screen while the next page loads. */
  const displayedAnythingRef = useRef(false);
  /** Detached `<img>` elements warming neighbouring pages in the browser's image/HTTP cache. */
  const warmImagesRef = useRef<Map<string, HTMLImageElement>>(new Map());
  /** Per page: the in-flight/resolved metadata HEAD (box + will-the-server-convert). */
  const pageMetaRef = useRef<Map<number, Promise<PageMeta | null>>>(new Map());
  const pageTinyBlobsRef = useRef<Map<number, string>>(new Map());
  const pageTinyDimsRef = useRef<Map<number, { width: number; height: number }>>(new Map());
  const pageTinyFetchesRef = useRef<Map<number, AbortController>>(new Map());
  const currentPageNumbersRef = useRef<number[]>([]);

  if (
    infiniteScrollResumePageRef.current === null &&
    !metadata.isLoading &&
    !pages.isLoading
  ) {
    infiniteScrollResumePageRef.current = currentPage;
  }

  const infiniteScrollResumeDimensions = usePageDimensions(
    archiveId,
    infiniteScrollResumePageRef.current ?? 0,
    readerSettings.infiniteScroll,
  );

  const spread = readerSettings.infiniteScroll
    ? { left: currentPage, right: null }
    : computeSpread(
        currentPage,
        totalPages,
        readerSettings.doublePageMode,
        readerSettings.mangaMode,
        (page) => widespreads[page],
      );

  const currentSpreadLoaded =
    (pageDimensions[spread.left] !== undefined &&
      (spread.right === null || pageDimensions[spread.right] !== undefined)) ||
    (spread.right === null
      ? !!pageTinyBlobs[spread.left]
      : !!pageTinyBlobs[spread.left] && !!pageTinyBlobs[spread.right]);

  const infiniteScrollResumeReady =
    infiniteScrollResumePageRef.current === null ||
    infiniteScrollResumePageRef.current <= 1 ||
    infiniteScrollResumeDimensions.isSuccess;

  /** The URL a plain `<img>` should request for a page: the address `/files` generated (it carries
   * `optimize=1` and any patch marker) plus the variant the reader wants and the queue priority.
   * A `<img>` cannot send `X-LRR-Priority`, but the backend reads the same hint from
   * `?priority=` (`PageParams::priority`), and the codec capabilities already travel in the
   * `lrr_img_support` cookie — which is what lets the visible page stream instead of waiting for a
   * full Blob, i.e. paint its first frame before the whole file has arrived. */
  const pageStreamUrl = useCallback(
    (page: number) => {
      const url = pages.data?.pages[page - 1]?.url;
      if (!url) return undefined;
      const parsed = new URL(url, window.location.origin);
      parsed.searchParams.set("max_short_edge", String(readerFullShortEdge()));
      // Deliberately no `priority` parameter: the prefetch and the visible `<img>` have to request
      // the *same* URL or the browser cannot reuse the warmed cache entry. Speculative requests are
      // marked with the `X-LRR-Priority` header instead, which the server reads too.
      return parsed.toString();
    },
    [pages.data],
  );

  /**
   * Warms a neighbouring page through a detached `new Image()`: the browser's own image loader and
   * HTTP cache, requesting exactly the URL the visible `<img>` will use, so the bytes are reused
   * rather than downloaded twice. Nothing is held in JS (no Blob, no fetched body), and
   * `fetchPriority = "low"` tells the browser to keep speculative work behind the page the reader is
   * actually looking at. Cancelling is `src = ""`.
   */
  const warmPage = useCallback(
    (page: number) => {
      const url = pageStreamUrl(page);
      if (!url || warmImagesRef.current.has(url)) return;
      const image = new Image();
      image.fetchPriority = "low";
      image.decoding = "async";
      image.src = url;
      warmImagesRef.current.set(url, image);
    },
    [pageStreamUrl],
  );

  /**
   * Asks the server what a page will cost before deciding to fetch its preview placeholder: one
   * metadata-only HEAD per page (deduplicated), which reports the page's box and whether the bytes
   * will be the archive's own. A passthrough page renders from its own first frame in a fraction of
   * the time a placeholder needs (measured: ~0.24s for a 20 MB animation on Fast 4G versus ~21s for
   * a cold `variant=tiny` of the same page), so the placeholder is only requested for pages the
   * server has to convert. The HEAD carries the same `max_short_edge` the visible `<img>` uses, so
   * the answer describes the request the reader is actually making.
   */
  const ensurePageMeta = useCallback(
    (page: number): Promise<PageMeta | null> => {
      const existing = pageMetaRef.current.get(page);
      if (existing) return existing;
      const url = pageStreamUrl(page);
      const promise = url
        ? fetchPageMeta(url).then((meta) => {
            if (meta && meta.width > 0 && meta.height > 0) {
              // The same box the preview reports, but without decoding the page.
              setPageTinyDims((prev) =>
                prev[page] ? prev : { ...prev, [page]: { width: meta.width, height: meta.height } },
              );
            }
            return meta;
          })
        : Promise.resolve(null);
      pageMetaRef.current.set(page, promise);
      return promise;
    },
    [pageStreamUrl, setPageTinyDims],
  );

  // Fetches a much smaller WebP preview for the current page only. It is what the reader sees until
  // the streaming page has pixels to paint (and it reports that page's box), so it is intentionally
  // excluded from file-info metadata.
  const fetchTinyPreview = useCallback(
    (page: number) => {
      const url = pages.data?.pages[page - 1]?.url;
      if (!url) return;
      if (pageTinyBlobsRef.current.has(page)) return;
      const existing = pageTinyFetchesRef.current.get(page);
      if (existing) return;
      const controller = new AbortController();
      pageTinyFetchesRef.current.set(page, controller);
      const tinyUrl = new URL(url, window.location.origin);
      tinyUrl.searchParams.set("variant", "tiny");
      // The preview response reports the *full-size* page's dimensions (`x-lrr-preview-*`), which
      // the reader turns into the `<img width height>` box. Without this the server has to guess
      // that box from its own 1064/960 default while the full request asks for
      // `readerFullShortEdge()` (up to 1440) — same box only while the source is narrower than
      // both, then the image resizes under the reader when the full one lands. The preview image
      // itself is unaffected: previews are always produced at `MAX_LONG_EDGE_PREVIEW`.
      tinyUrl.searchParams.set("max_short_edge", String(readerFullShortEdge()));
      fetch(tinyUrl.toString(), {
        headers: {
          "X-LRR-Priority": "preview",
          [IMAGE_FORMAT_SUPPORT_HEADER]: getImageCapabilityHeader(),
        },
        signal: controller.signal,
      })
        .then(async (res) => {
          if (!res.ok) throw new Error(`preview fetch failed: ${res.status}`);
          const width = Number(res.headers.get("x-lrr-preview-width"));
          const height = Number(res.headers.get("x-lrr-preview-height"));
          if (width > 0 && height > 0) {
            pageTinyDimsRef.current.set(page, { width, height });
            setPageTinyDims(Object.fromEntries(pageTinyDimsRef.current));
          }
          return res.blob();
        })
        .then((blob) => {
          if (pageTinyFetchesRef.current.get(page) !== controller) return;
          pageTinyFetchesRef.current.delete(page);
          const blobUrl = URL.createObjectURL(blob);
          pageTinyBlobsRef.current.set(page, blobUrl);
          setPageTinyBlobs(Object.fromEntries(pageTinyBlobsRef.current));
        })
        .catch(() => {
          if (pageTinyFetchesRef.current.get(page) !== controller) return;
          pageTinyFetchesRef.current.delete(page);
        });
    },
    [pages.data, setPageTinyBlobs, setPageTinyDims],
  );

  useEffect(() => {
    document.body.classList.toggle(
      "infinite-scroll",
      readerSettings.infiniteScroll,
    );
    return () => document.body.classList.remove("infinite-scroll");
  }, [readerSettings.infiniteScroll]);

  // Always (re)fetch the currently visible page(s) as high-priority `current` requests. If a
  // prefetch was already in flight for the same page, it is upgraded/cancelled and replaced by the
  // high-priority request, which is what makes rapid multi-page jumps reach the final page first.
  useEffect(() => {
    if (readerSettings.infiniteScroll) return;
    const pageNumbers =
      spread.right === null ? [spread.left] : [spread.left, spread.right];
    currentPageNumbersRef.current = pageNumbers;
    // First page of a fresh archive: there is no previous bitmap to keep visible, so waiting for
    // the 300ms settle would leave a blank img. Start preview+full immediately for the initial page.
    if (!displayedAnythingRef.current && pageNumbers.length > 0) {
      for (const page of pageNumbers) {
        if (page < 1) continue;
        fetchTinyPreview(page);
      }
    }
  }, [spread.left, spread.right, fetchTinyPreview, readerSettings.infiniteScroll]);

  // Tiny previews are only for the page the user actually stops on. Starting one on every
  // intermediate page during a rapid flip would add extra small requests that are almost always
  // wasted. Wait for the current page to settle before creating it.
  // Asks the browser when the visible pages' bitmaps are actually paintable. `decode()` resolves
  // once the image can be drawn, which is the earliest signal a streaming (partially received)
  // image offers — `onLoad` only fires when the whole file is in. Readiness is only ever flipped
  // *to* true: a page whose dimensions are already known never shows the preview in the first
  // place, so there is nothing to reset on a page turn.
  useEffect(() => {
    if (readerSettings.infiniteScroll) return;
    const shown = spread.right === null ? [spread.left] : [spread.left, spread.right];
    let cancelled = false;
    const markReady = (page: number) =>
      setMainReady((prev) => (prev[page] ? prev : { ...prev, [page]: true }));
    const elements = [leftImgRef.current, rightImgRef.current].filter(
      (el): el is HTMLImageElement => !!el,
    );
    const timers: number[] = [];
    for (const el of elements) {
      const page = Number(el.dataset.page ?? "0");
      if (!shown.includes(page)) continue;
      if (el.complete) {
        markReady(page);
        continue;
      }
      // `decode()` only resolves once the *whole* file is in for a streaming image (≈40 s for a
      // 20 MB page on Fast 4G), which would leave the preview covering it for that long.
      // `naturalWidth > 0` instead flips as soon as the header arrives and the browser can start
      // painting what it has, which is the moment the preview should get out of the way.
      const timer = window.setInterval(() => {
        if (cancelled || el.naturalWidth === 0) return;
        window.clearInterval(timer);
        // One frame later: give the element a chance to paint the bytes it already decoded.
        window.requestAnimationFrame(() => {
          if (!cancelled) markReady(page);
        });
      }, 120);
      timers.push(timer);
      if (typeof el.decode === "function") {
        void el
          .decode()
          .then(() => {
            if (!cancelled) markReady(page);
          })
          .catch(() => {
            /* rejected for a broken/blocked image; `onError` marks it ready instead */
          });
      }
    }
    return () => {
      cancelled = true;
      for (const timer of timers) window.clearInterval(timer);
    };
  }, [spread.left, spread.right, readerSettings.infiniteScroll]);

  // The preview is what the reader sees while the page itself has no pixels, so it is requested as
  // soon as a page becomes visible — not after a settle delay, which on a throttled connection left
  // a blank frame for as long as it took to notice the user had stopped. It doubles as the source of
  // the page's box (`x-lrr-preview-*`). Requests for pages flipped past are aborted below.
  useEffect(() => {
    if (readerSettings.infiniteScroll) return;
    const pageNumbers = spread.right === null ? [spread.left] : [spread.left, spread.right];
    let cancelled = false;
    for (const page of pageNumbers) {
      if (page < 1) continue;
      void ensurePageMeta(page).then((meta) => {
        // `meta === null` (the HEAD failed) keeps the previous behaviour: a placeholder is cheap
        // insurance when we cannot tell.
        if (!cancelled && meta?.servedOriginal !== true) fetchTinyPreview(page);
      });
    }
    for (const [page, controller] of [...pageTinyFetchesRef.current]) {
      if (!pageNumbers.includes(page)) {
        controller.abort();
        pageTinyFetchesRef.current.delete(page);
      }
    }
    return () => {
      cancelled = true;
    };
  }, [spread.left, spread.right, readerSettings.infiniteScroll, fetchTinyPreview, ensurePageMeta]);

  // Start the file-size HEAD requests as soon as the page URLs are known, instead of only after
  // each `<img>` finishes loading. The file-info bar can then render `name :: size KB` while the
  // image itself is still being downloaded; dimensions fill in later when `onImageLoad` fires.
  useEffect(() => {
    if (!pages.data || !pages.data.pages.length) return
    const pageNumbers =
      spread.right === null ? [spread.left] : [spread.left, spread.right]
    for (const page of pageNumbers) {
      if (page < 1 || page > pages.data.pages.length) continue
      if (prefetchedPageSizesRef.current.has(page)) continue
      const url = pages.data.pages[page - 1]?.url
      if (!url) continue
      prefetchedPageSizesRef.current.add(page)
      const absoluteUrl = new URL(url, window.location.origin).toString()
      void fetchContentLengthKb(absoluteUrl).then((kb) => {
        if (kb !== null) {
          setPageSizesKb((prev) =>
            prev[page] !== undefined ? prev : { ...prev, [page]: kb },
          )
        }
      })
      void fetchResizedPageInfo(absoluteUrl).then((info) => {
        if (info !== null) {
          setResizedPageInfo((prev) =>
            prev[page] !== undefined ? prev : { ...prev, [page]: info },
          )
        }
      })
    }
  }, [pages.data, spread.left, spread.right]);

  useEffect(() => {
    if (!archiveId || totalPages === 0) return;
    if (usesLocalReaderProgress(loggedIn, settings.data)) {
      localStorage.setItem(`${archiveId}-reader`, String(currentPage));
    } else if (isTank) {
      updateTankoubonProgress.mutate(currentPage);
    } else {
      updateProgress.mutate(currentPage);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archiveId, currentPage, totalPages]);

  useEffect(() => {
    if (!archiveId || isTank || totalPages === 0 || !newBadgeMode) return;
    if (newBadgeMode === "until_finished" && currentPage >= totalPages) {
      clearArchiveNewRef.current(archiveId);
    }
  }, [archiveId, isTank, totalPages, currentPage, newBadgeMode]);

  useEffect(() => {
    if (readerSettings.infiniteScroll) return;
    if (!pages.data || readerSettings.preloadCount <= 0) return;
    // Only once the reader has settled: while pages are being flipped through, warming every
    // intermediate neighbour just competes with the page that is actually on screen.
    const timer = window.setTimeout(() => {
      const wanted = new Set<string>();
      for (let offset = 1; offset <= readerSettings.preloadCount; offset++) {
        for (const page of [currentPage + offset, currentPage - offset]) {
          if (page < 1 || page > totalPages) continue;
          const url = pageStreamUrl(page);
          if (!url) continue;
          wanted.add(url);
          warmPage(page);
        }
      }
      for (const [url, image] of [...warmImagesRef.current]) {
        if (!wanted.has(url) && !image.complete) {
          image.src = "";
          warmImagesRef.current.delete(url);
        }
      }
    }, 300);
    return () => window.clearTimeout(timer);
  }, [pages.data, currentPage, totalPages, readerSettings.preloadCount, readerSettings.infiniteScroll, pageStreamUrl, warmPage]);

  // Revoke preview Blob URLs when the reader unmounts.
  useEffect(() => {
    const tinyUrls = [...pageTinyBlobsRef.current.values()];
    for (const url of tinyUrls) URL.revokeObjectURL(url);
    pageTinyBlobsRef.current.clear();
  }, []);

  useEffect(() => {
    if (!archiveId) return;
    let cancelled = false;
    void setupArchiveNavigation(archiveId).then((nav) => {
      if (!cancelled) setNavState(nav);
    });
    return () => {
      cancelled = true;
    };
  }, [archiveId]);

  function goTo(target: Parameters<typeof computeNextPage>[0]) {
    const isSpread = spread.right !== null;
    const next = computeNextPage(
      target,
      currentPage,
      totalPages || 1,
      readerSettings.mangaMode,
      readerSettings.doublePageMode,
      isSpread,
    );
    // At a boundary, step into the adjacent archive instead of clamping (legacy's behavior).
    if (next === currentPage) {
      const goingForward = readerSettings.mangaMode
        ? target === "prev"
        : target === "next";
      if (
        (target === "next" || target === "prev") &&
        (currentPage === 1 || currentPage === totalPages)
      ) {
        startArchiveTransition(goingForward ? "next" : "prev");
        return;
      }
    }
    if (imageAreaRef.current) {
      lastSpreadHeightRef.current =
        imageAreaRef.current.getBoundingClientRect().height;
    }
    setPageOverride(next);
    if (!readerSettings.infiniteScroll) {
      if (
        target === "prev" &&
        readerSettings.kBehavior === K_BEHAVIOR.BACK_BOTTOM
      ) {
        let attempts = 0;
        let lastHeight = -1;
        const scrollWhenSettled = () => {
          attempts += 1;
          const img = leftImgRef.current;
          const height = document.documentElement.scrollHeight;
          const loaded = img && img.complete;
          const settled = loaded && height === lastHeight;
          lastHeight = height;
          if (settled || attempts >= 40) {
            window.scrollTo({ top: height, behavior: "smooth" });
          } else {
            requestAnimationFrame(scrollWhenSettled);
          }
        };
        requestAnimationFrame(scrollWhenSettled);
      } else if (
        target !== "prev" ||
        readerSettings.kBehavior !== K_BEHAVIOR.BACK
      ) {
        window.scrollTo({ top: 0, behavior: "smooth" });
      }
    }
  }

  function goToInfiniteScrollPage(fromPage: number, target: "prev" | "next") {
    let offset = target === "next" ? 1 : -1;
    if (readerSettings.mangaMode) offset = -offset;
    const nextPage = fromPage + offset;
    if (nextPage < 1 || nextPage > totalPages) {
      startArchiveTransition(offset > 0 ? "next" : "prev");
      return;
    }
    // kBehavior only configures `prev`; next always lands at the top. BACK omits block ("nearest").
    const block: ScrollIntoViewOptions["block"] =
      target === "prev"
        ? readerSettings.kBehavior === K_BEHAVIOR.BACK_BOTTOM
          ? "end"
          : readerSettings.kBehavior === K_BEHAVIOR.BACK_TOP
            ? "start"
            : undefined
        : "start";
    document
      .querySelector(`[data-page="${nextPage}"]`)
      ?.scrollIntoView({ behavior: "smooth", block });
  }

  useEffect(() => {
    if (!archiveTransition) return;
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prevOverflow;
    };
  }, [archiveTransition]);

  const queryClient = useQueryClient();
  useEffect(() => {
    if (!archiveId) return;
    void queryClient.prefetchQuery({
      queryKey: RECS_QUERY_KEY(archiveId),
      staleTime: 60_000,
      queryFn: () =>
        fetch(
          `/api/reader/recommendations/${encodeURIComponent(archiveId)}?limit=10`,
        ).then((r) => (r.ok ? r.json() : null)),
    });
  }, [archiveId, queryClient, isTank]);

  function startArchiveTransition(direction: "prev" | "next") {
    // Cached (prefetched) shortlist opens instantly; otherwise show the skeleton while the
    // fetch runs.
    const cached = archiveId
      ? queryClient.getQueryData<{
          recommendations?: {
            archive_id: string;
            title: string;
            score: number;
            isnew: boolean;
            is_read: boolean;
            is_tank: boolean;
            extension?: string;
          }[];
        }>(RECS_QUERY_KEY(archiveId))
      : undefined;
    setArchiveTransition({
      direction,
      recommendations: cached?.recommendations ?? null,
    });
    if (!archiveId || cached) return;
    void queryClient
      .fetchQuery({
        queryKey: RECS_QUERY_KEY(archiveId),
        staleTime: 60_000,
        queryFn: () =>
          fetch(
            `/api/reader/recommendations/${encodeURIComponent(archiveId)}?limit=10`,
          ).then((r) => (r.ok ? r.json() : null)),
      })
      .then((data) => {
        setArchiveTransition((prev) =>
          prev
            ? {
                ...prev,
                recommendations: data?.recommendations ?? [],
              }
            : prev,
        );
      })
      .catch(() => {
        setArchiveTransition((prev) =>
          prev ? { ...prev, recommendations: [] } : prev,
        );
      });
  }

  async function readAdjacentArchive(direction: "prev" | "next") {
    if (document.fullscreenElement) {
      console.warn("Archive navigation not supported in fullscreen mode.");
      return;
    }
    const adjacentId = resolveAdjacentArchive(navState, direction);
    if (!adjacentId) {
      toast({
        text:
          direction === "prev"
            ? (t("reader.thisIsTheFirstArchive") ?? undefined)
            : (t("reader.thisIsTheLastArchive") ?? undefined),
      });
      return;
    }
    if (autoNextActive) sessionStorage.setItem("autoNextPage", "true");
    window.location.assign(`/reader/${adjacentId}`);
  }

  function selectPage(page: number) {
    setPageOverride(clamp(page, 1, totalPages || 1));
    setOverlay(null);
    if (readerSettings.infiniteScroll) {
      document
        .querySelector(`[data-page="${page}"]`)
        ?.scrollIntoView({ block: "start" });
    } else {
      window.scrollTo({ top: 0, behavior: "smooth" });
    }
  }

  function onImageError(e: React.SyntheticEvent<HTMLImageElement>) {
    const img = e.currentTarget;
    // Do not leave the preview covering an image that failed: show the (browser-default) broken
    // state instead of pretending the page is still loading.
    const page = Number(img.dataset.page ?? "0");
    if (page > 0) setMainReady((prev) => (prev[page] ? prev : { ...prev, [page]: true }));
    if (img.src.includes("optimize=1")) return;
    const parsed = new URL(img.src);
    if (!parsed.searchParams.has("path")) return;
    parsed.searchParams.set("optimize", "1");
    img.src = parsed.toString();
  }

  function onImageLoad(
    page: number,
    e: React.SyntheticEvent<HTMLImageElement>,
  ) {
    const img = e.currentTarget;
    const isWide = img.naturalWidth > img.naturalHeight;
    const isPreview = pageTinyBlobsRef.current.get(page) === img.src;
    // Preview/tiny images share the page's aspect ratio, so they may drive the spread layout, but
    // they must never be reported as the final file info (user requirement: no preview dimensions
    // or size in the UI). The full image's own onLoad later fills in dimensions/size.
    setWidespreads((prev) =>
      prev[page] === isWide ? prev : { ...prev, [page]: isWide },
    );
    if (isPreview) return;
    setMainReady((prev) => (prev[page] ? prev : { ...prev, [page]: true }));
    if (img.naturalWidth > 0 && img.naturalHeight > 0) {
      lastDimsRef.current = { width: img.naturalWidth, height: img.naturalHeight };
    }
    // The tiny preview has done its job once the real page is painted. Its Blob used to be released
    // when the full Blob arrived, but the visible page now streams instead of being fetched, so
    // release it here.
    const tinyUrl = pageTinyBlobsRef.current.get(page);
    if (tinyUrl) {
      URL.revokeObjectURL(tinyUrl);
      pageTinyBlobsRef.current.delete(page);
      setPageTinyBlobs(Object.fromEntries(pageTinyBlobsRef.current));
    }
    setPageDimensions((prev) => ({
      ...prev,
      [page]: { width: img.naturalWidth, height: img.naturalHeight },
    }));
    // There is a bitmap on screen now, so a later page turn has something to keep visible while
    // the next page streams.
    displayedAnythingRef.current = true;
    const timing = performance
      .getEntriesByName(img.currentSrc || img.src)
      .at(-1) as PerformanceResourceTiming | undefined;
    if (timing && timing.encodedBodySize > 0) {
      const kb = Math.floor(timing.encodedBodySize / 1024);
      setServedSizesKb((prev) => (prev[page] === kb ? prev : { ...prev, [page]: kb }));
    }
    // The file-info HEADs always use the original server URL, so Content-Length/resize metadata
    // keeps working regardless of the variant the reader is displaying.
    const originalUrl = pages.data?.pages[page - 1]?.url ?? img.src;
    if (pageSizesKb[page] === undefined) {
      void fetchContentLengthKb(originalUrl).then((kb) => {
        if (kb !== null) setPageSizesKb((prev) => ({ ...prev, [page]: kb }));
      });
    }
    if (resizedPageInfo[page] === undefined) {
      void fetchResizedPageInfo(originalUrl).then((info) => {
        setResizedPageInfo((prev) => ({ ...prev, [page]: info }));
      });
    }
  }

  function toggleFullScreen() {
    if (!document.fullscreenElement) {
      containerRef.current?.requestFullscreen?.().catch(() => undefined);
    } else {
      document.exitFullscreen?.().catch(() => undefined);
    }
  }

  useEffect(() => {
    function onFullscreenChange() {
      setIsFullscreen(document.fullscreenElement !== null);
    }
    document.addEventListener("fullscreenchange", onFullscreenChange);
    return () =>
      document.removeEventListener("fullscreenchange", onFullscreenChange);
  }, []);

  async function goRandom() {
    const id = await fetchRandomArchiveId();
    if (id) navigate(routes.reader(id));
  }

  async function handleDeleteEmptyTankoubon() {
    if (
      !archiveId ||
      !(await confirmDialog(t("reader.confirmDeleteTankoubon") ?? ""))
    ) {
      return;
    }
    await deleteTankoubon.mutateAsync(archiveId);
    navigate(routes.library());
  }

  function cleanCache() {
    // Tank mode loops every member archive's thumbnail-regen endpoint.
    if (isTank) {
      generateThumbnailsForArchives.mutate(
        tankReading.chapters.map((c) => c.arcId),
      );
    } else {
      generateThumbnails.mutate();
    }
    window.location.reload();
  }

  function retranslatePage() {
    if (!translationArchiveId) return;
    // Drop this page's detection/text/render caches server-side, then reload so the reader
    // re-requests the page and the pipeline re-runs OCR -> translate -> composite.
    void fetch(
      `/api/archives/${encodeURIComponent(translationArchiveId)}/page/${currentPage}/translation`,
      { method: "DELETE" },
    ).finally(() => window.location.reload());
  }

  async function acquireWakeLock() {
    if (wakeLockRef.current) return;
    const nav = navigator as Navigator & {
      wakeLock?: { request(type: "screen"): Promise<WakeLockSentinelLike> };
    };
    if (!nav.wakeLock) return;
    try {
      const sentinel = await nav.wakeLock.request("screen");
      sentinel.addEventListener("release", () => {
        wakeLockRef.current = null;
      });
      wakeLockRef.current = sentinel;
    } catch {
      // Wake lock is a nice-to-have; a denial (e.g. backgrounded tab) shouldn't break the slideshow.
    }
  }

  function releaseWakeLock() {
    wakeLockRef.current?.release().catch(() => undefined);
    wakeLockRef.current = null;
  }

  function stopAutoNextPage() {
    setAutoNextActive(false);
    releaseWakeLock();
  }

  function startAutoNextPage() {
    if (readerSettings.autoNextPageInterval <= 0) {
      toast({
        heading: t("reader.startingAutoNextPageFailed") ?? undefined,
        text: t("reader.pleaseSetTheAutoNext") ?? undefined,
        icon: "error",
        hideAfter: TOAST_DURATION_MS,
      });
      return;
    }
    setAutoNextCountdown(Math.trunc(readerSettings.autoNextPageInterval));
    setAutoNextActive(true);
    void acquireWakeLock();
  }

  function toggleAutoNextPage() {
    if (autoNextActive) stopAutoNextPage();
    else startAutoNextPage();
  }

  useEffect(() => {
    if (!autoNextActive) return;
    const id = window.setInterval(() => {
      setAutoNextCountdown((prev) => {
        if (prev > 1) return prev - 1;
        const atLastPage = readerSettings.mangaMode
          ? currentPage === 1
          : currentPage === totalPages;
        if (atLastPage) {
          if (navState.ids.length > 0) {
            void readAdjacentArchive(
              readerSettings.mangaMode ? "prev" : "next",
            );
          }
          setAutoNextActive(false);
          releaseWakeLock();
        } else {
          goTo(readerSettings.mangaMode ? "prev" : "next");
        }
        return Math.trunc(readerSettings.autoNextPageInterval);
      });
    }, 1000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    autoNextActive,
    currentPage,
    totalPages,
    readerSettings.mangaMode,
    readerSettings.autoNextPageInterval,
  ]);

  useEffect(() => {
    if (!autoNextActive || totalPages === 0) return;
    sessionStorage.removeItem("autoNextPage");
    void acquireWakeLock();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archiveId, totalPages]);

  useEffect(() => releaseWakeLock, []);

  const currentBookmark = (bookmarks.data ?? []).find(
    (b) => b.page === currentPage,
  );
  const isPageBookmarked = currentBookmark !== undefined;

  async function toggleBookmark() {
    if (!loggedIn) {
      const template = t("reader.aHrefUrlLogin") ?? "";
      toast({
        text: template.replace("${url}", "/login"),
        html: true,
        icon: "warning",
        hideAfter: TOAST_DURATION_MS,
      });
      return;
    }
    if (!archiveId) return;
    if (isPageBookmarked) {
      await removeBookmark.mutateAsync({ archiveId, page: currentPage });
    } else {
      await addBookmark.mutateAsync({ archiveId, page: currentPage });
    }
  }

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if ((e.target as HTMLElement)?.tagName === "INPUT") return;

      // While the boundary overlay is open, only Escape (cancel) acts — navigation shortcuts
      // firing on the locked page behind it were a live bug.
      if (archiveTransition) {
        if (e.key === "Escape") {
          setArchiveTransition(null);
        }
        return;
      }

      if (e.key === ",") {
        void readAdjacentArchive("prev");
        return;
      }
      if (e.key === ".") {
        void readAdjacentArchive("next");
        return;
      }

      switch (e.key) {
        case "Backspace":
          navigate(routes.library());
          return;
        case "Escape":
          if (markerPlacementMode) {
            setMarkerPlacementMode(false);
            return;
          }
          setOverlay(null);
          return;
        case " ":
          window.scrollBy({
            top: window.innerHeight * 0.8,
            behavior: "smooth",
          });
          return;
        case "j":
          if (readerSettings.infiniteScroll) {
            goToInfiniteScrollPage(currentPage, "next");
          } else if (
            window.innerHeight + window.scrollY >=
            document.documentElement.scrollHeight - 1
          ) {
            goTo("next");
          } else {
            const step =
              readerSettings.jScrollUnit === J_SCROLL_UNIT.PX
                ? readerSettings.jScrollAmount
                : window.innerHeight * (readerSettings.jScrollAmount / 100);
            window.scrollBy({ top: step, behavior: "smooth" });
          }
          return;
        case "k":
          if (readerSettings.infiniteScroll) {
            goToInfiniteScrollPage(currentPage, "prev");
          } else if (readerSettings.kBehavior === K_BEHAVIOR.BACK) {
            goTo("prev");
          } else if (window.scrollY <= 0) {
            goTo("prev");
          } else {
            const step =
              readerSettings.jScrollUnit === J_SCROLL_UNIT.PX
                ? readerSettings.jScrollAmount
                : window.innerHeight * (readerSettings.jScrollAmount / 100);
            window.scrollBy({ top: -step, behavior: "smooth" });
          }
          return;
        case "ArrowLeft":
        case "a":
          if (readerSettings.infiniteScroll) {
            if (e.shiftKey) selectPage(1);
            else goToInfiniteScrollPage(currentPage, "prev");
          } else {
            goTo(e.shiftKey ? "first" : "prev");
          }
          return;
        case "ArrowRight":
        case "d":
          if (readerSettings.infiniteScroll) {
            if (e.shiftKey) selectPage(totalPages);
            else goToInfiniteScrollPage(currentPage, "next");
          } else {
            goTo(e.shiftKey ? "last" : "next");
          }
          return;
        case "b":
          void toggleBookmark();
          return;
        case "f":
          toggleFullScreen();
          return;
        case "g": {
          void (async () => {
            const value = await promptDialog(t("reader.goToPage") ?? "");
            const page = value ? parseInt(value, 10) : NaN;
            if (!Number.isNaN(page)) selectPage(page);
          })();
          return;
        }
        case "h":
          setOverlay((prev) => (prev === "help" ? null : "help"));
          return;
        case "m":
          updateReaderSettings({ mangaMode: !readerSettings.mangaMode });
          return;
        case "n":
          toggleAutoNextPage();
          return;
        case "o":
          setOverlay((prev) => (prev === "settings" ? null : "settings"));
          return;
        case "p":
          updateReaderSettings({
            doublePageMode: !readerSettings.doublePageMode,
          });
          return;
        case "q":
          setOverlay((prev) => (prev === "archive" ? null : "archive"));
          return;
        case "r":
          void goRandom();
          return;
        case "s":
          if (!readerSettings.infiniteScroll && loggedIn)
            setMarkerPlacementMode(true);
          return;
        default:
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    currentPage,
    totalPages,
    readerSettings.mangaMode,
    readerSettings.doublePageMode,
    readerSettings.infiniteScroll,
    readerSettings.jScrollUnit,
    readerSettings.jScrollAmount,
    readerSettings.kBehavior,
    navState,
    autoNextActive,
    isPageBookmarked,
    archiveId,
    loggedIn,
    markerPlacementMode,
    archiveTransition,
  ]);

  useEffect(() => {
    if (!readerSettings.infiniteScroll || totalPages === 0) {
      infiniteScrollResumedRef.current = false;
      infiniteScrollResumePageRef.current = null;
      return;
    }
    const root = infiniteScrollRootRef.current;
    if (!root) return;

    // One explicit resume scrollIntoView, only once every prior page has a real reserved height —
    // the tracker attaches after it fires so its first read doesn't stomp currentPage back to 1.
    const alreadyResumed = infiniteScrollResumedRef.current;
    if (!alreadyResumed) {
      if (!infiniteScrollResumeReady) return;
      infiniteScrollResumedRef.current = true;
      infiniteScrollCurrentPageRef.current = currentPage;
      root
        .querySelector<HTMLElement>(`[data-page="${currentPage}"]`)
        ?.scrollIntoView({ block: "start" });
    }
    const REFLOW_GUARD_MS = 400;
    let reflowGuardUntil = performance.now() + REFLOW_GUARD_MS;

    let rafId: number | null = null;
    function updateCurrentPageFromScroll() {
      rafId = null;
      if (performance.now() < reflowGuardUntil) return;
      if (!root) return;
      const viewportMid = window.innerHeight / 2;
      const images = root.querySelectorAll<HTMLElement>("[data-page]");
      for (const img of images) {
        const rect = img.getBoundingClientRect();
        if (rect.top > viewportMid || rect.bottom < viewportMid) continue;
        const page = Number(img.dataset.page);
        if (
          !Number.isNaN(page) &&
          infiniteScrollCurrentPageRef.current !== page
        ) {
          infiniteScrollCurrentPageRef.current = page;
          setPageOverride(page);
        }
        break;
      }
    }
    function onScroll() {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(updateCurrentPageFromScroll);
    }
    window.addEventListener("scroll", onScroll, { passive: true });

    const resizeObserver = new ResizeObserver(() => {
      reflowGuardUntil = performance.now() + REFLOW_GUARD_MS;
    });
    root
      .querySelectorAll<HTMLElement>("[data-page]")
      .forEach((img) => resizeObserver.observe(img));

    return () => {
      window.removeEventListener("scroll", onScroll);
      if (rafId !== null) cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    readerSettings.infiniteScroll,
    totalPages,
    pages.data,
    infiniteScrollResumeReady,
  ]);

  if (metadata.isLoading || pages.isLoading) {
    return (
      <div className="loading">
        <div className="loading-overlay">
          <p className="loading-spinner">
            <i className="fas fa-fan fa-spin"></i>
          </p>
        </div>
      </div>
    );
  }

  if (metadata.isError || pages.isError || !pages.data || !metadata.data) {
    const firstError = [metadata.error, pages.error].find(
      (e) => e instanceof ApiError,
    ) as ApiError | undefined;
    if (firstError?.status === 404) return <NotFoundPage />;
    if (firstError?.status === 403)
      return <ForbiddenPage reason={firstError.message} />;
    if (firstError?.status === 401) return null;

    return (
      <div className="ido">
        <p>
          {t("common.failedToLoadArchivesError", {
            error: String(metadata.error ?? pages.error),
          })}
        </p>
        <input
          type="button"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    );
  }

  if (isTank && totalPages === 0) {
    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <i className="fas fa-8x fa-box-open" aria-hidden="true"></i>
        <h2 style={{ marginTop: 16 }}>
          {t("reader.thisTankoubonHasNoArchives")}
        </h2>
        <div
          style={{
            marginTop: 16,
            display: "flex",
            gap: 8,
            justifyContent: "center",
          }}
        >
          <input
            type="button"
            className="stdbtn"
            value={t("common.editTankoubon") ?? undefined}
            onClick={() =>
              archiveId && navigate(routes.tankoubonEdit(archiveId))
            }
          />
          <input
            type="button"
            className="stdbtn"
            value={t("common.deleteTankoubon") ?? undefined}
            onClick={() => void handleDeleteEmptyTankoubon()}
          />
          <input
            type="button"
            className="stdbtn"
            value={t("common.returnToLibrary") ?? undefined}
            onClick={() => navigate(routes.library())}
          />
        </div>
      </div>
    );
  }

  const leftUrl = pages.data.pages[spread.left - 1]?.url;
  const rightUrl =
    spread.right !== null ? pages.data.pages[spread.right - 1]?.url : null;
  // “查看全尺寸图像” should open the raw entry (no optimize=1), not the reader's resized WebP.
  const originalLeftUrl = leftUrl
    ? (() => {
        const u = new URL(leftUrl, window.location.origin);
        u.searchParams.delete("optimize");
        return u.toString();
      })()
    : undefined;
  const originalRightUrl = rightUrl
    ? (() => {
        const u = new URL(rightUrl, window.location.origin);
        u.searchParams.delete("optimize");
        return u.toString();
      })()
    : null;

  // Pages of a PDF are served as images; its original opens in the browser's own PDF viewer, for
  // selectable, searchable text. Raw files are not available to guests.
  const renderOpenPdfButton = (style: React.CSSProperties) =>
    !isTank && loggedIn && metadata.data?.extension === "pdf" ? (
      <IconButton
        variant="ghost-btn"
        icon="fas fa-file-pdf fa-2x"
        title={t("reader.openOriginalPdf") ?? undefined}
        size={32}
        style={style}
        onClick={() => {
          window.open(`/api/archives/${archiveId}/download?inline=true`, "_blank", "noopener");
        }}
      />
    ) : null;

  // In Tankoubon reading, `archiveId` from the route is the tank id (`TANK_xxx`), not a real
  // archive — every displayed page actually belongs to one of the tank's member archives, so
  // marker/translation targets must resolve through `getArchiveForPage` (global page → real
  // member archive + that archive's own local page number), not use the route param directly.
  // Confirmed live: before this, translation silently never worked while reading a Tankoubon,
  // because the server was asked to translate a page of an archive whose id was actually a tank id.
  const markerTarget = isTank
    ? tankReading.getArchiveForPage(spread.left)
    : archiveId
      ? { arcId: archiveId, localPage: spread.left }
      : null;
  const markerTargetRight =
    spread.right === null
      ? null
      : isTank
        ? tankReading.getArchiveForPage(spread.right)
        : archiveId
          ? { arcId: archiveId, localPage: spread.right }
          : null;

  // Local-backend compositing happens in the browser, and OCR coordinates are relative to the
  // original archive page — not the reader's optimize=1 WebP. `loadLeftPageImage`/
  // `loadRightPageImage` (defined near the top of the component, before any early `return`) read
  // these off `originalPageUrlsRef` rather than closing over them directly — this assignment can
  // safely live here, after the early-return guards that make `pages.data` safe to index, without
  // it affecting how many hooks this component calls on any given render.
  originalPageUrlsRef.current = { left: originalLeftUrl, right: originalRightUrl };

  const isSpreadShowing = spread.right !== null;
  const imageStyle: React.CSSProperties = {};
  const outerStyle: React.CSSProperties = {};
  // The tiny placeholder is an absolutely positioned sibling inside the same wrapper (see
  // `tinyUnderlayStyle`). An absolutely positioned box paints *above* a static sibling regardless
  // of DOM order, so without this the reader showed the low-res preview stretched over the
  // full-resolution page. Making the real page a positioned element with a higher stack level puts
  // it back on top; `placementImageStyle` may raise its own z-index further for marker placement.
  imageStyle.position = "relative";
  imageStyle.zIndex = 1;
  if (!isFullscreen) {
    if (readerSettings.fitMode === FIT_MODE.FIT_HEIGHT) {
      const heightVh =
        readerSettings.hideHeader || readerSettings.infiniteScroll ? 98 : 90;
      imageStyle.maxHeight =
        fitHeightPx !== null ? `${fitHeightPx}px` : `${heightVh}vh`;
      // On desktop the container hugs the page (legacy's shape), but not tighter than the toolbar
      // needs: its two side icon groups are absolutely positioned (198px and 214px) and the paginator
      // between them is ~224px, so ~676px is where they stop overlapping. Measuring that instead
      // feeds back into itself (the side groups widen with the container), hence a constant. Phones
      // keep the plain full-width container — their toolbar is a separate floating bar, and
      // `fit-content` there overflowed by a few pixels.
      if (!isMobileToolbarViewport) {
        outerStyle.width = "fit-content";
        outerStyle.minWidth = "min(680px, 100%)";
      }
    } else if (readerSettings.fitMode === FIT_MODE.FIT_WIDTH) {
      imageStyle.width = "100%";
      outerStyle.maxWidth = "98%";
    } else if (readerSettings.containerWidth) {
      outerStyle.maxWidth = readerSettings.containerWidth;
      imageStyle.width = "100%";
    } else if (isSpreadShowing) {
      outerStyle.maxWidth = "90%";
    } else {
      outerStyle.maxWidth = "1200px";
    }
  }

  /** The tiny preview, pinned under the streaming page in the same box: it costs ~10 KB and paints
   * long before a full page does (the first frame of a 20 MB animation needs ~76 KB, a static scan
   * usually more), so it is what the reader sees while the real image streams in. Absolutely
   * positioned, so it never contributes to layout — the box comes from the page's own
   * `width`/`height` attributes above. */
  const tinyUnderlayStyle: React.CSSProperties = {
    position: "absolute",
    zIndex: 0,
    top: 0,
    left: 0,
    width: "100%",
    height: "100%",
    objectFit: "contain",
    pointerEvents: "none",
  };

  const placementImageStyle: React.CSSProperties = markerPlacementMode
    ? { ...imageStyle, zIndex: 22, cursor: "cell", touchAction: "none" }
    : imageStyle;

  const helpContent = (
    <div style={{ fontSize: FONT_SIZE_XS }}>
      <p style={{ margin: "0 0 4px" }}>
        {t("reader.youCanNavigateBetweenPages")}
      </p>
      <ul style={{ margin: "0 0 8px", paddingLeft: 18 }}>
        <li>{t("reader.theArrowIcons")}</li>
        <li>
          {t("reader.the")} <Key>A</Key>/<Key>D</Key> {t("reader.keys")}
        </li>
        <li>{t("reader.yourKeyboardArrowsAndThe")}</li>
        <li>{t("reader.touchingTheLeftRightSide")}</li>
        <li>
          <Key>J</Key> {t("reader.jScrollsDownThenAdvancesAtTheBottom")}
        </li>
        <li>
          <Key>K</Key> {t("reader.kGoesToThePreviousPage")}
        </li>
      </ul>
      <p style={{ margin: "0 0 4px" }}>
        {t("reader.whenReadingAnArchiveFrom")}
      </p>
      <ul style={{ margin: "0 0 8px", paddingLeft: 18 }}>
        <li>
          <Key>,</Key> {t("reader.and")} <Key>.</Key> {t("reader.keys")}
        </li>
        <li>{t("reader.readingPastTheFirstLast")}</li>
      </ul>
      <p style={{ margin: "0 0 4px" }}>{t("reader.otherKeyboardShortcuts")}</p>
      <ul style={{ margin: "0 0 8px", paddingLeft: 18 }}>
        <li>
          <Key>M</Key> {t("reader.mToggleMangaModeRighttoleft")}
        </li>
        <li>
          <Key>O</Key> {t("reader.oShowAdvancedReaderOptions")}
        </li>
        <li>
          <Key>P</Key> {t("reader.pToggleDoublePageMode")}
        </li>
        <li>
          <Key>Q</Key> {t("reader.qBringUpTheThumbnail")}
        </li>
        <li>
          <Key>R</Key> {t("reader.rOpenARandomArchive")}
        </li>
        <li>
          <Key>F</Key> {t("reader.fToggleFullscreenMode")}
        </li>
        {loggedIn && (
          <li>
            <Key>B</Key> {t("reader.bToggleBookmarkThisPage")}
          </li>
        )}
        <li>
          <Key>N</Key> {t("reader.nToggleAutoNextPage")}
        </li>
        <li>
          <Key>Shift</Key>+<Key>←</Key>/<Key>→</Key>{" "}
          {t("reader.shiftleftRightGoToFirst")}
        </li>
        <li>
          <Key>G</Key> {t("reader.gGoToPageNumber")}
        </li>
        {loggedIn && (
          <li>
            <Key>S</Key> {t("reader.sSetAStamp")}
          </li>
        )}
      </ul>
      <p style={{ margin: 0 }}>{t("reader.toReturnToTheArchive")}</p>
    </div>
  );

  const pagesel = (
    <>
      <div className="absolute-options absolute-left">
        <IconButton
          variant="ghost-btn"
          icon="fas fa-cog fa-2x"
          title={t("reader.readerOptions") ?? undefined}
          size={32}
          style={{ marginRight: 13, borderRadius: "50%" }}
          onClick={() => {
            setOverlay((prev) => (prev === "settings" ? null : "settings"));
          }}
        />
        {translationSettings && translationScope && (
          <ToggleButtonMenu
            icon="fas fa-language fa-2x"
            title={t("reader.toggleTranslation") ?? undefined}
            active={translationScope.enabled}
            onClick={toggleTranslationEnabled}
            menuTrigger={t("reader.chooseTranslationLanguage") ?? "Choose translation language"}
            menuTitle={t("reader.chooseTranslationLanguage") ?? undefined}
            menuSide="bottom"
            menuAlign="start"
            size={32}
            style={{ marginRight: 13 }}
            menuChildren={
              <>
                <MenuItem
                  onClick={() => setTranslationTargetLanguage(null)}
                >
                  {translationSettings.targetLanguage === null && <i className="fas fa-check" style={{ marginRight: 8 }} />}
                  {t("reader.translationLanguageBrowserDefault") ?? "Browser default"}
                </MenuItem>
                <MenuSeparator />
                {TRANSLATION_TARGET_LANGUAGES.map(({ code, nativeName }) => (
                  <MenuItem key={code} onClick={() => setTranslationTargetLanguage(code)}>
                    {translationSettings.targetLanguage === code && <i className="fas fa-check" style={{ marginRight: 8 }} />}
                    {nativeName}
                  </MenuItem>
                ))}
              </>
            }
          />
        )}
        {!supportsHover && !readerSettings.infiniteScroll && loggedIn && (
          <IconButton
            variant="ghost-btn"
            icon="fas fa-stamp fa-2x"
            title={t("reader.placeStamp") ?? undefined}
            size={32}
            style={{ marginRight: 13, borderRadius: "50%", opacity: markerPlacementMode ? 1 : 0.55 }}
            onClick={() => setMarkerPlacementMode((prev) => !prev)}
          />
        )}
        {loggedIn && archiveId && (
          <BookmarkButton
            archiveId={archiveId}
            page={currentPage}
            bookmarked={isPageBookmarked}
            name={currentBookmark?.name}
            loggedIn={loggedIn}
            onRequireLogin={() => {
              const template = t("reader.aHrefUrlLogin") ?? "";
              toast({
                text: template.replace("${url}", "/login"),
                html: true,
                icon: "warning",
                hideAfter: TOAST_DURATION_MS,
              });
            }}
          />
        )}
        <Tooltip label={helpContent} maxWidth={420}>
          <IconButton
            variant="ghost-btn"
            icon="fas fa-question-circle fa-2x"
            title={t("reader.help") ?? undefined}
            size={32}
            style={{ marginRight: 13, borderRadius: "50%" }}
          />
        </Tooltip>
      </div>
      <div className="absolute-options absolute-right">
        <IconButton
          variant="ghost-btn"
          className="reading-direction"
          icon={`fas ${readerSettings.mangaMode ? "fa-arrow-left" : "fa-arrow-right"} fa-2x`}
          title={t("reader.readingDirection") ?? undefined}
          size={32}
          style={{ marginRight: 13, borderRadius: "50%" }}
          onClick={() => {
            updateReaderSettings({ mangaMode: !readerSettings.mangaMode });
          }}
        />
        <IconButton
          variant="ghost-btn"
          className="toggle-auto-next-page"
          icon={
            <>
              <i className="fas fa-stopwatch fa-2x" aria-hidden="true" />
              {autoNextActive ? autoNextCountdown : ""}
            </>
          }
          title={t("reader.autoNextPage") ?? undefined}
          size={32}
          style={{ marginRight: 13, borderRadius: "50%" }}
          onClick={() => {
            toggleAutoNextPage();
          }}
        />
        {renderOpenPdfButton({ marginRight: 13, borderRadius: "50%" })}
        <IconButton
          variant="ghost-btn"
          icon="fas fa-th fa-2x"
          title={t("reader.archiveOverview") ?? undefined}
          size={32}
          style={{ marginRight: 13, borderRadius: "50%" }}
          onClick={() => {
            openedByDefaultSetting.current = false;
            setOverlay((prev) => (prev === "archive" ? null : "archive"));
          }}
        />
        <FitMenuButton
          isFullscreen={isFullscreen}
          onToggleFullscreen={toggleFullScreen}
          onSelectFitMode={(mode) => updateReaderSettings({ fitMode: mode })}
          primary={fitAction}
          onPrimaryChange={(action) => {
            setFitAction(action);
            storeFitAction(action);
          }}
          compact={isMobileToolbarViewport}
        />
      </div>
    </>
  );

  const arrows = readerSettings.infiniteScroll ? null : (
    <div
      className="sn paginator"
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 12,
        marginTop: 10,
        marginBottom: 10,
      }}
    >
      <a
        className="fas fa-backward-step page-link archive-nav-link"
        title={t("reader.previousArchive") ?? undefined}
        style={{
          fontSize: PAGINATOR_ICON_FONT_SIZE,
          display: navState.ids.length > 0 ? undefined : "none",
        }}
        onClick={() => void readAdjacentArchive("prev")}
      />
      <a
        className="fas fa-angle-double-left page-link"
        title={t("reader.firstPage") ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo("first")}
      />
      <a
        className="fas fa-angle-left page-link"
        title={t("reader.previousPage") ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo("prev")}
      />
      <div
        className="pagecount"
        style={{
          fontSize: PAGINATOR_PAGECOUNT_FONT_SIZE,
          lineHeight: 1,
          margin: 0,
          padding: 0,
        }}
      >
        <span className="current-page">{currentPage}</span> /{" "}
        <span className="max-page">{totalPages}</span>
      </div>
      <a
        className="fas fa-angle-right page-link"
        title={t("reader.nextPage") ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo("next")}
      />
      <a
        className="fas fa-angle-double-right page-link"
        title={t("reader.lastPage") ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo("last")}
      />
      <a
        className="fas fa-forward-step page-link archive-nav-link"
        title={t("reader.nextArchive") ?? undefined}
        style={{
          fontSize: PAGINATOR_ICON_FONT_SIZE,
          display: navState.ids.length > 0 ? undefined : "none",
        }}
        onClick={() => void readAdjacentArchive("next")}
      />
    </div>
  );

  // Single fixed bottom bar for narrow viewports, replacing `pagesel` + `arrows` (which mobile CSS
  // used to render as two separate floating icon pills plus an in-document-flow paginator strip —
  // real user report, 2026-09-07: the paginator isn't `position: fixed` like the icon pills are, so
  // it could scroll out of view/end up hidden depending on page height, reading as "the toolbar
  // sometimes disappears"). Everything here lives inside one `position: fixed` container instead, so
  // nothing in it can be scrolled away. Infrequently-used controls (reading direction, auto-advance,
  // reader options) collapse into a `moreMenu` overflow menu per the user's own triage of what
  // deserves to stay one tap away vs. what doesn't.
  // In tankoubon reading the route id is the tank; "this archive" is the member being shown.
  const downloadArchiveId = isTank ? markerTarget?.arcId : archiveId;
  const moreMenuTrigger = (
    <IconButton
      variant="ghost-btn"
      icon="fas fa-ellipsis-h fa-2x"
      title={t("reader.moreOptions") ?? undefined}
      size={32}
      style={{ borderRadius: "50%" }}
    />
  );
  const moreMenu = (
    <Menu trigger={moreMenuTrigger} side="top" align="end">
      <MenuItem onClick={() => updateReaderSettings({ mangaMode: !readerSettings.mangaMode })}>
        <i className={`fas ${readerSettings.mangaMode ? "fa-arrow-left" : "fa-arrow-right"}`} style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
        {t("reader.readingDirection")}
      </MenuItem>
      <MenuItem onClick={() => toggleAutoNextPage()} closeOnClick={false}>
        <i className="fas fa-stopwatch" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
        {t("reader.autoNextPage")}
        {autoNextActive ? ` (${autoNextCountdown})` : ""}
      </MenuItem>
      {loggedIn && downloadArchiveId && (
        <MenuItem onClick={() => window.location.assign(`/api/archives/${downloadArchiveId}/download`)}>
          <i className="fas fa-download" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
          {t("reader.downloadThisArchive")}
        </MenuItem>
      )}
      {loggedIn && downloadArchiveId && (
        <MenuItem onClick={() => setShareItem({ kind: "archive", id: downloadArchiveId })}>
          <i className="fas fa-share-nodes" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
          {isTank ? t("reader.shareThisArchive") : t("share.action.share")}
        </MenuItem>
      )}
      {loggedIn && isTank && archiveId && (
        <MenuItem onClick={() => setShareItem({ kind: "tankoubon", id: archiveId })}>
          <i className="fas fa-share-nodes" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
          {t("reader.shareWholeTankoubon")}
        </MenuItem>
      )}
      {loggedIn && isTank && archiveId && (
        <MenuItem onClick={() => window.location.assign(`/api/tankoubons/${archiveId}/download`)}>
          <i className="fas fa-file-zipper" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
          {t("reader.downloadWholeTankoubon")}
        </MenuItem>
      )}
      <MenuSeparator />
      <MenuItem onClick={() => setOverlay((prev) => (prev === "settings" ? null : "settings"))}>
        <i className="fas fa-cog" style={{ marginRight: 8, width: 16 }} aria-hidden="true" />
        {t("reader.readerOptions")}
      </MenuItem>
    </Menu>
  );
  const mobileToolbar = !readerSettings.infiniteScroll && (
    <div className="mobile-reader-toolbar">
      {arrows}
      <div className="mobile-reader-toolbar-icons">
        {translationSettings && translationScope && (
          <ToggleButtonMenu
            icon="fas fa-language fa-2x"
            title={t("reader.toggleTranslation") ?? undefined}
            active={translationScope.enabled}
            onClick={toggleTranslationEnabled}
            menuTrigger={t("reader.chooseTranslationLanguage") ?? "Choose translation language"}
            menuTitle={t("reader.chooseTranslationLanguage") ?? undefined}
            menuSide="top"
            menuAlign="end"
            size={32}
            menuChildren={
              <>
                <MenuItem onClick={() => setTranslationTargetLanguage(null)}>
                  {translationSettings.targetLanguage === null && <i className="fas fa-check" style={{ marginRight: 8 }} />}
                  {t("reader.translationLanguageBrowserDefault") ?? "Browser default"}
                </MenuItem>
                <MenuSeparator />
                {TRANSLATION_TARGET_LANGUAGES.map(({ code, nativeName }) => (
                  <MenuItem key={code} onClick={() => setTranslationTargetLanguage(code)}>
                    {translationSettings.targetLanguage === code && <i className="fas fa-check" style={{ marginRight: 8 }} />}
                    {nativeName}
                  </MenuItem>
                ))}
              </>
            }
          />
        )}
        {!supportsHover && loggedIn && (
          <IconButton
            variant="ghost-btn"
            icon="fas fa-stamp fa-2x"
            title={t("reader.placeStamp") ?? undefined}
            size={32}
            style={{ borderRadius: "50%", opacity: markerPlacementMode ? 1 : 0.55 }}
            onClick={() => setMarkerPlacementMode((prev) => !prev)}
          />
        )}
        {loggedIn && archiveId && (
          <BookmarkButton
            archiveId={archiveId}
            page={currentPage}
            bookmarked={isPageBookmarked}
            name={currentBookmark?.name}
            loggedIn={loggedIn}
            onRequireLogin={() => {
              const template = t("reader.aHrefUrlLogin") ?? "";
              toast({
                text: template.replace("${url}", "/login"),
                html: true,
                icon: "warning",
                hideAfter: TOAST_DURATION_MS,
              });
            }}
          />
        )}
        <Tooltip label={helpContent} maxWidth={420}>
          <IconButton
            variant="ghost-btn"
            icon="fas fa-question-circle fa-2x"
            title={t("reader.help") ?? undefined}
            size={32}
            style={{ borderRadius: "50%" }}
          />
        </Tooltip>
        {renderOpenPdfButton({ borderRadius: "50%" })}
        <IconButton
          variant="ghost-btn"
          icon="fas fa-th fa-2x"
          title={t("reader.archiveOverview") ?? undefined}
          size={32}
          style={{ borderRadius: "50%" }}
          onClick={() => {
            openedByDefaultSetting.current = false;
            setOverlay((prev) => (prev === "archive" ? null : "archive"));
          }}
        />
        <FitMenuButton
          isFullscreen={isFullscreen}
          onToggleFullscreen={toggleFullScreen}
          onSelectFitMode={(mode) => updateReaderSettings({ fitMode: mode })}
          primary={fitAction}
          onPrimaryChange={(action) => {
            setFitAction(action);
            storeFitAction(action);
          }}
          compact={isMobileToolbarViewport}
        />
        {moreMenu}
      </div>
    </div>
  );

  const currentFileInfo = pages.data
    ? fileInfoText(
        pages.data.pages,
        spread,
        pageDimensions,
        pageSizesKb,
        window.location.origin,
      )
    : "";
  const isFileInfoReady =
    spread.right === null
      ? pageSizesKb[spread.left] !== undefined
      : pageSizesKb[spread.left] !== undefined &&
        pageSizesKb[spread.right] !== undefined;
  if (isFileInfoReady) lastFileInfoRef.current = currentFileInfo;
  const displayedFileInfo = isFileInfoReady
    ? currentFileInfo
    : (lastFileInfoRef.current ?? currentFileInfo);
  const leftResizeInfo =
    spread.right === null ? resizedPageInfo[spread.left] : undefined;
  const pageEntryName = pages.data
    ? (new URL(
        pages.data.pages[spread.left - 1].url,
        window.location.origin,
      ).searchParams.get("path") ?? "")
    : "";
  const servedKb = spread.right === null ? servedSizesKb[spread.left] : undefined;
  const leftDims =
    spread.right === null ? pageDimensions[spread.left] : undefined;
  let resizedFileInfo: React.ReactNode | null = null;
  const resizedDims =
    leftDims ??
    (leftResizeInfo && leftResizeInfo.origWidth > 0 && leftResizeInfo.origHeight > 0
      ? { width: leftResizeInfo.origWidth, height: leftResizeInfo.origHeight }
      : null);
  if (leftResizeInfo && servedKb !== undefined && resizedDims) {
    const origKb = leftResizeInfo.origSizeBytes / 1024;
    const origSizeText =
      origKb >= 1024
        ? `${(origKb / 1024).toFixed(1)} MB`
        : `${Math.round(origKb)} KB`;
    const savedPct = Math.max(
      0,
      Math.round((1 - (servedKb * 1024) / leftResizeInfo.origSizeBytes) * 100),
    );
    resizedFileInfo = (
      <>
        <span className="file-info-opt">
          {pageEntryName} → {leftResizeInfo.servedFormat.toUpperCase()} :: {resizedDims.width} x {resizedDims.height} ::{" "}
          {servedKb} KB
        </span>
        {" · "}
        <span className="file-info-orig">
          {t("reader.original")}: {pageEntryName.split(".").pop()}{" "}
          {leftResizeInfo.origWidth} x {leftResizeInfo.origHeight} ::{" "}
          {origSizeText}
          {" · "}
          {t("reader.savedPct", { pct: savedPct })}
        </span>
      </>
    );
  }
  const fileinfo = (
    <div className="file-info" title={displayedFileInfo}>
      {resizedFileInfo ?? displayedFileInfo}
    </div>
  );

  const artistMatch = metadata.data.tags.match(/artist:([^,]+)(?:,|$)/i);
  const formatBadge = isBookExtension(metadata.data.extension) && (
    <>
      {" "}
      <FormatBadge extension={metadata.data.extension} />
    </>
  );
  const archiveHeading = artistMatch ? (
    <>
      {metadata.data.title} by{" "}
      <a href={getTagSearchURL("artist", artistMatch[1])}>{artistMatch[1]}</a>
      {formatBadge}
    </>
  ) : (
    <>
      {metadata.data.title}
      {formatBadge}
    </>
  );

  return (
    <>
      <div id="i1" className="sni" ref={containerRef} style={outerStyle}>
        {!readerSettings.hideHeader && (
          <div id="i2">
            <h1 id="archive-title">{archiveHeading}</h1>
            {pagesel}
            {arrows}
            {fileinfo}
          </div>
        )}

        <div
          id="i3"
          ref={imageAreaRef}
          className={
            !readerSettings.infiniteScroll && !currentSpreadLoaded
              ? "loading"
              : undefined
          }
          style={
            !currentSpreadLoaded && lastSpreadHeightRef.current !== null
              ? { minHeight: lastSpreadHeightRef.current }
              : undefined
          }
        >
          {readerSettings.infiniteScroll ? (
            <div id="display" className="infinite-scroll" ref={infiniteScrollRootRef}>
              {pages.data.pages.map((url, i) => {
                const hasRealHeight = pageDimensions[i + 1] !== undefined;
                const resumeDim =
                  infiniteScrollResumeDimensions.data?.dimensions[i];
                const style: React.CSSProperties =
                  !hasRealHeight && resumeDim
                    ? {
                        ...imageStyle,
                        aspectRatio: `${resumeDim.width} / ${resumeDim.height}`,
                      }
                    : imageStyle;
                return (
                  <img
                    key={url.url}
                    data-page={i + 1}
                    className={
                      hasRealHeight
                        ? "reader-image"
                        : "reader-image loading-placeholder"
                    }
                    src={url.url}
                    alt={`${t("reader.page")} ${i + 1}`}
                    loading="lazy"
                    draggable={false}
                    style={style}
                    onLoad={(e) => onImageLoad(i + 1, e)}
                    onError={onImageError}
                    onClick={(e) => {
                      const isLeftHalf = e.clientX < window.innerWidth / 2;
                      goToInfiniteScrollPage(
                        i + 1,
                        isLeftHalf ? "prev" : "next",
                      );
                    }}
                  />
                );
              })}
            </div>
          ) : (
            <div id="display">
              <a
                id="imgLink"
                href={leftUrl}
                onClick={(e) => {
                  const x = e.clientX;
                  const isLeftHalf = x < window.innerWidth / 2;
                  e.preventDefault();
                  e.currentTarget.blur();
                  goTo(isLeftHalf ? "prev" : "next");
                }}
                style={{ position: "relative", display: "inline-flex" }}
              >
                <span style={{ position: "relative", display: "inline-flex" }}>
                  {pageTinyBlobs[spread.left] &&
                  pageDimensions[spread.left] === undefined ? (
                    <img
                      className="reader-image"
                      key={`tiny-left-${spread.left}`}
                      src={pageTinyBlobs[spread.left]}
                      alt=""
                      aria-hidden="true"
                      draggable={false}
                      style={{
                        ...tinyUnderlayStyle,
                        // Above the page while it has nothing to draw (so the preview is what you
                        // see), below it once the real bitmap is paintable.
                        zIndex: mainReady[spread.left] ? 0 : 2,
                      }}
                    />
                  ) : null}
                  <img
                    id="img"
                    // Remounting on a page turn is what makes the *old* page stop being displayed:
                    // Chrome keeps painting the previous bitmap (and even reports its
                    // `naturalWidth`) until a changed `src` finishes decoding, which on a 20 MB page
                    // over a throttled connection looked exactly like "paging does nothing". A fresh
                    // element has no bitmap and reports 0, so the preview below shows through and
                    // the readiness poll below means what it says.
                    key={`left-${spread.left}`}
                    ref={leftImgRef}
                    data-page={spread.left}
                    className="reader-image"
                    src={
                      pageStreamUrl(spread.left) ??
                      leftUrl
                    }
                    // Fall back to the last box we saw: a page whose dimensions are not known yet
                    // (and whose stream has not produced a header) would otherwise collapse to
                    // zero size, which reads as "paging stopped working".
                    width={
                      pageTinyDims[spread.left]?.width ??
                      pageDimensions[spread.left]?.width ??
                      lastDimsRef.current?.width
                    }
                    height={
                      pageTinyDims[spread.left]?.height ??
                      pageDimensions[spread.left]?.height ??
                      lastDimsRef.current?.height
                    }
                    alt={`${t("reader.page")} ${spread.left}`}
                    fetchPriority="high"
                    onLoad={(e) => onImageLoad(spread.left, e)}
                    onError={onImageError}
                    draggable={false}
                    style={placementImageStyle}
                  />
                  {/* Phase 2 on-page translation. Absolutely positioned over the page above (the
                      wrapper is `position: relative`), so the original image's own load/error
                      pipeline is completely unaffected. Renders nothing when translation is off.
                      Also kept per page rather than across the whole spread: an overlay spanning
                      both pages would stretch one page's rendering over a double-page spread. */}
                  {markerTarget && (
                    <TranslationOverlay
                      archiveId={markerTarget.arcId}
                      page={markerTarget.localPage}
                      settings={translationSettings}
                      enabled={translationScope?.enabled ?? false}
                      loadPageImage={loadLeftPageImage}
                    />
                  )}
                </span>
                {rightUrl && (
                  <span style={{ position: "relative", display: "inline-flex" }}>
                    {spread.right !== null &&
                    pageTinyBlobs[spread.right] &&
                    pageDimensions[spread.right] === undefined ? (
                      <img
                        className="reader-image"
                        key={`tiny-right-${spread.right}`}
                        src={pageTinyBlobs[spread.right]}
                        alt=""
                        aria-hidden="true"
                        draggable={false}
                        style={{
                          ...tinyUnderlayStyle,
                          zIndex: mainReady[spread.right] ? 0 : 2,
                        }}
                      />
                    ) : null}
                    <img
                      id="img_doublepage"
                      key={`right-${spread.right ?? "none"}`}
                      ref={rightImgRef}
                      data-page={spread.right ?? undefined}
                      className="reader-image"
                      src={
                        rightUrl
                          ? (pageStreamUrl(spread.right ?? 0) ?? rightUrl)
                          : undefined
                      }
                      width={
                        pageTinyDims[spread.right ?? 0]?.width ??
                        pageDimensions[spread.right ?? 0]?.width ??
                        lastDimsRef.current?.width
                      }
                      height={
                        pageTinyDims[spread.right ?? 0]?.height ??
                        pageDimensions[spread.right ?? 0]?.height ??
                        lastDimsRef.current?.height
                      }
                      alt={`${t("reader.page")} ${spread.right}`}
                      fetchPriority="high"
                      onLoad={(e) => onImageLoad(spread.right ?? 0, e)}
                      onError={onImageError}
                      draggable={false}
                      style={imageStyle}
                    />
                    {markerTargetRight && (
                      <TranslationOverlay
                        archiveId={markerTargetRight.arcId}
                        page={markerTargetRight.localPage}
                        settings={translationSettings}
                        enabled={translationScope?.enabled ?? false}
                        loadPageImage={loadRightPageImage}
                      />
                    )}
                  </span>
                )}
              </a>
              {markerTarget && (
                <MarkerLayer
                  archiveId={markerTarget.arcId}
                  page={markerTarget.localPage}
                  imageRef={leftImgRef}
                  visible={readerSettings.markersVisible}
                  placementMode={markerPlacementMode}
                  onPlaced={() => setMarkerPlacementMode(false)}
                  loggedIn={loggedIn}
                />
              )}
            </div>
          )}
        </div>

        <div id="i4">
          {fileinfo}
          {isMobileToolbarViewport ? (
            mobileToolbar
          ) : (
            <>
              {pagesel}
              {arrows}
            </>
          )}
        </div>

        <div id="i5">
          <div className="sb">
            <a
              id="return-to-index"
              style={{ cursor: "pointer" }}
              title={t("reader.doneReadingGoBackTo") ?? undefined}
              onClick={() => navigate(routes.library())}
            >
              <i className="fas fa-angle-down fa-3x"></i>
            </a>
          </div>
        </div>

        <div id="i7" className="if">
          <i className="fas fa-caret-right fa-lg"></i>
          <a href={originalLeftUrl} target="_blank" rel="noreferrer">
            {t("reader.viewFullsizeImage")}
          </a>
          <i className="fas fa-caret-right fa-lg"></i>
          <a style={{ cursor: "pointer" }} onClick={() => void goRandom()}>
            {t("reader.switchToAnotherRandomArchive")}
          </a>
          {loggedIn && (
            <>
              <i className="fas fa-caret-right fa-lg"></i>
              <a style={{ cursor: "pointer" }} onClick={cleanCache}>
                {t("reader.cleanArchiveCache")}
              </a>
              {translationScope?.enabled && (
                <>
                  <i className="fas fa-caret-right fa-lg"></i>
                  <a style={{ cursor: "pointer" }} onClick={retranslatePage}>
                    {t("reader.retranslatePage")}
                  </a>
                </>
              )}
            </>
          )}
        </div>

        {shareItem && (
          <Suspense fallback={null}>
            <ShareDialog items={[shareItem]} onClose={() => setShareItem(null)} />
          </Suspense>
        )}
        {showTankSplit && archiveId && isTank && (
          <TankoubonSplitModal
            tankId={archiveId}
            onClose={() => setShowTankSplit(false)}
          />
        )}
        {overlay === "archive" && (
          <ArchiveOverviewOverlay
            archive={metadata.data}
            categories={categories.data}
            loggedIn={loggedIn}
            currentPage={currentPage}
            onClose={() => setOverlay(null)}
            onSelectPage={selectPage}
            autoFocus={!openedByDefaultSetting.current}
            resolvePage={isTank ? tankReading.getArchiveForPage : undefined}
            tankChapters={isTank ? tankReading.chapters : undefined}
            tankPages={isTank ? pages.data.pages : undefined}
            hasTankSplitCandidates={tankSplitCandidates.length > 0}
            onOpenTankSplit={() => setShowTankSplit(true)}
            onShare={
              archiveId
                ? () => {
                    // Closed first so the share dialog is not opened underneath the overlay.
                    setOverlay(null);
                    setShareItem({ kind: isTank ? "tankoubon" : "archive", id: archiveId });
                  }
                : undefined
            }
          />
        )}

        {overlay === "settings" && (
          <SettingsOverlay
            settings={readerSettings}
            update={updateReaderSettings}
            onClose={() => setOverlay(null)}
            stampAutoBookmark={settings.data?.stampautobookmark ?? true}
            stampAutoUnbookmark={settings.data?.stampautounbookmark ?? true}
            onUpdateServerSetting={(partial) =>
              updateSettings.mutateAsync(partial)
            }
            loggedIn={loggedIn}
          />
        )}

        {overlay === "help" && (
          <>
            <div
              id="overlay-shade"
              style={{ display: "block", opacity: 0.6 }}
              onClick={() => setOverlay(null)}
            />
            <div id="reader-help" className="id1 base-overlay small-overlay">
              <div className="navigation-help-toast">{helpContent}</div>
            </div>
          </>
        )}

        {archiveTransition && (
          <>
            <div
              id="overlay-shade"
              style={{
                display: "block",
                opacity: 0.85,
                overscrollBehavior: "contain",
              }}
              onClick={() => setArchiveTransition(null)}
            />
            <div
              className="rec-overlay"
              onClick={() => setArchiveTransition(null)}
              style={{
                position: "fixed",
                top: "50%",
                left: 0,
                right: 0,
                transform: "translateY(-50%)",
                textAlign: "center",
                zIndex: 9001,
                background: "transparent",
                maxHeight: "95vh",
                overflowY: "auto",
                overscrollBehavior: "contain",
                paddingBottom: 16,
              }}
            >
              {metadata.data?.title && (
                <p
                  style={{
                    fontSize: 13,
                    color: "rgba(255,255,255,0.8)",
                    marginBottom: 4,
                  }}
                >
                  {t("reader.currentlyReading")}: {metadata.data.title}{" "}
                  <FormatBadge extension={metadata.data.extension} />
                </p>
              )}
              <p style={{ fontSize: 16, fontWeight: "bold", color: "#fff" }}>
                {t("reader.youMightAlsoLike")}
              </p>
              {archiveTransition.recommendations === null && (
                /* Skeleton while the (un-prefetched) LLM rerank is in flight — grey card shapes
                 matching the real cards' dimensions. */
                <div className="rec-row" aria-busy="true">
                  {Array.from({ length: 10 }).map((_, i) => (
                    <div
                      key={i}
                      className="rec-card rec-skeleton"
                      style={{
                        background: "rgba(255,255,255,0.1)",
                        border: "none",
                      }}
                    >
                      <div
                        style={{
                          width: "100%",
                          aspectRatio: "3 / 4",
                          borderRadius: 4,
                          background: "rgba(255,255,255,0.08)",
                        }}
                      />
                      <div
                        style={{
                          height: 11,
                          marginTop: 6,
                          width: "80%",
                          background: "rgba(255,255,255,0.08)",
                          borderRadius: 2,
                        }}
                      />
                    </div>
                  ))}
                </div>
              )}
              {archiveTransition.recommendations !== null &&
                archiveTransition.recommendations.length > 0 && (
                  /* 10 cards on desktop, 5 per row (two rows) — the fixed card width makes the
                 flex-wrap land at 5 across within this container's max width; narrow viewports
                 wrap to fewer per row naturally. */
                  <div className="rec-row">
                    {archiveTransition.recommendations
                      .slice(0, 10)
                      .map((rec) => (
                        <div key={rec.archive_id} className="rec-card">
                          <a
                            href={`/reader/${rec.archive_id}`}
                            title={rec.title}
                            style={{ display: "block", textDecoration: "none" }}
                            onClick={() => setArchiveTransition(null)}
                          >
                            <div style={{ position: "relative" }}>
                              <img
                                src={
                                  rec.archive_id.startsWith("TANK_")
                                    ? `/api/tankoubons/${rec.archive_id}/thumbnail?no_fallback=true`
                                    : `/api/archives/${rec.archive_id}/thumbnail?no_fallback=true`
                                }
                                alt={rec.title}
                                loading="lazy"
                              />
                              {(rec.isnew || rec.is_read || rec.is_tank) && (
                                <span
                                  style={{
                                    position: "absolute",
                                    top: 4,
                                    left: 4,
                                    fontSize: 10,
                                    lineHeight: 1,
                                    display: "flex",
                                    gap: 3,
                                  }}
                                >
                                  {rec.is_tank && (
                                    <span style={badgeChipStyle}>
                                      <StatusIcon
                                        kind="tankoubon"
                                        title={
                                          t("library.tankoubon") ?? undefined
                                        }
                                      />
                                    </span>
                                  )}
                                  {rec.isnew && (
                                    <span style={badgeChipStyle}>
                                      <StatusIcon
                                        kind="new"
                                        title={t("library.new") ?? undefined}
                                      />
                                    </span>
                                  )}
                                  {rec.is_read && (
                                    <span style={badgeChipStyle}>
                                      <StatusIcon
                                        kind="read"
                                        title={t("common.read") ?? undefined}
                                      />
                                    </span>
                                  )}
                                </span>
                              )}
                            </div>
                            <div className="rec-title">
                              <span>{rec.title}</span>
                              {rec.extension && (
                                <>
                                  {" "}
                                  <FormatBadge extension={rec.extension} />
                                </>
                              )}
                            </div>
                          </a>
                        </div>
                      ))}
                  </div>
                )}
              <div style={{ marginTop: 18 }}>
                <input
                  type="button"
                  className="stdbtn"
                  value={t("common.returnToLibrary") ?? undefined}
                  onClick={() => navigate(routes.library())}
                />
              </div>
            </div>
            <button
              type="button"
              aria-label={t("reader.close") ?? undefined}
              onClick={() => setArchiveTransition(null)}
              style={{
                position: "fixed",
                top: 16,
                right: 16,
                width: 36,
                height: 36,
                borderRadius: "50%",
                border: "none",
                cursor: "pointer",
                padding: 0,
                display: "inline-flex",
                alignItems: "center",
                justifyContent: "center",
                background: "rgba(255,255,255,0.2)",
                color: "#fff",
                zIndex: 9002,
              }}
              onMouseEnter={(e) =>
                (e.currentTarget.style.background = "rgba(255,255,255,0.45)")
              }
              onMouseLeave={(e) =>
                (e.currentTarget.style.background = "rgba(255,255,255,0.2)")
              }
            >
              <i className="fas fa-times" aria-hidden="true"></i>
            </button>
          </>
        )}
      </div>
      <Footer />
    </>
  );
}
