import { Info } from "lucide-react";

import { Badge } from "@/components/ui/Badge";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { Select } from "@/components/ui/Select";
import { errorMessage } from "@/lib/ipc";
import {
  PROVIDERS,
  PROVIDER_LABEL,
  hasKey,
  modelFor,
  type Provider,
  type Settings,
} from "@/lib/types";
import { BlockedJobs } from "./BlockedJobs";
import { ObsidianSection } from "./ObsidianSection";
import { ProviderSection } from "./ProviderSection";
import { useUpdateSettings } from "./queries";

const LANGUAGES = [
  { value: "ko", label: "한국어" },
  { value: "en", label: "English" },
];

const TRANSLATION_ENGINES = [
  { value: "google", label: "무료 (Google 번역)" },
  { value: "llm", label: "선택된 AI" },
];

export function SettingsPage({ settings }: { settings: Settings }) {
  const updateSettings = useUpdateSettings();
  const configured = PROVIDERS.filter((provider) => hasKey(settings, provider));
  const active = settings.activeProvider;

  return (
    // The app shell's <main> is overflow-hidden, so this page owns its scroll.
    <div className="h-full overflow-y-auto">
    <div className="mx-auto flex w-full max-w-[840px] flex-col gap-xl p-xl">
      <header className="flex flex-col gap-sm">
        <Eyebrow>설정</Eyebrow>
        <h1 className="text-subheading text-ink-heading">Bbrain 설정</h1>
      </header>

      <Card className="flex flex-col gap-lg">
        <div className="flex flex-col gap-xs">
          <CardTitle>AI 공급자</CardTitle>
          <CardDescription>
            요약, 번역, 채팅에 사용할 공급자와 모델입니다. 임베딩은 이 기기에서 로컬로
            수행하므로 키가 하나만 있어도 검색과 RAG가 동작합니다.
          </CardDescription>
        </div>

        {configured.length > 1 && (
          <Select
            label="사용 중인 공급자"
            value={active}
            options={configured.map((provider) => ({
              value: provider,
              label: PROVIDER_LABEL[provider],
            }))}
            onChange={(value) =>
              updateSettings.mutate({ activeProvider: value as Provider })
            }
          />
        )}

        {active && (
          <p className="flex items-start gap-2 text-caption text-ink-body">
            <Info aria-hidden className="mt-0.5 h-4 w-4 shrink-0" />
            현재 논문 텍스트는 {PROVIDER_LABEL[active]}의 {modelFor(settings, active) ?? "미선택"} 모델로
            전송됩니다.
          </p>
        )}

        <div className="flex flex-col gap-xl border-t border-line pt-lg">
          {PROVIDERS.map((provider) => (
            <ProviderSection key={provider} provider={provider} settings={settings} />
          ))}
        </div>

        {updateSettings.isError && (
          <p role="alert" className="text-caption text-danger">
            {errorMessage(updateSettings.error)}
          </p>
        )}
      </Card>

      <BlockedJobs />

      <ObsidianSection settings={settings} />

      <Card className="flex flex-col gap-lg">
        <div className="flex flex-col gap-xs">
          <CardTitle>언어</CardTitle>
          <CardDescription>앱 화면 언어와 기본 번역 대상 언어입니다.</CardDescription>
        </div>

        <div className="grid grid-cols-2 gap-lg">
          <Select
            label="앱 언어"
            value={settings.language}
            options={LANGUAGES}
            onChange={(value) => updateSettings.mutate({ language: value })}
          />
          <Select
            label="번역 대상 언어"
            value={settings.translationLanguage}
            options={LANGUAGES}
            onChange={(value) => updateSettings.mutate({ translationLanguage: value })}
          />
          <Select
            label="번역 엔진"
            value={settings.translationEngine}
            options={TRANSLATION_ENGINES}
            onChange={(value) =>
              updateSettings.mutate({ translationEngine: value as "google" | "llm" })
            }
            hint={
              settings.translationEngine === "llm"
                ? "현재 선택된 AI 공급자가 페이지를 번역합니다."
                : "무료 Google 엔진으로 번역합니다. 키가 필요 없습니다."
            }
          />
        </div>

        {settings.translationEngine === "llm" && !active && (
          <p role="alert" className="flex items-start gap-2 text-caption text-danger">
            <Info aria-hidden className="mt-0.5 h-4 w-4 shrink-0" />
            선택된 AI로 번역하려면 위에서 공급자를 먼저 연결하세요.
          </p>
        )}
      </Card>

      <Card className="flex flex-col gap-lg">
        <div className="flex flex-col gap-xs">
          <CardTitle>로컬 임베딩</CardTitle>
          <CardDescription>
            검색과 RAG에 사용하는 임베딩 모델입니다. 모델을 바꾸면 전체 라이브러리를 새
            generation으로 다시 색인합니다.
          </CardDescription>
        </div>

        <dl className="flex flex-wrap gap-md">
          <div className="flex items-center gap-2">
            <dt className="text-caption text-ink-body">모델</dt>
            <dd>
              <Badge>{settings.embeddingModelId}</Badge>
            </dd>
          </div>
          <div className="flex items-center gap-2">
            <dt className="text-caption text-ink-body">차원</dt>
            <dd>
              <Badge>{settings.embeddingDimension}</Badge>
            </dd>
          </div>
          <div className="flex items-center gap-2">
            <dt className="text-caption text-ink-body">인덱스 generation</dt>
            <dd>
              <Badge>{settings.indexGeneration}</Badge>
            </dd>
          </div>
        </dl>
      </Card>
    </div>
    </div>
  );
}
