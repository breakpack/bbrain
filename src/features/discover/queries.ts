import { useMutation, useQueryClient } from "@tanstack/react-query";

import { api } from "@/lib/ipc";
import type { DiscoverQuery } from "@/lib/types";

/**
 * External scholarly search is imperative (a submit, then "더 보기"), not a
 * standing query, so it is a mutation the page drives and whose accumulated
 * results the page holds.
 */
export function useSearchPapers() {
  return useMutation({
    mutationFn: (query: DiscoverQuery) => api.searchPapers(query),
  });
}

/** Downloads and imports a found paper, then refreshes the library list. */
export function useImportDiscoveredPaper() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ paperId, groupId }: { paperId: string; groupId?: string }) =>
      api.importDiscoveredPaper(paperId, groupId),
    onSuccess: () => client.invalidateQueries({ queryKey: ["papers"] }),
  });
}
