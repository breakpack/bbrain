import { listen } from "@tauri-apps/api/event";

import { api } from "@/lib/ipc";
import {
  destroyDocument,
  detectDocumentTitle,
  extractPage,
  findRepeatedLines,
  loadDocument,
} from "@/lib/pdf";
import type { ExtractedPage, FrontendJobRequest } from "@/lib/types";

const THUMBNAIL_WIDTH = 240;

/**
 * PDF.js lives in the webview, so the Rust job runner delegates extraction and
 * thumbnails here and waits for the result. The job row is the durable record:
 * if the window closes mid-extraction the job is requeued on the next launch
 * (DEVELOPMENT.md §10.1), so this handler only has to be correct, not durable.
 */
export function startExtractionWorker(): () => void {
  let stopped = false;
  // The runner re-emits a request until answered, so the same job can arrive
  // several times; process each job id only once.
  const active = new Set<string>();

  const unlisten = listen<FrontendJobRequest>("job://frontend-request", async (event) => {
    if (stopped) return;
    const request = event.payload;
    if (active.has(request.jobId)) return;
    active.add(request.jobId);

    try {
      if (request.jobType === "extract") {
        await runExtraction(request);
      } else if (request.jobType === "thumbnail") {
        await runThumbnail(request);
      }
    } catch (error) {
      const reason = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      console.error("[bbrain] frontend job failed", request.jobType, error);
      await api
        .reportExtractionFailure(request.jobId, request.paperId, reason)
        .catch(() => undefined);
    } finally {
      active.delete(request.jobId);
    }
  });

  return () => {
    stopped = true;
    void unlisten.then((off) => off());
  };
}

async function runExtraction(request: FrontendJobRequest): Promise<void> {
  const bytes = await api.readPaperBytes(request.paperId);
  const pdf = await loadDocument(bytes);

  try {
    const pages: ExtractedPage[] = [];
    const pageLines: string[][] = [];

    for (let number = 1; number <= pdf.numPages; number++) {
      const page = await pdf.getPage(number);
      const extraction = await extractPage(page);
      pageLines.push(extraction.lines);
      pages.push({
        pageNumber: extraction.pageNumber,
        width: extraction.width,
        height: extraction.height,
        rotation: extraction.rotation,
        text: extraction.text,
        sentences: extraction.sentences,
      });
      page.cleanup();
    }

    // Running headers and footers repeat on every page; they add noise to
    // analysis and retrieval, so drop them from the stored page text.
    const repeated = findRepeatedLines(pageLines);
    if (repeated.size > 0) {
      for (let i = 0; i < pages.length; i++) {
        pages[i].text = pageLines[i]
          .filter((line) => !repeated.has(line.trim().replace(/\d+/g, "#").toLowerCase()))
          .join("\n");
      }
    }

    // The paper names itself: its in-PDF title replaces the filename-derived
    // placeholder. Detection failure is not an extraction failure.
    const detectedTitle = await detectDocumentTitle(pdf).catch(() => null);

    await api.submitExtraction({
      jobId: request.jobId,
      paperId: request.paperId,
      pages,
      detectedTitle: detectedTitle ?? undefined,
    });
  } finally {
    await destroyDocument(pdf);
  }
}

async function runThumbnail(request: FrontendJobRequest): Promise<void> {
  const bytes = await api.readPaperBytes(request.paperId);
  const pdf = await loadDocument(bytes);

  try {
    const page = await pdf.getPage(1);
    const base = page.getViewport({ scale: 1 });
    const viewport = page.getViewport({ scale: THUMBNAIL_WIDTH / base.width });

    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.floor(viewport.width));
    canvas.height = Math.max(1, Math.floor(viewport.height));
    await page.render({ canvas, viewport }).promise;

    // WKWebView cannot encode WebP and silently returns PNG instead, so ask for
    // PNG on both platforms and keep the stored bytes identical.
    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/png"),
    );
    if (!blob) throw new Error("thumbnail encoding produced no bytes");

    await api.submitThumbnail({
      jobId: request.jobId,
      paperId: request.paperId,
      png: Array.from(new Uint8Array(await blob.arrayBuffer())),
    });
  } finally {
    await destroyDocument(pdf);
  }
}
