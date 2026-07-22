import { TextLayer, type PDFPageProxy, type RenderTask } from "pdfjs-dist";
import { useEffect, useRef, useState } from "react";

import type { Highlight, NormalizedRect, Sentence } from "@/lib/types";
import { cn } from "@/lib/cn";

const HIGHLIGHT_FILL: Record<string, string> = {
  yellow: "rgba(250, 204, 21, 0.35)",
  green: "rgba(0, 196, 115, 0.28)",
  blue: "rgba(59, 130, 246, 0.28)",
  pink: "rgba(236, 72, 153, 0.28)",
  purple: "rgba(168, 85, 247, 0.28)",
};

/** Highlight shown while hovering a translated sentence (DEVELOPMENT.md §9.4). */
const SOURCE_FILL = "rgba(0, 196, 115, 0.32)";
/** The drag-selection preview, drawn in place of the native selection. */
const SELECTION_FILL = "rgba(59, 130, 246, 0.35)";

export function PdfPage({
  page,
  scale,
  highlights,
  sourceRects,
  pulse,
  sentences,
  selectionRects,
  onHoverSource,
  onTextLayerReady,
}: {
  page: PDFPageProxy;
  scale: number;
  highlights: Highlight[];
  sourceRects: NormalizedRect[];
  pulse: boolean;
  /** Source sentences for this page, used to hit-test what the cursor is over.
   * Only the page being read supplies them; others pass an empty list. */
  sentences: Sentence[];
  /** Clipped rectangles of the in-progress drag selection on this page — drawn in
   * place of the browser's native selection, which bleeds into the margin. */
  selectionRects: NormalizedRect[];
  /** Reports the sentence under the cursor (or null), so the translation panel
   * can highlight the matching unit — the reverse of hovering the translation. */
  onHoverSource: (sentenceId: string | null) => void;
  onTextLayerReady: (pageNumber: number, layer: HTMLElement | null) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const textLayerRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  // Only report when the sentence under the cursor changes, so a mousemove does
  // not set state on every pixel.
  const lastHovered = useRef<string | null>(null);
  // Normalized glyph rectangles, captured once the text layer renders. Used to
  // clip highlights to the actual text so old highlights saved with margin
  // overshoot (before the extraction fix) also render tight to the words.
  const [glyphs, setGlyphs] = useState<NormalizedRect[]>([]);

  const viewport = page.getViewport({ scale });

  const report = (sentenceId: string | null) => {
    if (lastHovered.current === sentenceId) return;
    lastHovered.current = sentenceId;
    onHoverSource(sentenceId);
  };

  // Hit-test the cursor against sentence rectangles. A mousemove listener on the
  // container works even though the text layer sits on top (events bubble up),
  // so it does not disturb text selection.
  const onMouseMove = (event: React.MouseEvent) => {
    if (sentences.length === 0) return;
    const container = containerRef.current;
    if (!container) return;

    const box = container.getBoundingClientRect();
    const nx = (event.clientX - box.left) / box.width;
    const ny = (event.clientY - box.top) / box.height;

    const hit = sentences.find((sentence) =>
      sentence.rects.some(
        (rect) =>
          nx >= rect.x &&
          nx <= rect.x + rect.width &&
          ny >= rect.y &&
          ny <= rect.y + rect.height,
      ),
    );
    report(hit?.id ?? null);
  };

  const onMouseLeave = () => report(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    const textLayerDiv = textLayerRef.current;
    if (!canvas || !container || !textLayerDiv) return;

    let renderTask: RenderTask | null = null;
    let textLayer: TextLayer | null = null;
    let cancelled = false;

    const render = async () => {
      const viewport = page.getViewport({ scale });
      const dpr = window.devicePixelRatio || 1;

      canvas.width = Math.floor(viewport.width * dpr);
      canvas.height = Math.floor(viewport.height * dpr);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;

      // The text layer positions its spans from these variables; v6 reads
      // --total-scale-factor, not just --scale-factor.
      container.style.setProperty("--scale-factor", String(scale));
      container.style.setProperty("--user-unit", "1");
      container.style.setProperty("--total-scale-factor", String(scale));

      renderTask = page.render({
        canvas,
        viewport,
        transform: dpr === 1 ? undefined : [dpr, 0, 0, dpr, 0, 0],
      });

      try {
        await renderTask.promise;
      } catch (error) {
        // A superseded render (zoom changed mid-flight) is expected, not a fault.
        if ((error as Error)?.name === "RenderingCancelledException") return;
        throw error;
      }
      if (cancelled) return;

      textLayerDiv.replaceChildren();
      textLayer = new TextLayer({
        textContentSource: await page.getTextContent(),
        container: textLayerDiv,
        viewport,
      });
      await textLayer.render();
      if (cancelled) return;

      // Capture glyph rectangles (normalized to the page box) so highlights can
      // be clipped to the text. Scale-independent, so computed once per render.
      const box = container.getBoundingClientRect();
      const captured: NormalizedRect[] = [];
      if (box.width > 0 && box.height > 0) {
        for (const span of textLayerDiv.querySelectorAll("span")) {
          for (const g of span.getClientRects()) {
            if (g.width > 0 && g.height > 0) {
              captured.push({
                x: (g.left - box.left) / box.width,
                y: (g.top - box.top) / box.height,
                width: g.width / box.width,
                height: g.height / box.height,
              });
            }
          }
        }
      }
      setGlyphs(captured);

      onTextLayerReady(page.pageNumber, textLayerDiv);
    };

    void render().catch((error) => {
      console.error("[bbrain] page render failed", page.pageNumber, error);
    });

    return () => {
      cancelled = true;
      renderTask?.cancel();
      textLayer?.cancel();
      onTextLayerReady(page.pageNumber, null);
    };
  }, [page, scale, onTextLayerReady]);

  return (
    <div
      ref={containerRef}
      data-page-number={page.pageNumber}
      className="relative mx-auto shadow-card"
      style={{ width: viewport.width, height: viewport.height }}
      onMouseMove={onMouseMove}
      onMouseLeave={onMouseLeave}
    >
      <canvas ref={canvasRef} className="block" />

      {/* Saved highlights sit under the text layer so selection still works.
          Each rectangle is clipped to the glyphs it covers, so a highlight never
          bleeds into the margin — including ones saved before the fix. */}
      <div className="pointer-events-none absolute inset-0" aria-hidden>
        {highlights.flatMap((highlight) =>
          clipRectsToGlyphs(highlight.rects, glyphs).map((rect, index) => (
            <span
              key={`${highlight.id}-${index}`}
              data-highlight-id={highlight.id}
              className="absolute rounded-[2px]"
              style={{
                left: `${rect.x * 100}%`,
                top: `${rect.y * 100}%`,
                width: `${rect.width * 100}%`,
                height: `${rect.height * 100}%`,
                background: HIGHLIGHT_FILL[highlight.color] ?? HIGHLIGHT_FILL.yellow,
              }}
            />
          )),
        )}

        {sourceRects.map((rect, index) => (
          <span
            key={`source-${index}`}
            className={cn(
              "absolute rounded-[2px] transition-opacity duration-fast",
              pulse && "animate-pulse",
            )}
            style={{
              left: `${rect.x * 100}%`,
              top: `${rect.y * 100}%`,
              width: `${rect.width * 100}%`,
              height: `${rect.height * 100}%`,
              background: SOURCE_FILL,
            }}
          />
        ))}

        {selectionRects.map((rect, index) => (
          <span
            key={`selection-${index}`}
            className="absolute rounded-[2px]"
            style={{
              left: `${rect.x * 100}%`,
              top: `${rect.y * 100}%`,
              width: `${rect.width * 100}%`,
              height: `${rect.height * 100}%`,
              background: SELECTION_FILL,
            }}
          />
        ))}
      </div>

      <div ref={textLayerRef} className="textLayer" />
    </div>
  );
}

/**
 * Clips highlight rectangles to the glyphs beneath them, so a bar never extends
 * past the text into the margin. Falls back to the raw rectangles until the
 * glyphs are captured (the text layer has rendered).
 */
function clipRectsToGlyphs(rects: NormalizedRect[], glyphs: NormalizedRect[]): NormalizedRect[] {
  if (glyphs.length === 0) return rects;

  const clipped: NormalizedRect[] = [];
  for (const rect of rects) {
    let left = Infinity;
    let right = -Infinity;
    for (const glyph of glyphs) {
      const overlap =
        Math.min(rect.y + rect.height, glyph.y + glyph.height) - Math.max(rect.y, glyph.y);
      if (overlap <= Math.min(rect.height, glyph.height) * 0.4) continue;
      const l = Math.max(rect.x, glyph.x);
      const r = Math.min(rect.x + rect.width, glyph.x + glyph.width);
      if (r - l <= 0) continue;
      left = Math.min(left, l);
      right = Math.max(right, r);
    }
    if (right > left) clipped.push({ x: left, y: rect.y, width: right - left, height: rect.height });
  }

  // Merge slivers left between adjacent text spans (a space-sized gap), but keep
  // larger gaps — those are words the reader deleted, which must stay split.
  const MERGE_GAP = 0.014;
  const out: NormalizedRect[] = [];
  for (const rect of clipped.sort((a, b) => a.y - b.y || a.x - b.x)) {
    const prev = out[out.length - 1];
    const sameLine =
      prev &&
      Math.min(prev.y + prev.height, rect.y + rect.height) - Math.max(prev.y, rect.y) >
        Math.min(prev.height, rect.height) * 0.5;
    if (sameLine && rect.x - (prev.x + prev.width) <= MERGE_GAP) {
      const right = Math.max(prev.x + prev.width, rect.x + rect.width);
      prev.width = right - prev.x;
      prev.height = Math.max(prev.height, rect.height);
    } else {
      out.push({ ...rect });
    }
  }
  return out;
}
