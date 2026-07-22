import { useState } from "react";
import { ArrowRight, Lock, ShieldCheck } from "lucide-react";

import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { ProviderSection } from "@/features/settings/ProviderSection";
import { useUpdateSettings } from "@/features/settings/queries";
import { errorMessage } from "@/lib/ipc";
import { PROVIDERS, hasKey, type Settings } from "@/lib/types";

type Step = "notice" | "provider";

/**
 * First run: the network notice must be acknowledged before any provider key is
 * accepted (DEVELOPMENT.md §16.2). AI setup is skippable — local features work
 * without a key and AI jobs simply wait.
 */
export function Onboarding({ settings }: { settings: Settings }) {
  const [step, setStep] = useState<Step>(
    settings.networkNoticeAcceptedAt ? "provider" : "notice",
  );
  const [error, setError] = useState<string | null>(null);
  const updateSettings = useUpdateSettings();

  const anyKey = PROVIDERS.some((provider) => hasKey(settings, provider));

  async function acceptNotice() {
    setError(null);
    try {
      await updateSettings.mutateAsync({ networkNoticeAccepted: true });
      setStep("provider");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function finish() {
    setError(null);
    try {
      await updateSettings.mutateAsync({ onboardingCompleted: true });
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <main className="flex min-h-full items-center justify-center bg-canvas-soft p-section">
      <Card className="w-full max-w-[640px] p-xxl">
        <div className="flex flex-col gap-lg">
          <div className="flex flex-col gap-sm">
            <Eyebrow>BBRAIN 시작하기</Eyebrow>
            <CardTitle>
              {step === "notice" ? "데이터가 어디에 저장되는지 먼저 알려드립니다" : "AI 공급자를 연결하세요"}
            </CardTitle>
            <CardDescription>
              {step === "notice"
                ? "논문, 하이라이트, 임베딩, 채팅 기록은 이 기기에만 저장됩니다."
                : "요약, 번역, 채팅에 사용할 공급자와 모델을 선택합니다. 나중에 설정에서 바꿀 수 있습니다."}
            </CardDescription>
          </div>

          {step === "notice" ? (
            <>
              <ul className="flex flex-col gap-md">
                <li className="flex gap-md">
                  <Lock aria-hidden className="mt-0.5 h-5 w-5 shrink-0 text-primary" />
                  <p className="text-caption text-ink">
                    원본 PDF, 임베딩과 검색 인덱스, 하이라이트, 채팅 기록 전체본은 이 기기를
                    벗어나지 않습니다.
                  </p>
                </li>
                <li className="flex gap-md">
                  <ShieldCheck aria-hidden className="mt-0.5 h-5 w-5 shrink-0 text-primary" />
                  <p className="text-caption text-ink">
                    요약·번역·채팅을 요청하면 해당 작업에 필요한 논문 텍스트와 대화 맥락만
                    선택한 공급자(OpenAI, Anthropic 또는 DeepSeek)로 전송됩니다. API 키는 시스템
                    키체인에 저장하며 로그에 남기지 않습니다.
                  </p>
                </li>
              </ul>

              <div className="flex justify-end">
                <Button onClick={acceptNotice} loading={updateSettings.isPending}>
                  이해했습니다
                  <ArrowRight aria-hidden className="h-[18px] w-[18px]" />
                </Button>
              </div>
            </>
          ) : (
            <>
              <div className="flex flex-col gap-xl">
                {PROVIDERS.map((provider) => (
                  <ProviderSection
                    key={provider}
                    provider={provider}
                    settings={settings}
                  />
                ))}
              </div>

              <div className="flex items-center justify-between gap-md border-t border-line pt-lg">
                <p className="text-caption text-ink-body">
                  키가 없어도 가져오기와 읽기는 사용할 수 있고, AI 작업은 대기합니다.
                </p>
                <Button
                  variant={anyKey ? "primary" : "outline"}
                  onClick={finish}
                  loading={updateSettings.isPending}
                >
                  {anyKey ? "시작하기" : "나중에 설정"}
                </Button>
              </div>
            </>
          )}

          {error && (
            <p role="alert" className="text-caption text-danger">
              {error}
            </p>
          )}
        </div>
      </Card>
    </main>
  );
}
