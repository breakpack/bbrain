import { useState } from "react";
import { AlertCircle, CheckCircle2, KeyRound, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { Select } from "@/components/ui/Select";
import { errorMessage } from "@/lib/ipc";
import {
  DEFAULT_MODEL,
  PROVIDER_LABEL,
  hasKey,
  modelFor,
  type Provider,
  type Settings,
} from "@/lib/types";
import {
  useConfigureProvider,
  useProviderModels,
  useRemoveProvider,
  useUpdateSettings,
} from "./queries";

/**
 * Connect / re-key / model-select for one provider. The API key is typed here,
 * sent once, and stored in the OS credential store — it is never read back.
 */
export function ProviderSection({
  provider,
  settings,
  onConnected,
}: {
  provider: Provider;
  settings: Settings;
  onConnected?: () => void;
}) {
  const connected = hasKey(settings, provider);
  const selectedModel = modelFor(settings, provider);

  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const configure = useConfigureProvider();
  const remove = useRemoveProvider();
  const updateSettings = useUpdateSettings();
  const models = useProviderModels(provider, connected);

  const label = PROVIDER_LABEL[provider];
  const modelMissing =
    connected &&
    models.isSuccess &&
    selectedModel !== null &&
    !models.data.some((model) => model.id === selectedModel);

  async function connect() {
    setError(null);
    try {
      await configure.mutateAsync({ provider, apiKey: apiKey.trim() });
      setApiKey("");
      onConnected?.();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function selectModel(model: string) {
    setError(null);
    try {
      await updateSettings.mutateAsync(
        provider === "openai" ? { openaiModel: model } : { anthropicModel: model },
      );
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function disconnect() {
    setError(null);
    try {
      await remove.mutateAsync(provider);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <section className="flex flex-col gap-md" aria-labelledby={`provider-${provider}`}>
      <div className="flex items-center justify-between gap-md">
        <h3 id={`provider-${provider}`} className="text-body font-bold text-ink-heading">
          {label}
        </h3>
        {connected ? (
          <Badge tone="primary" icon={<CheckCircle2 aria-hidden className="h-4 w-4" />}>
            연결됨
          </Badge>
        ) : (
          <Badge icon={<KeyRound aria-hidden className="h-4 w-4" />}>연결 안 됨</Badge>
        )}
      </div>

      {!connected && (
        <div className="flex items-end gap-md">
          <Input
            className="flex-1"
            type="password"
            label="API 키"
            autoComplete="off"
            spellCheck={false}
            placeholder={provider === "openai" ? "sk-..." : "sk-ant-..."}
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
            hint={`${label} 키는 이 기기의 시스템 키체인에만 저장됩니다.`}
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

      {connected && (
        <>
          <Select
            label="모델"
            value={selectedModel}
            placeholder={models.isPending ? "모델을 불러오는 중" : "모델을 선택하세요"}
            disabled={models.isPending || models.isError}
            options={(models.data ?? []).map((model) => ({
              value: model.id,
              label: model.displayName,
            }))}
            onChange={selectModel}
            hint={`기본 프리셋: ${DEFAULT_MODEL[provider]}`}
          />

          {models.isError && (
            <p role="alert" className="text-caption text-danger">
              {errorMessage(models.error)} 키를 다시 확인한 뒤 재시도하세요.
            </p>
          )}

          {modelMissing && (
            <p role="alert" className="flex items-center gap-2 text-caption text-danger">
              <AlertCircle aria-hidden className="h-4 w-4 shrink-0" />
              저장된 모델을 더 이상 사용할 수 없습니다. 다른 모델을 선택하세요.
            </p>
          )}

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
        </>
      )}

      {error && (
        <p role="alert" className="text-caption text-danger">
          {error}
        </p>
      )}
    </section>
  );
}
