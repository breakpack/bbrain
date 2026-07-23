import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Cable, FolderOpen, RefreshCw } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { api, errorMessage } from "@/lib/ipc";
import type { ObsidianRestHealth, Settings } from "@/lib/types";

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

      {settings.obsidianVaultPath && <RestSection settings={settings} />}

      {error && (
        <p role="alert" className="text-caption text-danger">
          {error}
        </p>
      )}
    </Card>
  );
}

const HEALTH_LABEL: Record<ObsidianRestHealth, { label: string; tone: "primary" | "neutral" }> = {
  connected: { label: "연결됨", tone: "primary" },
  unauthorized: { label: "API 키가 거부됨", tone: "neutral" },
  unreachable: { label: "응답 없음 — Obsidian이 실행 중인지 확인", tone: "neutral" },
};

/**
 * Obsidian Local REST API hookup (the channel MCP servers use). When connected,
 * notes are written through a running Obsidian and appear instantly; otherwise
 * Bbrain silently falls back to writing files.
 */
function RestSection({ settings }: { settings: Settings }) {
  const client = useQueryClient();
  const [url, setUrl] = useState(settings.obsidianRestUrl ?? "https://127.0.0.1:27124");
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const status = useQuery({
    queryKey: ["obsidian-rest-status"],
    queryFn: api.obsidianRestStatus,
    enabled: Boolean(settings.obsidianRestUrl),
  });

  const configure = useMutation({
    mutationFn: ({ url, apiKey }: { url: string; apiKey?: string }) =>
      api.configureObsidianRest(url, apiKey),
    onSuccess: (health) => {
      setApiKey("");
      client.setQueryData(["obsidian-rest-status"], health);
      void client.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (cause) => setError(errorMessage(cause)),
  });

  const connect = () => {
    setError(null);
    if (url.trim().length === 0) return;
    // Omit the key when the field is empty and one is already stored — that
    // re-tests the connection instead of overwriting the stored key.
    configure.mutate({
      url: url.trim(),
      apiKey: apiKey.trim().length > 0 ? apiKey.trim() : undefined,
    });
  };

  const disconnect = () => {
    setError(null);
    configure.mutate({ url: "" });
  };

  const health = status.data ?? null;
  const connected = Boolean(settings.obsidianRestUrl);

  return (
    <div className="flex flex-col gap-md border-t border-line pt-lg">
      <div className="flex items-center justify-between gap-md">
        <div className="flex flex-col gap-xs">
          <h3 className="flex items-center gap-2 text-caption font-medium text-ink-heading">
            <Cable aria-hidden className="h-4 w-4" />
            Local REST API 연동 (MCP 채널)
          </h3>
          <p className="text-caption text-ink-body">
            Obsidian의 Local REST API 플러그인에 연결하면 노트가 실행 중인 Obsidian에 즉시
            반영됩니다. 연결이 없으면 파일로 직접 기록합니다.
          </p>
        </div>
        {connected && health && (
          <Badge tone={HEALTH_LABEL[health].tone}>{HEALTH_LABEL[health].label}</Badge>
        )}
      </div>

      <div className="flex items-end gap-sm">
        <div className="flex-1">
          <Input
            label="엔드포인트"
            placeholder="https://127.0.0.1:27124"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
        </div>
        <div className="flex-1">
          <Input
            label="API 키"
            type="password"
            placeholder={settings.hasObsidianRestKey ? "저장됨 — 바꿀 때만 입력" : "플러그인 설정에서 복사"}
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
          />
        </div>
        <Button
          aria-label="Local REST API 연결"
          variant="outline"
          size="sm"
          onClick={connect}
          loading={configure.isPending}
          disabled={url.trim().length === 0 || (!settings.hasObsidianRestKey && apiKey.trim().length === 0)}
        >
          연결
        </Button>
        {connected && (
          <Button variant="ghost" size="sm" onClick={disconnect} disabled={configure.isPending}>
            해제
          </Button>
        )}
      </div>

      {error && (
        <p role="alert" className="text-caption text-danger">
          {error}
        </p>
      )}
    </div>
  );
}
