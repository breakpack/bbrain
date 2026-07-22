import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import type { PDFDocumentProxy } from "pdfjs-dist";

import { api } from "@/lib/ipc";
import { destroyDocument, loadDocument } from "@/lib/pdf";
import type { HighlightColor, NormalizedRect, SaveHighlightInput } from "@/lib/types";

export function useViewerDocument(paperId: string) {
  return useQuery({
    queryKey: ["viewer", paperId],
    queryFn: () => api.getViewerDocument(paperId),
  });
}

/** Owns the PDF.js document handle and destroys it when the viewer closes. */
export function usePdfDocument(paperId: string) {
  const [pdf, setPdf] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let current: PDFDocumentProxy | null = null;
    let cancelled = false;

    void api
      .readPaperBytes(paperId)
      .then(loadDocument)
      .then((document) => {
        if (cancelled) {
          void destroyDocument(document);
          return;
        }
        current = document;
        setPdf(document);
      })
      .catch(setError);

    return () => {
      cancelled = true;
      setPdf(null);
      if (current) void destroyDocument(current);
    };
  }, [paperId]);

  return { pdf, error };
}

export function useHighlights(paperId: string) {
  return useQuery({
    queryKey: ["highlights", paperId],
    queryFn: () => api.listHighlights(paperId),
  });
}

export function useSaveHighlight(paperId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: SaveHighlightInput) => api.saveHighlight(input),
    onSuccess: () => client.invalidateQueries({ queryKey: ["highlights", paperId] }),
  });
}

export function useUpdateHighlight(paperId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ id, color }: { id: string; color: HighlightColor }) =>
      api.updateHighlight(id, { color }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["highlights", paperId] }),
  });
}

export function useDeleteHighlight(paperId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.deleteHighlight(id),
    onSuccess: () => client.invalidateQueries({ queryKey: ["highlights", paperId] }),
  });
}

/** Word-level deletion: replace a highlight's rectangles and text with what
 * remains after a word is removed (the backend deletes the row if empty). */
export function useTrimHighlight(paperId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      rects,
      selectedText,
    }: {
      id: string;
      rects: NormalizedRect[];
      selectedText: string;
    }) => api.updateHighlight(id, { rects, selectedText }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["highlights", paperId] }),
  });
}
