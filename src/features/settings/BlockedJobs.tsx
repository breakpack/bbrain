import { AlertCircle, KeyRound, RotateCw } from "lucide-react";

import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { useBlockedJobs, useRetryJob } from "@/features/library/queries";
import type { JobType } from "@/lib/types";

const JOB_LABEL: Record<JobType, string> = {
  extract: "본문 추출",
  thumbnail: "썸네일 생성",
  embed: "임베딩",
  analyze: "AI 분석",
  relations: "관계 계산",
  obsidian_sync: "Obsidian 동기화",
};

/**
 * Every failure states what happened and the next safe action the user can take
 * (DEVELOPMENT.md §17).
 */
export function BlockedJobs() {
  const jobs = useBlockedJobs();
  const retry = useRetryJob();

  if (!jobs.data || jobs.data.length === 0) return null;

  return (
    <Card className="flex flex-col gap-lg">
      <div className="flex flex-col gap-xs">
        <CardTitle>처리하지 못한 작업</CardTitle>
        <CardDescription>
          아래 작업은 자동으로 재시도하지 않습니다. 원인을 해결한 뒤 다시 시도하세요.
        </CardDescription>
      </div>

      <ul className="flex flex-col gap-md">
        {jobs.data.map((job) => {
          const waiting = job.status === "waiting_for_key";

          return (
            <li key={job.id} className="flex items-center justify-between gap-md">
              <div className="flex min-w-0 items-start gap-md">
                {waiting ? (
                  <KeyRound aria-hidden className="mt-0.5 h-[18px] w-[18px] shrink-0 text-ink-body" />
                ) : (
                  <AlertCircle aria-hidden className="mt-0.5 h-[18px] w-[18px] shrink-0 text-danger" />
                )}
                <div className="min-w-0">
                  <p className="truncate text-caption text-ink">
                    {job.paperTitle ?? "논문"} — {JOB_LABEL[job.jobType]}
                  </p>
                  <p className="text-caption text-ink-body">
                    {waiting
                      ? "API 키를 설정하면 자동으로 다시 실행됩니다."
                      : `${job.attempts}번 시도했지만 실패했습니다.`}
                  </p>
                </div>
              </div>

              <Button
                variant="ghost"
                size="sm"
                onClick={() => retry.mutate(job.id)}
                loading={retry.isPending}
              >
                <RotateCw aria-hidden className="h-4 w-4" />
                다시 시도
              </Button>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}
