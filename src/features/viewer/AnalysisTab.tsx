import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { CardDescription } from "@/components/ui/Card";
import { api, errorMessage } from "@/lib/ipc";
import type { Claim, Finding } from "@/lib/types";

/**
 * Rendered from the validated JSON, not from model-authored Markdown, and every
 * claim carries the pages it can be checked against (DEVELOPMENT.md §3, §10.3).
 */
export function AnalysisTab({
  paperId,
  onJump,
}: {
  paperId: string;
  onJump: (pageNumber: number) => void;
}) {
  const client = useQueryClient();

  const analysis = useQuery({
    queryKey: ["analysis", paperId],
    queryFn: () => api.getAnalysis(paperId),
  });

  const reanalyze = useMutation({
    mutationFn: () => api.reanalyzePaper(paperId),
    onSuccess: () => client.invalidateQueries({ queryKey: ["analysis", paperId] }),
  });

  if (analysis.isPending) {
    return (
      <div className="flex flex-col gap-sm p-md" aria-busy="true" aria-label="불러오는 중">
        {[0, 1, 2].map((row) => (
          <div key={row} className="h-20 animate-pulse rounded-sm bg-canvas-soft" />
        ))}
      </div>
    );
  }

  if (!analysis.data) {
    return (
      <div className="flex flex-col gap-md p-lg">
        <CardDescription>
          아직 AI 정리가 없습니다. API 키를 설정하면 가져온 논문이 자동으로 분석되고, 여기에
          요약·기여점·방법론·결과·한계가 정리됩니다.
        </CardDescription>
        <div>
          <Button variant="outline" size="sm" onClick={() => reanalyze.mutate()} loading={reanalyze.isPending}>
            <RefreshCw aria-hidden className="h-[18px] w-[18px]" />
            지금 분석
          </Button>
        </div>
        {reanalyze.isError && (
          <p role="alert" className="text-caption text-danger">
            {errorMessage(reanalyze.error)}
          </p>
        )}
      </div>
    );
  }

  const { analysis: data, provider, model } = analysis.data;

  return (
    <div className="flex flex-col gap-lg p-md">
      <Section title="요약">
        <p className="text-caption text-ink">{data.shortSummary}</p>
        {data.detailedSummary && (
          <p className="mt-sm text-caption text-ink-body">{data.detailedSummary}</p>
        )}
      </Section>

      <Section title="연구 문제">
        <p className="text-caption text-ink">{data.researchProblem}</p>
      </Section>

      <Section title="기여점">
        <EvidenceList
          items={data.contributions.map((item: Claim) => ({
            text: item.claim,
            pages: item.evidencePages,
          }))}
          onJump={onJump}
        />
      </Section>

      <Section title="방법론">
        <p className="text-caption text-ink">{data.methodology}</p>
      </Section>

      <Section title="주요 결과">
        <EvidenceList
          items={data.results.map((item: Finding) => ({
            text: item.finding,
            pages: item.evidencePages,
          }))}
          onJump={onJump}
        />
      </Section>

      <Section title="한계">
        <ul className="flex flex-col gap-xs">
          {data.limitations.map((limitation, index) => (
            <li key={index} className="text-caption text-ink">
              • {limitation}
            </li>
          ))}
        </ul>
      </Section>

      {data.suggestedTags.length > 0 && (
        <Section title="제안 태그">
          <div className="flex flex-wrap gap-1.5">
            {data.suggestedTags.map((tag) => (
              <Badge key={tag}>{tag}</Badge>
            ))}
          </div>
        </Section>
      )}

      {data.followUpQuestions.length > 0 && (
        <Section title="후속 연구 질문">
          <ul className="flex flex-col gap-xs">
            {data.followUpQuestions.map((question, index) => (
              <li key={index} className="text-caption text-ink">
                • {question}
              </li>
            ))}
          </ul>
        </Section>
      )}

      <div className="flex items-center justify-between gap-md border-t border-line pt-md">
        <span className="text-caption text-ink-body">
          {provider} · {model}
        </span>
        <Button variant="ghost" size="sm" onClick={() => reanalyze.mutate()} loading={reanalyze.isPending}>
          <RefreshCw aria-hidden className="h-[18px] w-[18px]" />
          다시 분석
        </Button>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-sm">
      <h3 className="text-caption font-bold text-ink-heading">{title}</h3>
      {children}
    </section>
  );
}

function EvidenceList({
  items,
  onJump,
}: {
  items: Array<{ text: string; pages: number[] }>;
  onJump: (pageNumber: number) => void;
}) {
  if (items.length === 0) {
    return <p className="text-caption text-ink-body">추출된 항목이 없습니다.</p>;
  }

  return (
    <ul className="flex flex-col gap-sm">
      {items.map((item, index) => (
        <li key={index} className="text-caption text-ink">
          {item.text}
          {item.pages.length > 0 && (
            <span className="ml-2 inline-flex gap-1">
              {item.pages.map((page) => (
                <button
                  key={page}
                  onClick={() => onJump(page)}
                  className="rounded-sm border border-line px-1.5 py-0.5 text-caption text-ink-body transition-colors duration-fast hover:border-primary hover:text-primary"
                >
                  {page}쪽
                </button>
              ))}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
