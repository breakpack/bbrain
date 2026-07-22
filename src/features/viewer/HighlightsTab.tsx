import { useMutation } from "@tanstack/react-query";
import { Sparkles, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/Button";
import { CardDescription } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import { HIGHLIGHT_COLORS, type HighlightColor } from "@/lib/types";
import { useDeleteHighlight, useHighlights, useUpdateHighlight } from "./queries";

const SWATCH: Record<HighlightColor, string> = {
  yellow: "rgba(250, 204, 21, 0.6)",
  green: "rgba(0, 196, 115, 0.5)",
  blue: "rgba(59, 130, 246, 0.5)",
  pink: "rgba(236, 72, 153, 0.5)",
  purple: "rgba(168, 85, 247, 0.5)",
};

export function HighlightsTab({
  paperId,
  onJump,
}: {
  paperId: string;
  onJump: (pageNumber: number) => void;
}) {
  const highlights = useHighlights(paperId);
  const updateHighlight = useUpdateHighlight(paperId);
  const deleteHighlight = useDeleteHighlight(paperId);
  const summarize = useMutation({ mutationFn: () => api.summarizeHighlights(paperId) });

  if (highlights.isPending) {
    return (
      <div className="flex flex-col gap-sm p-md" aria-busy="true" aria-label="불러오는 중">
        {[0, 1, 2].map((row) => (
          <div key={row} className="h-16 animate-pulse rounded-sm bg-canvas-soft" />
        ))}
      </div>
    );
  }

  if (!highlights.data || highlights.data.length === 0) {
    return (
      <div className="p-lg">
        <CardDescription>
          본문에서 텍스트를 선택하면 색상 도구가 나타납니다. 하이라이트는 확대·축소와 앱
          재실행 후에도 같은 위치에 남습니다.
        </CardDescription>
      </div>
    );
  }

  // One row per logical selection: a multi-page selection is stored per page.
  const seen = new Set<string>();
  const rows = highlights.data.filter((highlight) => {
    const key = highlight.groupKey ?? highlight.id;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });

  return (
    <div className="flex flex-col">
      <div className="flex flex-col gap-sm border-b border-line p-md">
        <Button
          size="sm"
          variant="outline"
          onClick={() => summarize.mutate()}
          loading={summarize.isPending}
        >
          <Sparkles aria-hidden className="h-4 w-4" />
          하이라이트 AI 종합
        </Button>
        {summarize.isError && (
          <p role="alert" className="text-caption text-danger">
            {errorMessage(summarize.error)}
          </p>
        )}
        {summarize.data && (
          <div className="rounded-sm bg-canvas-soft p-md text-caption leading-relaxed text-ink">
            {summarize.data}
          </div>
        )}
      </div>

      <ul className="flex flex-col">
        {rows.map((highlight) => (
        <li key={highlight.id} className="border-b border-line p-md">
          <button
            onClick={() => onJump(highlight.pageNumber)}
            className="mb-sm block w-full text-left"
          >
            <span className="mb-1 block text-caption text-ink-body">
              {highlight.pageNumber}쪽
            </span>
            <span
              className="block rounded-sm px-2 py-1 text-caption text-ink"
              style={{ background: SWATCH[highlight.color] }}
            >
              {highlight.selectedText}
            </span>
          </button>

          <div className="flex items-center gap-1">
            {HIGHLIGHT_COLORS.map((color) => (
              <button
                key={color}
                aria-label={`${color} 색으로 변경`}
                aria-pressed={highlight.color === color}
                onClick={() => updateHighlight.mutate({ id: highlight.id, color })}
                className={cn(
                  "h-5 w-5 rounded-sm border",
                  highlight.color === color ? "border-primary" : "border-line",
                )}
                style={{ background: SWATCH[color] }}
              />
            ))}

            <Button
              variant="ghost"
              size="sm"
              className="ml-auto"
              aria-label="하이라이트 삭제"
              onClick={() => deleteHighlight.mutate(highlight.id)}
            >
              <Trash2 aria-hidden className="h-4 w-4" />
            </Button>
          </div>
        </li>
        ))}
      </ul>
    </div>
  );
}
