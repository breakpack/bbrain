import type { PDFPageProxy, RenderTask } from "pdfjs-dist";
import { useEffect, useRef, useState } from "react";

/**
 * A small rendered preview of one page for the sidebar. Rendering is deferred
 * until the thumbnail scrolls near the sidebar viewport, so a 200-page paper
 * does not rasterize every page up front.
 */
export function PdfThumbnail({ page, width }: { page: PDFPageProxy; width: number }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [rendered, setRendered] = useState(false);

  // Reserve the page's aspect ratio before it renders so the list does not jump.
  // The canvas renders at `width` pixels but displays responsively (w-full), so
  // the rail can be resized without re-rasterizing every page.
  const base = page.getViewport({ scale: 1 });

  useEffect(() => {
    const wrapper = wrapperRef.current;
    if (!wrapper) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setVisible(true);
      },
      // Start rendering a little before the thumbnail is on screen.
      { root: null, rootMargin: "200px" },
    );
    observer.observe(wrapper);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!visible || !canvas || rendered) return;

    let task: RenderTask | null = null;
    let cancelled = false;

    const render = async () => {
      const dpr = window.devicePixelRatio || 1;
      const viewport = page.getViewport({ scale: width / base.width });
      canvas.width = Math.floor(viewport.width * dpr);
      canvas.height = Math.floor(viewport.height * dpr);

      task = page.render({
        canvas,
        viewport,
        transform: dpr === 1 ? undefined : [dpr, 0, 0, dpr, 0, 0],
      });

      try {
        await task.promise;
        if (!cancelled) setRendered(true);
      } catch (error) {
        if ((error as Error)?.name !== "RenderingCancelledException") {
          console.error("[bbrain] thumbnail render failed", page.pageNumber, error);
        }
      }
    };

    void render();
    return () => {
      cancelled = true;
      task?.cancel();
    };
  }, [visible, rendered, page, width, base.width]);

  return (
    <div
      ref={wrapperRef}
      className="w-full overflow-hidden rounded-sm border border-line bg-canvas"
      style={{ aspectRatio: `${base.width} / ${base.height}` }}
    >
      <canvas ref={canvasRef} className="block w-full" />
    </div>
  );
}
