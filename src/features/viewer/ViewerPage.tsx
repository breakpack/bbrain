import type { PDFPageProxy } from "pdfjs-dist";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  Languages,
  Loader2,
  Minus,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import { selectionToRects, splitAroundWord, wordAtPoint } from "@/lib/pdf";
import { HIGHLIGHT_COLORS, type HighlightColor, type NormalizedRect } from "@/lib/types";
import { PdfPage } from "./PdfPage";
import { PdfThumbnail } from "./PdfThumbnail";
import { HighlightsTab } from "./HighlightsTab";
import { TranslationTab } from "./TranslationTab";
import { AnalysisTab } from "./AnalysisTab";
import {
  useDeleteHighlight,
  useHighlights,
  usePdfDocument,
  useSaveHighlight,
  useTrimHighlight,
  useViewerDocument,
} from "./queries";

const ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3];
/** Continuous zoom bounds for trackpad pinch (the step buttons stay discrete). */
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 4;
const PANEL_MIN = 280;
const PANEL_MAX = 720;
const PANEL_DEFAULT = 380;
/** Resizable thumbnail rail bounds. */
const RAIL_MIN = 110;
const RAIL_MAX = 340;
const RAIL_DEFAULT = 160;
/** Below this width the right panel overlays instead of splitting (§9.1). */
const OVERLAY_BREAKPOINT = 1180;

type Tab = "translation" | "highlights" | "analysis";

export function ViewerPage({
  paperId,
  onBack,
}: {
  paperId: string;
  onBack: () => void;
}) {
  const document = useViewerDocument(paperId);
  const { pdf, error: pdfError } = usePdfDocument(paperId);
  const highlights = useHighlights(paperId);
  const saveHighlight = useSaveHighlight(paperId);
  const trimHighlight = useTrimHighlight(paperId);
  const deleteHighlight = useDeleteHighlight(paperId);

  const [scale, setScale] = useState(1);
  const [currentPage, setCurrentPage] = useState(1);
  const [tab, setTab] = useState<Tab>("translation");
  const [panelWidth, setPanelWidth] = useState(() => {
    const stored = Number(localStorage.getItem("bbrain.viewer.panelWidth"));
    return Number.isFinite(stored) && stored >= PANEL_MIN && stored <= PANEL_MAX
      ? stored
      : PANEL_DEFAULT;
  });
  useEffect(() => {
    localStorage.setItem("bbrain.viewer.panelWidth", String(panelWidth));
  }, [panelWidth]);
  const [railWidth, setRailWidth] = useState(RAIL_DEFAULT);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [pages, setPages] = useState<PDFPageProxy[]>([]);
  const [sourceRects, setSourceRects] = useState<{ page: number; rects: NormalizedRect[] }>({
    page: 0,
    rects: [],
  });
  const [pulse, setPulse] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The source sentence the cursor is over on the page, so the translation panel
  // can highlight the matching unit (the reverse of hovering the translation).
  const [hoveredSentenceId, setHoveredSentenceId] = useState<string | null>(null);

  const hasTextLayer = document.data?.hasTextLayer ?? false;
  // Sentences of the page being read, used to hit-test cursor position on the
  // PDF. Same query key as the translation panel, so it is fetched once.
  const pageSentences = useQuery({
    queryKey: ["sentences", paperId, currentPage],
    queryFn: () => api.getPageSentences(paperId, currentPage),
    enabled: hasTextLayer,
  });

  const scrollRef = useRef<HTMLDivElement>(null);
  const textLayers = useRef(new Map<number, HTMLElement>());
  const overlay = useOverlayLayout();

  useEffect(() => {
    if (!pdf) {
      setPages([]);
      return;
    }
    let cancelled = false;

    void Promise.all(
      Array.from({ length: pdf.numPages }, (_, index) => pdf.getPage(index + 1)),
    ).then((loaded) => {
      if (!cancelled) setPages(loaded);
    });

    return () => {
      cancelled = true;
    };
  }, [pdf]);

  // Trackpad pinch-zoom. A browser delivers a pinch as a wheel event with
  // ctrlKey set; a native non-passive listener is required so preventDefault can
  // suppress the browser's own page zoom. Zoom is continuous within [MIN, MAX].
  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      setScale((current) => {
        const next = current * Math.exp(-event.deltaY / 100);
        return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(next * 100) / 100));
      });
    };
    root.addEventListener("wheel", onWheel, { passive: false });
    return () => root.removeEventListener("wheel", onWheel);
  }, []);

  const onTextLayerReady = useCallback((pageNumber: number, layer: HTMLElement | null) => {
    if (layer) textLayers.current.set(pageNumber, layer);
    else textLayers.current.delete(pageNumber);
  }, []);

  // The current page is the one crossing a reference line near the top of the
  // viewport. Geometry, not intersection ratio, so it stays correct when a page
  // is zoomed larger than the viewport — that page has a low visible ratio but
  // is still the one being read. Drives the translation panel and the active
  // thumbnail (§9.4).
  useEffect(() => {
    const root = scrollRef.current;
    if (!root || pages.length === 0) return;

    let frame = 0;
    const update = () => {
      frame = 0;
      const rootRect = root.getBoundingClientRect();
      const referenceY = rootRect.top + rootRect.height * 0.25;

      let current = 1;
      for (const element of root.querySelectorAll("[data-page-number]")) {
        const rect = element.getBoundingClientRect();
        if (rect.top - 1 <= referenceY) {
          current = Number(element.getAttribute("data-page-number"));
        } else {
          break; // pages are in document order, so we can stop at the first below
        }
      }
      setCurrentPage(current);
    };

    const onScroll = () => {
      if (frame === 0) frame = window.requestAnimationFrame(update);
    };

    root.addEventListener("scroll", onScroll, { passive: true });
    update();
    return () => {
      root.removeEventListener("scroll", onScroll);
      if (frame) window.cancelAnimationFrame(frame);
    };
  }, [pages]);

  const goToPage = useCallback((pageNumber: number) => {
    const root = scrollRef.current;
    const target = root?.querySelector(`[data-page-number="${pageNumber}"]`);
    target?.scrollIntoView({ behavior: "smooth", block: "start" });
  }, []);

  /** Hovering a translated sentence highlights its source rectangles. */
  const showSource = useCallback(
    (pageNumber: number, rects: NormalizedRect[], jump: boolean) => {
      setSourceRects({ page: pageNumber, rects });
      if (jump) {
        goToPage(pageNumber);
        setPulse(true);
        window.setTimeout(() => setPulse(false), 600);
      }
    },
    [goToPage],
  );

  // Word-level highlight deletion. Removing a word in the middle splits the
  // highlight into two — before and after — so the right panel shows them as two
  // separate entries; a word at an edge just trims; the last word deletes it.
  const onWordDelete = useCallback(
    async (highlightId: string, wordRect: NormalizedRect, wordText: string) => {
      const target = (highlights.data ?? []).find((highlight) => highlight.id === highlightId);
      if (!target) return;

      const { before, after } = splitAroundWord(target.rects, wordRect);
      const index = target.selectedText.indexOf(wordText);
      const textBefore =
        index >= 0 ? target.selectedText.slice(0, index).trim() : target.selectedText;
      const textAfter = index >= 0 ? target.selectedText.slice(index + wordText.length).trim() : "";

      if (before.length === 0 && after.length === 0) {
        deleteHighlight.mutate(highlightId);
        return;
      }

      // Only one side survives — trim the existing row rather than resplitting it.
      if (before.length === 0 || after.length === 0) {
        const rects = before.length === 0 ? after : before;
        const selectedText = before.length === 0 ? textAfter : textBefore;
        trimHighlight.mutate({ id: highlightId, rects, selectedText });
        return;
      }

      // A middle word: replace the one highlight with two.
      try {
        await deleteHighlight.mutateAsync(highlightId);
        for (const [rects, text] of [
          [before, textBefore],
          [after, textAfter],
        ] as const) {
          await saveHighlight.mutateAsync({
            paperId,
            color: target.color,
            note: target.note ?? undefined,
            pages: [{ pageNumber: target.pageNumber, selectedText: text, rects }],
          });
        }
      } catch (cause) {
        setError(errorMessage(cause));
      }
    },
    [highlights.data, paperId, deleteHighlight, trimHighlight, saveHighlight],
  );

  // A plain click on a highlighted word hit-tests the point against this page's
  // highlights, then trims the clicked word out.
  const handleWordClick = useCallback(
    (x: number, y: number) => {
      const caret = caretInTextLayer(x, y);
      const layer = caret && textLayerOf(caret.node);
      if (!layer) return;

      let pageNumber: number | null = null;
      for (const [pn, element] of textLayers.current) {
        if (element === layer) pageNumber = pn;
      }
      if (pageNumber == null) return;

      const box = layer.getBoundingClientRect();
      const nx = (x - box.left) / box.width;
      const ny = (y - box.top) / box.height;
      const hit = (highlights.data ?? []).find(
        (highlight) =>
          highlight.pageNumber === pageNumber &&
          highlight.rects.some(
            (rect) =>
              nx >= rect.x && nx <= rect.x + rect.width && ny >= rect.y && ny <= rect.y + rect.height,
          ),
      );
      if (!hit) return;

      // Resolve the word from item geometry (whitespace-delimited), never from
      // the DOM caret — fallback-font layout puts the caret on the wrong glyph.
      const word = wordAtPoint(layer, nx, ny);
      if (!word || word.text.trim().length === 0) return;
      void onWordDelete(hit.id, word.rect, word.text.trim());
    },
    [textLayers, highlights.data, onWordDelete],
  );

  const selection = useDragSelection(textLayers, scrollRef, handleWordClick);

  // Native selection is disabled, so copy is wired explicitly: ⌘/Ctrl+C copies the
  // current custom selection's text.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "c") return;
      const pages = selection.current?.pages;
      if (!pages || pages.length === 0) return;
      const text = pages.map((page) => page.selectedText).join("\n").trim();
      if (text) void navigator.clipboard?.writeText(text);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection.current]);

  // A translation of the current selection, shown in a popover under the toolbar
  // — the "이 부분만 번역" action. Kept separate from highlights so the reader can
  // translate a span without committing it to the highlight list (§9.3).
  const [snippet, setSnippet] = useState<{ status: "loading" | "done" | "error"; text: string } | null>(
    null,
  );

  const applyHighlight = async (color: HighlightColor) => {
    if (!selection.current) return;
    setError(null);
    try {
      await saveHighlight.mutateAsync({
        paperId,
        color,
        pages: selection.current.pages,
      });
      selection.clear();
      setSnippet(null);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  // A new drag replaces the selection, so any snippet from the previous one is
  // stale — drop it. Keyed on the selected text so re-capturing the same span
  // (e.g. after a re-render) does not wipe a just-shown translation.
  const selectionText = selection.current?.pages.map((page) => page.selectedText).join(" ") ?? "";
  useEffect(() => {
    setSnippet(null);
  }, [selectionText]);

  const translateSelection = async () => {
    if (!selection.current) return;
    const text = selection.current.pages.map((page) => page.selectedText).join(" ").trim();
    if (text.length === 0) return;

    setSnippet({ status: "loading", text: "" });
    try {
      const translated = await api.translateSelection(text);
      setSnippet({ status: "done", text: translated });
    } catch (cause) {
      setSnippet({ status: "error", text: errorMessage(cause) });
    }
  };

  const explainSelection = async () => {
    if (!selection.current) return;
    const text = selection.current.pages.map((page) => page.selectedText).join(" ").trim();
    if (text.length === 0) return;

    setSnippet({ status: "loading", text: "" });
    try {
      const explanation = await api.explainSelection(text);
      setSnippet({ status: "done", text: explanation });
    } catch (cause) {
      setSnippet({ status: "error", text: errorMessage(cause) });
    }
  };

  const dismissSelection = () => {
    selection.clear();
    setSnippet(null);
  };

  if (document.isError || pdfError) {
    return (
      <div className="flex h-full items-center justify-center p-section">
        <Card className="flex max-w-[480px] flex-col gap-md">
          <CardTitle>논문을 열지 못했습니다</CardTitle>
          <CardDescription>
            {errorMessage(document.error ?? pdfError)} 라이브러리에서 다시 시도하거나 파일을
            다시 가져오세요.
          </CardDescription>
          <div>
            <Button variant="outline" onClick={onBack}>
              라이브러리로 돌아가기
            </Button>
          </div>
        </Card>
      </div>
    );
  }

  const paper = document.data?.paper;
  const pageCount = pdf?.numPages ?? paper?.pageCount ?? 0;

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-md border-b border-line bg-canvas px-md py-sm">
        <Button variant="ghost" size="sm" onClick={onBack} aria-label="라이브러리로 돌아가기">
          <ArrowLeft aria-hidden className="h-[18px] w-[18px]" />
        </Button>

        <button
          aria-label={sidebarOpen ? "썸네일 접기" : "썸네일 펼치기"}
          onClick={() => setSidebarOpen((open) => !open)}
          className="rounded-control p-2 text-ink-body hover:text-primary"
        >
          {sidebarOpen ? (
            <PanelLeftClose aria-hidden className="h-[18px] w-[18px]" />
          ) : (
            <PanelLeftOpen aria-hidden className="h-[18px] w-[18px]" />
          )}
        </button>

        <h1 className="flex-1 truncate text-caption font-medium text-ink-heading">
          {paper?.title ?? "불러오는 중"}
        </h1>

        <div className="flex items-center gap-1">
          <label className="sr-only" htmlFor="page-input">
            페이지 번호
          </label>
          <input
            id="page-input"
            type="number"
            min={1}
            max={pageCount || 1}
            value={currentPage}
            onChange={(event) => {
              const next = Number(event.target.value);
              if (next >= 1 && next <= pageCount) {
                setCurrentPage(next);
                goToPage(next);
              }
            }}
            className="w-14 rounded-control border border-line px-2 py-1 text-center text-caption"
          />
          <span className="text-caption text-ink-body">/ {pageCount}</span>
        </div>

        <div className="flex items-center gap-1">
          <button
            aria-label="축소"
            onClick={() => setScale(previousStep)}
            className="rounded-control p-2 text-ink-body hover:text-primary"
          >
            <Minus aria-hidden className="h-[18px] w-[18px]" />
          </button>
          <span className="w-12 text-center text-caption text-ink-body">
            {Math.round(scale * 100)}%
          </span>
          <button
            aria-label="확대"
            onClick={() => setScale(nextStep)}
            className="rounded-control p-2 text-ink-body hover:text-primary"
          >
            <Plus aria-hidden className="h-[18px] w-[18px]" />
          </button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        {sidebarOpen && (
          <ThumbnailRail
            pages={pages}
            pageCount={pageCount}
            currentPage={currentPage}
            onSelect={goToPage}
            width={railWidth}
            onResize={setRailWidth}
          />
        )}

        <div
          ref={scrollRef}
          className="relative min-w-0 flex-1 overflow-auto bg-canvas-soft p-lg"
          onMouseDown={selection.onMouseDown}
          onMouseMove={selection.onMouseMove}
          onMouseUp={selection.onMouseUp}
        >
          {pages.length === 0 ? (
            <div className="flex h-full items-center justify-center" aria-busy="true">
              <Loader2 aria-hidden className="h-6 w-6 animate-spin text-ink-body" />
            </div>
          ) : (
            <div className="flex flex-col items-center gap-lg">
              {pages.map((page) => (
                <PdfPage
                  key={page.pageNumber}
                  page={page}
                  scale={scale}
                  highlights={(highlights.data ?? []).filter(
                    (highlight) => highlight.pageNumber === page.pageNumber,
                  )}
                  sourceRects={
                    sourceRects.page === page.pageNumber ? sourceRects.rects : []
                  }
                  pulse={pulse}
                  sentences={
                    page.pageNumber === currentPage ? pageSentences.data ?? [] : []
                  }
                  selectionRects={selection.preview[page.pageNumber] ?? []}
                  onHoverSource={setHoveredSentenceId}
                  onTextLayerReady={onTextLayerReady}
                />
              ))}
            </div>
          )}

          {selection.current && (
            <div
              className="fixed z-20 flex flex-col gap-1"
              style={{ left: selection.current.x, top: selection.current.y }}
              // The toolbar sits inside the scroll container, which drives the
              // drag-selection on mousedown/up. A mousedown on the toolbar must
              // NOT reach that handler: selection.onMouseDown clears the current
              // selection (setCurrent(null)), which unmounts the toolbar before
              // the click can land — so the button's onClick never fires. Stop
              // propagation on BOTH mousedown and mouseup; preventDefault keeps
              // the pending selection intact for the action.
              onMouseDown={(event) => {
                event.preventDefault();
                event.stopPropagation();
              }}
              onMouseUp={(event) => event.stopPropagation()}
            >
              <div
                role="toolbar"
                aria-label="선택 영역 도구"
                className="flex items-center gap-1 rounded-control border border-line bg-canvas p-1 shadow-card"
              >
                {HIGHLIGHT_COLORS.map((color) => (
                  <button
                    key={color}
                    aria-label={`${colorLabel(color)}으로 하이라이트`}
                    onClick={() => void applyHighlight(color)}
                    className="h-6 w-6 rounded-sm border border-line"
                    style={{ background: swatch(color) }}
                  />
                ))}
                <span aria-hidden className="mx-0.5 h-5 w-px bg-line" />
                <button
                  aria-label="이 부분 번역"
                  onClick={() => void translateSelection()}
                  className="flex items-center gap-1 rounded-sm px-2 py-1 text-caption text-ink-body hover:text-primary"
                >
                  <Languages aria-hidden className="h-4 w-4" />
                  번역
                </button>
                <button
                  aria-label="이 부분 AI 설명"
                  onClick={() => void explainSelection()}
                  className="flex items-center gap-1 rounded-sm px-2 py-1 text-caption text-ink-body hover:text-primary"
                >
                  <Sparkles aria-hidden className="h-4 w-4" />
                  설명
                </button>
                <button
                  aria-label="선택 취소"
                  onClick={dismissSelection}
                  className="rounded-sm p-1 text-ink-body hover:text-danger"
                >
                  <Trash2 aria-hidden className="h-4 w-4" />
                </button>
              </div>

              {snippet && (
                <div className="max-w-[360px] rounded-control border border-line bg-canvas p-md shadow-card">
                  {snippet.status === "loading" && (
                    <div className="flex items-center gap-2 text-caption text-ink-body" aria-busy="true">
                      <Loader2 aria-hidden className="h-4 w-4 animate-spin" />
                      번역하는 중…
                    </div>
                  )}
                  {snippet.status === "done" && (
                    <p className="text-caption text-ink">{snippet.text}</p>
                  )}
                  {snippet.status === "error" && (
                    <p role="alert" className="text-caption text-danger">
                      {snippet.text}
                    </p>
                  )}
                </div>
              )}
            </div>
          )}
        </div>

        <aside
          aria-label="논문 도구"
          className={cn(
            "flex shrink-0 flex-col border-l border-line bg-canvas",
            overlay && "absolute right-0 top-[49px] z-10 h-[calc(100%-49px)] shadow-card",
          )}
          style={{ width: panelWidth }}
        >
          <PanelResizer width={panelWidth} onResize={setPanelWidth} />

          <div role="tablist" aria-label="도구 탭" className="flex border-b border-line">
            {(
              [
                ["translation", "번역"],
                ["highlights", "하이라이트"],
                ["analysis", "AI 정리"],
              ] as Array<[Tab, string]>
            ).map(([id, label]) => (
              <button
                key={id}
                role="tab"
                id={`tab-${id}`}
                aria-selected={tab === id}
                aria-controls={`panel-${id}`}
                onClick={() => setTab(id)}
                className={cn(
                  "flex-1 border-b-2 px-md py-sm text-caption transition-colors duration-fast",
                  tab === id
                    ? "border-primary text-primary"
                    : "border-transparent text-ink-body hover:text-ink",
                )}
              >
                {label}
              </button>
            ))}
          </div>

          <div
            role="tabpanel"
            id={`panel-${tab}`}
            aria-labelledby={`tab-${tab}`}
            className="min-h-0 flex-1 overflow-y-auto"
          >
            {tab === "translation" && (
              <TranslationTab
                paperId={paperId}
                pageNumber={currentPage}
                pageCount={pageCount}
                hasTextLayer={hasTextLayer}
                hoveredSentenceId={hoveredSentenceId}
                onHoverSentence={showSource}
              />
            )}
            {tab === "highlights" && (
              <HighlightsTab paperId={paperId} onJump={(page) => goToPage(page)} />
            )}
            {tab === "analysis" && <AnalysisTab paperId={paperId} onJump={goToPage} />}
          </div>
        </aside>
      </div>

      {error && (
        <p role="alert" className="border-t border-line bg-canvas px-md py-sm text-caption text-danger">
          {error}
        </p>
      )}
    </div>
  );
}

// Fixed render resolution; the thumbnail then CSS-scales to the rail width.
const THUMBNAIL_WIDTH = 180;

/**
 * The page-preview rail. Thumbnails render lazily, the active one follows the
 * document scroll (driven by `currentPage`), and the rail scrolls it back into
 * view so the current page is always visible without the user scrolling here.
 */
function ThumbnailRail({
  pages,
  pageCount,
  currentPage,
  onSelect,
  width,
  onResize,
}: {
  pages: PDFPageProxy[];
  pageCount: number;
  currentPage: number;
  onSelect: (pageNumber: number) => void;
  width: number;
  onResize: (width: number) => void;
}) {
  const activeRef = useRef<HTMLButtonElement>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const userScrolling = useRef(false);
  const releaseTimer = useRef<number | undefined>(undefined);

  // Keep the active thumbnail visible as the document scrolls — but not while
  // the user is scrolling the rail itself, so their manual scroll is not fought.
  useEffect(() => {
    if (userScrolling.current) return;
    activeRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [currentPage]);

  const onRailScroll = () => {
    userScrolling.current = true;
    window.clearTimeout(releaseTimer.current);
    releaseTimer.current = window.setTimeout(() => {
      userScrolling.current = false;
    }, 1200);
  };

  return (
    <div ref={wrapperRef} className="relative shrink-0" style={{ width }}>
      <nav
        aria-label="페이지 썸네일"
        onWheel={onRailScroll}
        className="h-full overflow-y-auto border-r border-line bg-canvas-soft p-sm"
      >
      <ul className="flex flex-col gap-sm">
        {Array.from({ length: pageCount }, (_, index) => index + 1).map((number) => {
          const active = currentPage === number;
          const page = pages.find((candidate) => candidate.pageNumber === number);

          return (
            <li key={number}>
              <button
                ref={active ? activeRef : undefined}
                onClick={() => onSelect(number)}
                aria-current={active ? "page" : undefined}
                aria-label={`${number}쪽`}
                className={cn(
                  "flex w-full flex-col items-center gap-1 rounded-control p-2",
                  "transition-colors duration-fast",
                  active ? "bg-canvas shadow-card" : "hover:bg-canvas",
                )}
              >
                <span
                  className={cn(
                    "w-full overflow-hidden rounded-sm",
                    // The active page is ringed in green — the one place green
                    // marks selection (DESIGN.md §7).
                    active && "ring-2 ring-primary",
                  )}
                >
                  {page ? (
                    <PdfThumbnail page={page} width={THUMBNAIL_WIDTH} />
                  ) : (
                    <span className="flex aspect-[3/4] w-full items-center justify-center border border-line bg-canvas text-caption text-ink-body">
                      {number}
                    </span>
                  )}
                </span>
                <span
                  className={cn(
                    "text-caption",
                    active ? "font-medium text-primary" : "text-ink-body",
                  )}
                >
                  {number}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
      </nav>
      <RailResizer width={width} onResize={onResize} edgeRef={wrapperRef} />
    </div>
  );
}

/** Drag handle on the rail's right edge; width follows the cursor from the
 * rail's left edge, clamped to the rail bounds. */
function RailResizer({
  width,
  onResize,
  edgeRef,
}: {
  width: number;
  onResize: (width: number) => void;
  edgeRef: React.RefObject<HTMLDivElement | null>;
}) {
  const dragging = useRef(false);

  useEffect(() => {
    const onMove = (event: MouseEvent) => {
      if (!dragging.current) return;
      const left = edgeRef.current?.getBoundingClientRect().left ?? 0;
      onResize(Math.min(RAIL_MAX, Math.max(RAIL_MIN, event.clientX - left)));
    };
    const onUp = () => {
      dragging.current = false;
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [onResize, edgeRef]);

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="썸네일 너비 조절"
      aria-valuenow={width}
      aria-valuemin={RAIL_MIN}
      aria-valuemax={RAIL_MAX}
      tabIndex={0}
      onMouseDown={() => {
        dragging.current = true;
        document.body.style.cursor = "col-resize";
      }}
      onKeyDown={(event) => {
        if (event.key === "ArrowRight") onResize(Math.min(RAIL_MAX, width + 20));
        if (event.key === "ArrowLeft") onResize(Math.max(RAIL_MIN, width - 20));
      }}
      className="absolute right-0 top-0 z-10 h-full w-1.5 cursor-col-resize hover:bg-primary/30"
    />
  );
}

function previousStep(current: number): number {
  const index = ZOOM_STEPS.findIndex((step) => step >= current);
  return ZOOM_STEPS[Math.max(0, index - 1)];
}

function nextStep(current: number): number {
  const index = ZOOM_STEPS.findIndex((step) => step > current);
  return index === -1 ? ZOOM_STEPS[ZOOM_STEPS.length - 1] : ZOOM_STEPS[index];
}

function colorLabel(color: HighlightColor): string {
  return { yellow: "노랑", green: "초록", blue: "파랑", pink: "분홍", purple: "보라" }[color];
}

function swatch(color: HighlightColor): string {
  return {
    yellow: "rgba(250, 204, 21, 0.6)",
    green: "rgba(0, 196, 115, 0.5)",
    blue: "rgba(59, 130, 246, 0.5)",
    pink: "rgba(236, 72, 153, 0.5)",
    purple: "rgba(168, 85, 247, 0.5)",
  }[color];
}

function useOverlayLayout(): boolean {
  const [overlay, setOverlay] = useState(() => window.innerWidth < OVERLAY_BREAKPOINT);

  useLayoutEffect(() => {
    const onResize = () => setOverlay(window.innerWidth < OVERLAY_BREAKPOINT);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  return overlay;
}

type SelectionPage = { pageNumber: number; selectedText: string; rects: NormalizedRect[] };

type Caret = { node: Node; offset: number };

/** The .textLayer ancestor of a node, or null if the node is outside any page. */
function textLayerOf(node: Node | null): HTMLElement | null {
  for (let n: Node | null = node; n && n !== document.body; n = n.parentNode) {
    if (n instanceof HTMLElement && n.classList.contains("textLayer")) return n;
  }
  return null;
}

/** The caret in a page's text layer at a screen point, or null if not on text. */
function caretInTextLayer(x: number, y: number): Caret | null {
  const caret = document.caretRangeFromPoint?.(x, y);
  if (!caret || !textLayerOf(caret.startContainer)) return null;
  return { node: caret.startContainer, offset: caret.startOffset };
}

/** Builds an ordered range between two carets (anchor may be after focus). */
function orderedRange(anchor: Caret, focus: Caret): Range {
  const a = document.createRange();
  a.setStart(anchor.node, anchor.offset);
  const f = document.createRange();
  f.setStart(focus.node, focus.offset);
  const range = document.createRange();
  if (a.compareBoundaryPoints(Range.START_TO_START, f) <= 0) {
    range.setStart(anchor.node, anchor.offset);
    range.setEnd(focus.node, focus.offset);
  } else {
    range.setStart(focus.node, focus.offset);
    range.setEnd(anchor.node, anchor.offset);
  }
  return range;
}

/**
 * Clips a range to each text layer it crosses, returning one entry per page with
 * page-space rectangles already trimmed to the text (§9.5).
 */
function collectRangePages(
  range: Range,
  textLayers: React.MutableRefObject<Map<number, HTMLElement>>,
): SelectionPage[] {
  const pages: SelectionPage[] = [];
  for (const [pageNumber, layer] of textLayers.current) {
    if (!range.intersectsNode(layer)) continue;

    const pageRange = range.cloneRange();
    const layerRange = document.createRange();
    layerRange.selectNodeContents(layer);
    if (pageRange.compareBoundaryPoints(Range.START_TO_START, layerRange) < 0) {
      pageRange.setStart(layerRange.startContainer, layerRange.startOffset);
    }
    if (pageRange.compareBoundaryPoints(Range.END_TO_END, layerRange) > 0) {
      pageRange.setEnd(layerRange.endContainer, layerRange.endOffset);
    }

    const rects = selectionToRects(pageRange, layer);
    const pageText = pageRange.toString().trim();
    if (rects.length > 0 && pageText.length > 0) {
      pages.push({ pageNumber, selectedText: pageText, rects });
    }
  }
  return pages.sort((a, b) => a.pageNumber - b.pageNumber);
}

type PendingSelection = { x: number; y: number; pages: SelectionPage[] };

/**
 * A custom drag-to-select for the PDF. Native selection is disabled on the text
 * layer (WKWebView ignores a transparent ::selection), so this tracks the drag
 * with caretRangeFromPoint, draws its own clipped preview, and on release exposes
 * the toolbar + per-page rectangles. The on-screen selection therefore always
 * hugs the text, exactly like the saved highlight.
 */
function useDragSelection(
  textLayers: React.MutableRefObject<Map<number, HTMLElement>>,
  scrollRef: React.RefObject<HTMLDivElement | null>,
  onClick: (x: number, y: number) => void,
) {
  const [current, setCurrent] = useState<PendingSelection | null>(null);
  const [preview, setPreview] = useState<Record<number, NormalizedRect[]>>({});
  const anchor = useRef<Caret | null>(null);
  const frame = useRef(0);

  const clear = useCallback(() => {
    anchor.current = null;
    setPreview({});
    setCurrent(null);
  }, []);

  const onMouseDown = useCallback(
    (event: React.MouseEvent) => {
      if (event.button !== 0) return;
      const caret = caretInTextLayer(event.clientX, event.clientY);
      anchor.current = caret;
      setPreview({});
      setCurrent(null);
    },
    [],
  );

  const onMouseMove = useCallback(
    (event: React.MouseEvent) => {
      if (!anchor.current || (event.buttons & 1) === 0) return;
      const x = event.clientX;
      const y = event.clientY;
      if (frame.current !== 0) return;
      frame.current = window.requestAnimationFrame(() => {
        frame.current = 0;
        const focus = caretInTextLayer(x, y);
        if (!anchor.current || !focus) return;
        const range = orderedRange(anchor.current, focus);
        if (range.collapsed) {
          setPreview({});
          return;
        }
        const next: Record<number, NormalizedRect[]> = {};
        for (const page of collectRangePages(range, textLayers)) next[page.pageNumber] = page.rects;
        setPreview(next);
      });
    },
    [textLayers],
  );

  const onMouseUp = useCallback(
    (event: React.MouseEvent) => {
      const start = anchor.current;
      anchor.current = null;
      if (!start) return;
      const focus = caretInTextLayer(event.clientX, event.clientY);
      if (!focus) {
        setPreview({});
        return;
      }
      const range = orderedRange(start, focus);
      const pages = range.collapsed ? [] : collectRangePages(range, textLayers);
      if (pages.length === 0) {
        setPreview({});
        setCurrent(null);
        // A click with no drag — offer it for word-level highlight deletion.
        onClick(event.clientX, event.clientY);
        return;
      }
      const box = range.getBoundingClientRect();
      const scroll = scrollRef.current?.getBoundingClientRect();
      setCurrent({
        x: Math.max((scroll?.left ?? 0) + 8, box.left),
        y: Math.max(8, box.top - 44),
        pages,
      });
    },
    [textLayers, scrollRef, onClick],
  );

  return { current, preview, clear, onMouseDown, onMouseMove, onMouseUp };
}

function PanelResizer({
  width,
  onResize,
}: {
  width: number;
  onResize: (width: number) => void;
}) {
  const dragging = useRef(false);

  useEffect(() => {
    const onMove = (event: MouseEvent) => {
      if (!dragging.current) return;
      // Without this the drag also sweeps a text selection through the panel,
      // which reads as the resizer "not working" — especially on Windows.
      event.preventDefault();
      const next = window.innerWidth - event.clientX;
      onResize(Math.min(PANEL_MAX, Math.max(PANEL_MIN, next)));
    };
    const onUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    };
  }, [onResize]);

  return (
    // A 6px hit strip with an always-visible center line, so the handle is
    // discoverable; the previous 1px strip was invisible until hovered.
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="패널 너비 조절"
      aria-valuenow={width}
      aria-valuemin={PANEL_MIN}
      aria-valuemax={PANEL_MAX}
      tabIndex={0}
      title="드래그하여 패널 너비 조절 · 더블클릭으로 기본 너비"
      onMouseDown={(event) => {
        event.preventDefault();
        dragging.current = true;
        document.body.style.userSelect = "none";
        document.body.style.cursor = "col-resize";
      }}
      onDoubleClick={() => onResize(PANEL_DEFAULT)}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft") onResize(Math.min(PANEL_MAX, width + 20));
        if (event.key === "ArrowRight") onResize(Math.max(PANEL_MIN, width - 20));
      }}
      className="group/resizer absolute left-0 top-0 z-10 flex h-full w-[6px] -translate-x-[3px] cursor-col-resize items-stretch justify-center focus-visible:outline-none"
    >
      <div className="w-[2px] bg-line transition-colors duration-fast group-hover/resizer:bg-primary group-focus-visible/resizer:bg-primary" />
    </div>
  );
}
