import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
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
  useLoginStatus,
  usePageDimensions,
  useRemoveBookmark,
  useSettings,
  useTankoubonFull,
  useUpdateProgress,
  useUpdateSettings,
  useUpdateTankoubonProgress,
} from "@/api/hooks";
import { Tooltip } from "@/components/common-ui/Display";
import { IconButton } from "@/components/common-ui/Form";
import { ForbiddenPage, NotFoundPage } from "@/components/Display";
import { Footer } from "@/components/Layout";
import { confirmDialog, promptDialog } from "@/dialog";
import { useDocumentTitle } from "@/hooks/useDocumentTitle";
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
import { fileInfoText } from "@/lib/utils/fileInfoText";
import {
  fetchContentLengthKb,
  fetchResizedPageInfo,
  type ResizedPageInfo,
} from "@/lib/utils/imageMeta";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { FONT_SIZE_XS, useApplyTheme } from "@/theme";
import { toast } from "@/toast";

import { ArchiveOverviewOverlay } from "./ArchiveOverviewOverlay";
import { BookmarkButton } from "./BookmarkButton";
import {
  type ArchiveNavState,
  resolveAdjacentArchive,
  setupArchiveNavigation,
} from "./crossArchiveNav";
import { MarkerLayer } from "./MarkerLayer";
import { SettingsOverlay } from "./SettingsOverlay";
import { TankoubonSplitModal } from "./TankoubonSplitModal";

// Port of legacy's reader page (`reader.html.tt2` + `reader.js`) — real DOM structure
// (`#i1`-`#i7`) and CSS classnames from `/legacy/lrr.css`, not Tailwind.

/** Uniform icon size (rem, not em) for every paginator nav link. */
const PAGINATOR_ICON_FONT_SIZE = "1.75rem";
/** How many prefetched/current Blob URLs to keep in memory. Beyond this the oldest non-current
 * page blob is revoked, keeping repeated paging fast without holding an entire archive in RAM. */
const MAX_PAGE_BLOB_CACHE = 8;

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
  const loginStatus = useLoginStatus();
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

  const clearArchiveNew = useClearArchiveNew();
  const clearArchiveNewRef = useRef(clearArchiveNew.mutate);
  clearArchiveNewRef.current = clearArchiveNew.mutate;
  const newBadgeMode = settings.data?.newbadgemode;
  useEffect(() => {
    if (!archiveId || isTank || !newBadgeMode) return;
    if (newBadgeMode === "until_opened") clearArchiveNewRef.current(archiveId);
  }, [archiveId, isTank, newBadgeMode]);

  const totalPages = pages.data?.pages.length ?? 0;
  const loggedIn = loginStatus.data?.logged_in ?? false;

  const params = new URLSearchParams(window.location.search);
  const startPage = Number(params.get("p")) || null;
  const startWithOverview = params.get("overview") === "1";

  const [pageOverride, setPageOverride] = useState<number | null>(startPage);
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
  const [pageDimensions, setPageDimensions] = useState<
    Record<number, { width: number; height: number }>
  >({});
  const [pageSizesKb, setPageSizesKb] = useState<Record<number, number>>({});
  const [resizedPageInfo, setResizedPageInfo] = useState<
    Record<number, ResizedPageInfo | null>
  >({});
  const [markerPlacementMode, setMarkerPlacementMode] = useState(false);
  const supportsHover = useSupportsHover();
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
        }[]
      | null;
  } | null>(null);
  const imageAreaRef = useRef<HTMLDivElement>(null);
  const lastSpreadHeightRef = useRef<number | null>(null);
  const lastFileInfoRef = useRef<string | null>(null);
  // Tracks pages whose Content-Length has already been requested, so the UI can show the file-size
  // part of the file-info bar while the actual image is still transferring (it used to wait for
  // `img.onload`, then issue a HEAD request on top of that — on mobile that made the text below a
  // page appear only after the whole large image had finished downloading).
  const prefetchedPageSizesRef = useRef<Set<number>>(new Set());
  const [pageBlobUrls, setPageBlobUrls] = useState<Record<number, string>>({});
  const [pageImageError, setPageImageError] = useState<Record<number, boolean>>({});
  const [pageTinyBlobs, setPageTinyBlobs] = useState<Record<number, string>>({});
  const [pageTinyDims, setPageTinyDims] = useState<
    Record<number, { width: number; height: number }>
  >({});
  const displayedLeftSrcRef = useRef<string | undefined>(undefined);
  const displayedRightSrcRef = useRef<string | undefined>(undefined);
  const pageBlobUrlsRef = useRef<Map<number, string>>(new Map());
  const pageImageFetchesRef = useRef<
    Map<number, { controller: AbortController; priority: "current" | "prefetch" }>
  >(new Map());
  const pageTinyBlobsRef = useRef<Map<number, string>>(new Map());
  const pageTinyDimsRef = useRef<Map<number, { width: number; height: number }>>(new Map());
  const pageTinyFetchesRef = useRef<Map<number, AbortController>>(new Map());
  const currentPageNumbersRef = useRef<number[]>([]);

  const localReaderProgress =
    archiveId && usesLocalReaderProgress(loggedIn, settings.data)
      ? Number(localStorage.getItem(`${archiveId}-reader`))
      : 0;

  const currentPage = clamp(
    pageOverride ??
      (readerSettings.ignoreProgress
        ? 1
        : Math.max(localReaderProgress || metadata.data?.progress || 1, 1)),
    1,
    totalPages || 1,
  );

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

  // Fetch page images via `fetch()` with an explicit `X-LRR-Priority` header instead of relying on
  // `<img src>` alone. This is what makes the Rust-side priority queue actually effective: the
  // server can tell a current-page request (`current`) from a speculative preload (`prefetch`),
  // which `<img>`/`<link rel=preload>` cannot express portably across browsers. Displaying the
  // result as a Blob URL also lets Firefox/Android use the same source regardless of its weaker
  // `fetchpriority` support.
  const fetchPageImage = useCallback(
    (page: number, priority: "current" | "prefetch") => {
      const url = pages.data?.pages[page - 1]?.url;
      if (!url) return;
      if (pageBlobUrlsRef.current.has(page)) return;
      const existing = pageImageFetchesRef.current.get(page);
      if (existing && existing.priority === priority) return;
      existing?.controller.abort();
      const controller = new AbortController();
      pageImageFetchesRef.current.set(page, { controller, priority });
      const fullUrl = new URL(url, window.location.origin);
      fullUrl.searchParams.set("max_short_edge", String(readerFullShortEdge()));
      fetch(fullUrl.toString(), {
        headers: { "X-LRR-Priority": priority },
        signal: controller.signal,
      })
        .then(async (res) => {
          if (!res.ok) throw new Error(`page fetch failed: ${res.status}`);
          return res.blob();
        })
        .then((blob) => {
          if (pageImageFetchesRef.current.get(page)?.controller !== controller) return;
          const blobUrl = URL.createObjectURL(blob);
          const map = pageBlobUrlsRef.current;
          const old = map.get(page);
          if (old) URL.revokeObjectURL(old);
          map.delete(page);
          map.set(page, blobUrl);
          while (map.size > MAX_PAGE_BLOB_CACHE) {
            const oldest = map.keys().next().value;
            if (oldest === undefined || currentPageNumbersRef.current.includes(oldest)) break;
            const victimUrl = map.get(oldest);
            if (
              victimUrl === displayedLeftSrcRef.current ||
              victimUrl === displayedRightSrcRef.current
            ) {
              // This blob is the stale image still shown (and blurred) while the next page loads;
              // revoking it now would make the reader flash blank instead of keeping the transition.
              break;
            }
            map.delete(oldest);
            if (victimUrl) URL.revokeObjectURL(victimUrl);
          }
          setPageBlobUrls(Object.fromEntries(map));
          const tinyUrl = pageTinyBlobsRef.current.get(page);
          if (tinyUrl) {
            URL.revokeObjectURL(tinyUrl);
            pageTinyBlobsRef.current.delete(page);
            setPageTinyBlobs(Object.fromEntries(pageTinyBlobsRef.current));
          }
          setPageImageError((prev) => ({ ...prev, [page]: false }));
          pageImageFetchesRef.current.delete(page);
        })
        .catch((err: unknown) => {
          if (pageImageFetchesRef.current.get(page)?.controller !== controller) return;
          pageImageFetchesRef.current.delete(page);
          if (err instanceof DOMException && err.name === "AbortError") return;
          setPageImageError((prev) => ({ ...prev, [page]: true }));
        });
    },
    [pages.data, setPageBlobUrls, setPageImageError, setPageTinyBlobs],
  );

  // Fetches a much smaller WebP preview for the current page only. It is shown immediately so the
  // reader never stares at a blank/old page even on slow connections; when the full-resolution blob
  // arrives, the UI swaps it in. The preview is intentionally excluded from file-info metadata.
  const fetchTinyPreview = useCallback(
    (page: number) => {
      const url = pages.data?.pages[page - 1]?.url;
      if (!url) return;
      if (pageBlobUrlsRef.current.has(page) || pageTinyBlobsRef.current.has(page)) return;
      const existing = pageTinyFetchesRef.current.get(page);
      if (existing) return;
      const controller = new AbortController();
      pageTinyFetchesRef.current.set(page, controller);
      const tinyUrl = new URL(url, window.location.origin);
      tinyUrl.searchParams.set("variant", "tiny");
      fetch(tinyUrl.toString(), {
        headers: { "X-LRR-Priority": "preview" },
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
    if (displayedLeftSrcRef.current === undefined && pageNumbers.length > 0) {
      for (const page of pageNumbers) {
        if (page < 1) continue;
        fetchTinyPreview(page);
        fetchPageImage(page, "current");
      }
    }
    // Abort any still-in-flight request for a page that is no longer on screen. Under a slow/throttled
    // network these stale downloads otherwise keep occupying browser connections and make the newest
    // current page wait behind pages the user already flipped past.
    for (const [page, fetch] of [...pageImageFetchesRef.current]) {
      if (!pageNumbers.includes(page)) {
        fetch.controller.abort();
      }
    }
  }, [spread.left, spread.right, fetchPageImage, fetchTinyPreview, readerSettings.infiniteScroll]);

  // Tiny previews are only for the page the user actually stops on. Starting one on every
  // intermediate page during a rapid flip would add extra small requests that are almost always
  // wasted. Wait for the current page to settle before creating it.
  useEffect(() => {
    if (readerSettings.infiniteScroll) return;
    const timer = window.setTimeout(() => {
      const pageNumbers =
        spread.right === null ? [spread.left] : [spread.left, spread.right];
      for (const page of pageNumbers) {
        if (page < 1) continue;
        // Tiny first: the reader gets a small preview as soon as the user stops. Full starts right
        // after so the seamless high-quality replacement follows in the background.
        fetchTinyPreview(page);
        fetchPageImage(page, "current");
      }
    }, 300);
    return () => window.clearTimeout(timer);
  }, [spread.left, spread.right, readerSettings.infiniteScroll, fetchTinyPreview, fetchPageImage]);

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
    const pageList = pages.data;
    // Debounce prefetching: while the user is rapidly clicking next/prev, speculative requests for
    // every intermediate page (5, 6, 7, ...) only queue up behind the page the reader actually
    // needs to show right now. Waiting until the current page settles means those low-priority
    // prefetch images are only requested once the user has stopped flipping, so the final page gets
    // the full queue of browser/network capacity in the meantime.
    const timer = window.setTimeout(() => {
      const prefetchPages: number[] = [];
      for (let offset = 1; offset <= readerSettings.preloadCount; offset++) {
        const nextPage = currentPage + offset;
        if (nextPage <= totalPages) {
          const url = pageList.pages[nextPage - 1]?.url;
          if (url) prefetchPages.push(nextPage);
        }
        const prevPage = currentPage - offset;
        if (prevPage >= 1) {
          const url = pageList.pages[prevPage - 1]?.url;
          if (url) prefetchPages.push(prevPage);
        }
      }
      for (const page of prefetchPages) {
        fetchPageImage(page, "prefetch");
      }
    }, 300);
    return () => window.clearTimeout(timer);
  }, [pages.data, currentPage, totalPages, readerSettings.preloadCount, readerSettings.infiniteScroll, fetchPageImage]);

  // Revoke all Blob URLs and abort in-flight fetches when the reader unmounts.
  useEffect(() => {
    const fetches = [...pageImageFetchesRef.current.values()];
    for (const fetch of fetches) fetch.controller.abort();
    pageImageFetchesRef.current.clear();
    const blobUrls = [...pageBlobUrlsRef.current.values()];
    for (const url of blobUrls) URL.revokeObjectURL(url);
    pageBlobUrlsRef.current.clear();
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
    setPageDimensions((prev) => ({
      ...prev,
      [page]: { width: img.naturalWidth, height: img.naturalHeight },
    }));
    // Remember what bitmap is currently on screen. When the reader flips to a new page before its
    // Blob is ready, this keeps the old image visible (and blurred by `#i3.loading`) instead of
    // clearing `src` and showing a blank flash.
    if (page === spread.left) displayedLeftSrcRef.current = img.src;
    if (page === spread.right) displayedRightSrcRef.current = img.src;
    // `img.src` may be a Blob URL now (fetch-based reader images), so the file-info HEADs must
    // always use the original server URL to keep Content-Length/resize metadata working.
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

  const markerTarget = archiveId ? { arcId: archiveId, localPage: spread.left } : null;

  const isSpreadShowing = spread.right !== null;
  const imageStyle: React.CSSProperties = {};
  const outerStyle: React.CSSProperties = {};
  if (!isFullscreen) {
    if (readerSettings.fitMode === FIT_MODE.FIT_HEIGHT) {
      const heightVh =
        readerSettings.hideHeader || readerSettings.infiniteScroll ? 98 : 90;
      imageStyle.maxHeight = `${heightVh}vh`;
      outerStyle.width = "fit-content";
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
        <IconButton
          variant="ghost-btn"
          icon={`fas ${isFullscreen ? "fa-compress" : "fa-expand"} fa-2x`}
          title={t("reader.fullscreen") ?? undefined}
          size={32}
          style={{ marginRight: 13, borderRadius: "50%" }}
          onClick={() => {
            toggleFullScreen();
          }}
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
  const servedKb = spread.right === null ? pageSizesKb[spread.left] : undefined;
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
          {pageEntryName} → WebP :: {resizedDims.width} x {resizedDims.height} ::{" "}
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
  const archiveHeading = artistMatch ? (
    <>
      {metadata.data.title} by{" "}
      <a href={getTagSearchURL("artist", artistMatch[1])}>{artistMatch[1]}</a>
    </>
  ) : (
    metadata.data.title
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
            <div id="display" ref={infiniteScrollRootRef}>
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
                <img
                  id="img"
                  ref={leftImgRef}
                  className="reader-image"
                  src={pageBlobUrls[spread.left] ?? (pageImageError[spread.left] ? leftUrl : (pageTinyBlobs[spread.left] ?? displayedLeftSrcRef.current))}
                  width={pageTinyDims[spread.left]?.width}
                  height={pageTinyDims[spread.left]?.height}
                  alt={`${t("reader.page")} ${spread.left}`}
                  fetchPriority="high"
                  onLoad={(e) => onImageLoad(spread.left, e)}
                  onError={onImageError}
                  draggable={false}
                  style={placementImageStyle}
                />
                {rightUrl && (
                  <img
                    id="img_doublepage"
                    className="reader-image"
                    src={rightUrl ? (pageBlobUrls[spread.right ?? 0] ?? (pageImageError[spread.right ?? 0] ? rightUrl : (pageTinyBlobs[spread.right ?? 0] ?? displayedRightSrcRef.current))) : undefined}
                    width={pageTinyDims[spread.right ?? 0]?.width}
                    height={pageTinyDims[spread.right ?? 0]?.height}
                    alt={`${t("reader.page")} ${spread.right}`}
                    fetchPriority="high"
                    onLoad={(e) => onImageLoad(spread.right ?? 0, e)}
                    onError={onImageError}
                    draggable={false}
                    style={imageStyle}
                  />
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
          {pagesel}
          {arrows}
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
            </>
          )}
        </div>

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
                  {t("reader.currentlyReading")}: {metadata.data.title}
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
                                    <span
                                      title={
                                        t("library.tankoubon") ?? undefined
                                      }
                                      style={badgeChipStyle}
                                    >
                                      📚
                                    </span>
                                  )}
                                  {rec.isnew && (
                                    <span
                                      title={t("library.new") ?? undefined}
                                      style={badgeChipStyle}
                                    >
                                      🆕
                                    </span>
                                  )}
                                  {rec.is_read && (
                                    <span
                                      title={t("common.read") ?? undefined}
                                      style={badgeChipStyle}
                                    >
                                      👑
                                    </span>
                                  )}
                                </span>
                              )}
                            </div>
                            <div className="rec-title">
                              <span>{rec.title}</span>
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
