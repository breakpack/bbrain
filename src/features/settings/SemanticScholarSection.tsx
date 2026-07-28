import { CheckCircle2, KeyRound, Trash2 } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { errorMessage } from "@/lib/ipc";
import type { Settings } from "@/lib/types";
import {
  useConfigureSemanticScholar,
  useRemoveSemanticScholar,
} from "./queries";

export function SemanticScholarSection({ settings }: { settings: Settings }) {
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const configure = useConfigureSemanticScholar();
  const remove = useRemoveSemanticScholar();

  async function connect() {
    setError(null);
    try {
      await configure.mutateAsync(apiKey.trim());
      setApiKey("");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function disconnect() {
    setError(null);
    try {
      await remove.mutateAsync();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <Card className="flex flex-col gap-lg">
      <div className="flex items-start justify-between gap-md">
        <div className="flex flex-col gap-xs">
          <CardTitle>논문 검색</CardTitle>
          <CardDescription>
            Semantic Scholar 공유 검색도 사용할 수 있지만, 전용 API 키를 연결하면 요청
            제한의 영향을 훨씬 적게 받습니다.{" "}
            <a
              href="https://www.semanticscholar.org/product/api#api-key-form"
              target="_blank"
              rel="noreferrer"
              className="text-primary hover:underline"
            >
              API 키 발급
            </a>
          </CardDescription>
        </div>
        {settings.hasSemanticScholarKey ? (
          <Badge tone="primary" icon={<CheckCircle2 aria-hidden className="h-4 w-4" />}>
            연결됨
          </Badge>
        ) : (
          <Badge icon={<KeyRound aria-hidden className="h-4 w-4" />}>공유 연결</Badge>
        )}
      </div>

      {settings.hasSemanticScholarKey ? (
        <div>
          <Button
            variant="ghost"
            size="sm"
            onClick={disconnect}
            loading={remove.isPending}
          >
            <Trash2 aria-hidden className="h-[18px] w-[18px]" />
            키 삭제
          </Button>
        </div>
      ) : (
        <div className="flex items-end gap-md">
          <Input
            className="flex-1"
            type="password"
            label="Semantic Scholar API 키 (선택)"
            autoComplete="off"
            spellCheck={false}
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
            hint="키는 이 기기의 시스템 키체인에만 저장됩니다."
          />
          <Button
            className="mb-[26px]"
            onClick={connect}
            loading={configure.isPending}
            disabled={apiKey.trim().length === 0}
          >
            연결
          </Button>
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
