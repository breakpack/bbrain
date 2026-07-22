import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, FolderOpen, RefreshCw } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { api, errorMessage } from "@/lib/ipc";
import type { Settings } from "@/lib/types";

export function ObsidianSection({ settings }: { settings: Settings }) {
  const client = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const records = useQuery({
    queryKey: ["sync-records"],
    queryFn: api.listSyncRecords,
    enabled: Boolean(settings.obsidianVaultPath),
  });

  const configure = useMutation({
    mutationFn: (vaultPath: string) => api.configureObsidian(vaultPath),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["settings"] });
      void client.invalidateQueries({ queryKey: ["sync-records"] });
    },
  });

  const sync = useMutation({
    mutationFn: () => api.syncObsidian(),
    onSuccess: () => client.invalidateQueries({ queryKey: ["sync-records"] }),
  });

  const pickVault = async () => {
    setError(null);
    // The native picker is the only way the app gains access to a folder — the
    // webview never holds filesystem scope (DEVELOPMENT.md §16.3).
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;

    try {
      await configure.mutateAsync(selected);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  const conflicts = (records.data ?? []).filter((record) => record.status === "conflict");

  return (
    <Card className="flex flex-col gap-lg">
      <div className="flex flex-col gap-xs">
        <CardTitle>Obsidian 연동</CardTitle>
        <CardDescription>
          분석 결과를 Vault의 Markdown 노트로 내보내고, 노트에서 수정한 태그·그룹·메모를 다시
          읽어옵니다. Bbrain은 Vault의 <code>Bbrain</code> 폴더만 사용합니다.
        </CardDescription>
      </div>

      {settings.obsidianVaultPath ? (
        <div className="flex items-center justify-between gap-md">
          <p className="truncate text-caption text-ink">{settings.obsidianVaultPath}</p>
          <div className="flex shrink-0 gap-sm">
            <Button variant="ghost" size="sm" onClick={() => sync.mutate()} loading={sync.isPending}>
              <RefreshCw aria-hidden className="h-[18px] w-[18px]" />
              지금 동기화
            </Button>
            <Button variant="outline" size="sm" onClick={pickVault} loading={configure.isPending}>
              Vault 변경
            </Button>
          </div>
        </div>
      ) : (
        <div>
          <Button variant="outline" onClick={pickVault} loading={configure.isPending}>
            <FolderOpen aria-hidden className="h-[18px] w-[18px]" />
            Vault 폴더 선택
          </Button>
        </div>
      )}

      {conflicts.length > 0 && (
        <div className="flex flex-col gap-sm rounded-control border border-danger/30 bg-canvas-soft p-md">
          <p className="flex items-center gap-2 text-caption text-danger">
            <AlertTriangle aria-hidden className="h-4 w-4 shrink-0" />
            {conflicts.length}개 노트의 Bbrain 관리 영역 표시가 손상되어 자동 동기화를
            중지했습니다.
          </p>
          <ul className="flex flex-col gap-1">
            {conflicts.map((record) => (
              <li key={record.paperId} className="text-caption text-ink-body">
                {record.paperTitle} — 노트에서{" "}
                <code>{"<!-- bbrain:managed:start -->"}</code>와{" "}
                <code>{"<!-- bbrain:managed:end -->"}</code>가 모두 있는지 확인한 뒤 다시
                동기화하세요. 그 전까지 사용자 내용은 그대로 보존됩니다.
              </li>
            ))}
          </ul>
        </div>
      )}

      {settings.obsidianVaultPath && (records.data?.length ?? 0) > 0 && (
        <div className="flex items-center gap-md">
          <Badge tone="primary">
            {(records.data ?? []).filter((record) => record.status === "synced").length}개 동기화됨
          </Badge>
        </div>
      )}

      {error && (
        <p role="alert" className="text-caption text-danger">
          {error}
        </p>
      )}
    </Card>
  );
}
