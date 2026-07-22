import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { api } from "@/lib/ipc";
import type { LibraryQuery, PaperPatch } from "@/lib/types";

export const libraryKeys = {
  papers: (query: LibraryQuery) => ["papers", query] as const,
  paper: (paperId: string) => ["paper", paperId] as const,
  groups: ["groups"] as const,
  tags: ["tags"] as const,
  blockedJobs: ["blocked-jobs"] as const,
};

/**
 * Backend events announce *that* something changed; the DB stays the source of
 * truth, so every listener refetches rather than patching cached rows
 * (DEVELOPMENT.md §6.1).
 */
export function useLibraryEvents(): void {
  const client = useQueryClient();

  useEffect(() => {
    const offs = [
      listen("library://changed", () => {
        void client.invalidateQueries({ queryKey: ["papers"] });
        void client.invalidateQueries({ queryKey: libraryKeys.groups });
        void client.invalidateQueries({ queryKey: libraryKeys.tags });
      }),
      listen<string>("paper://changed", (event) => {
        void client.invalidateQueries({ queryKey: libraryKeys.paper(event.payload) });
        void client.invalidateQueries({ queryKey: ["papers"] });
      }),
      listen("job://progress", () => {
        void client.invalidateQueries({ queryKey: ["papers"] });
        void client.invalidateQueries({ queryKey: libraryKeys.blockedJobs });
      }),
    ];

    return () => {
      for (const off of offs) void off.then((fn) => fn());
    };
  }, [client]);
}

export function usePapers(query: LibraryQuery) {
  return useQuery({
    queryKey: libraryKeys.papers(query),
    queryFn: () => api.listPapers(query),
  });
}

export function useGroups() {
  return useQuery({ queryKey: libraryKeys.groups, queryFn: api.listGroups });
}

export function useTags() {
  return useQuery({ queryKey: libraryKeys.tags, queryFn: api.listTags });
}

export function useBlockedJobs() {
  return useQuery({ queryKey: libraryKeys.blockedJobs, queryFn: api.listBlockedJobs });
}

export function useImportPapers() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ paths, groupId }: { paths: string[]; groupId?: string }) =>
      api.importPapers(paths, groupId),
    onSuccess: () => client.invalidateQueries({ queryKey: ["papers"] }),
  });
}

export function useUpdatePaper() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ paperId, patch }: { paperId: string; patch: PaperPatch }) =>
      api.updatePaper(paperId, patch),
    onSuccess: (paper) => {
      client.setQueryData(libraryKeys.paper(paper.id), paper);
      void client.invalidateQueries({ queryKey: ["papers"] });
    },
  });
}

export function useDeletePaper() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ paperId, deleteFile }: { paperId: string; deleteFile: boolean }) =>
      api.deletePaper(paperId, deleteFile),
    onSuccess: () => client.invalidateQueries({ queryKey: ["papers"] }),
  });
}

export function useCreateGroup() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => api.createGroup(name),
    onSuccess: () => client.invalidateQueries({ queryKey: libraryKeys.groups }),
  });
}

export function useDeleteGroup() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (groupId: string) => api.deleteGroup(groupId),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: libraryKeys.groups });
      void client.invalidateQueries({ queryKey: ["papers"] });
    },
  });
}

export function useRetryJob() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (jobId: string) => api.retryJob(jobId),
    onSuccess: () => client.invalidateQueries({ queryKey: libraryKeys.blockedJobs }),
  });
}
