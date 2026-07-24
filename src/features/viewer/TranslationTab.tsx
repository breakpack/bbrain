import { useMutation, useQuery } from "@tanstack/react-query";
import { Languages } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/Button";
import { CardDescription } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import { needsTranslation } from "@/lib/language";
import type {
  NormalizedRect,
  PageTranslation,
  TranslatedUnit,
  TranslationView,
} from "@/lib/types";

const VIEWS: Array<{ id: TranslationView; label: string }> = [
  { id: "paragraph", label: "문단" },
  { id: "whole", label: "전체" },
];

/**
 * Translations live in this panel only — never overlaid on the PDF, which stays
 * the original (DEVELOPMENT.md §9.2). A page is translated in a single request;
 * the reader chooses whether to read it grouped into paragraphs or as one whole
 * flow. Hovering a sentence highlights its source rectangles; clicking jumps to
 * them.
 */
export function TranslationTab({
  paperId,
  pageNumber,
  pageCount,
  hasTextLayer,
  hoveredSentenceId,
  onHoverSentence,
}: {
  paperId: string;
  pageNumber: number;
  pageCount: number;
  hasTextLayer: boolean;
  /** The source sentence the cursor is over on the PDF; its translated unit is
   * highlighted here and scrolled into view. */
  hoveredSentenceId?: string | null;
  onHoverSentence: (pageNumber: number, rects: NormalizedRect[], jump: boolean) => void;
}) {
  const [view, setView] = useState<TranslationView>("paragraph");
  const outputRef = useRef<HTMLDivElement>(null);
  // Pages translated in this session, keyed by page. Merged with whatever the
  // backend cached from earlier sessions so a reopened viewer restores past work.
  const [byPage, setByPage] = useState<Record<number, PageTranslation>>({});
  const inFlight = useRef<Set<number>>(new Set());

  const settings = useQuery({ queryKey: ["settings"], queryFn: api.getSettings });
  const targetLanguage = settings.data?.translationLanguage ?? "ko";

  const sentences = useQuery({
    queryKey: ["sentences", paperId, pageNumber],
    queryFn: () => api.getPageSentences(paperId, pageNumber),
    enabled: hasTextLayer,
  });

  // Auto-restore a previously saved translation for this page — no network call,
  // no button press (§9.4).
  const cached = useQuery({
    queryKey: ["translation-cache", paperId, pageNumber],
    queryFn: () => api.getCachedTranslation(paperId, pageNumber),
    enabled: hasTextLayer && !byPage[pageNumber],
  });

  const translate = useMutation({
    mutationFn: () => api.translatePage(paperId, pageNumber),
    onSuccess: (result) => setByPage((current) => ({ ...current, [pageNumber]: result })),
  });

  // Translates a page in the background (cache first, network only if needed) so
  // navigating there is instant. Deduplicated and bounded to one page ahead.
  const prefetch = useCallback(
    async (page: number) => {
      if (page < 1 || page > pageCount || byPage[page] || inFlight.current.has(page)) {
        return;
      }
      inFlight.current.add(page);
      try {
        const existing = await api.getCachedTranslation(paperId, page);
        const result = existing ?? (await api.translatePage(paperId, page));
        setByPage((current) => ({ ...current, [page]: result }));
      } catch {
        // A background prefetch failure is silent; translate on demand later.
      } finally {
        inFlight.current.delete(page);
      }
    },
    [paperId, pageCount, byPage],
  );

  const translated = byPage[pageNumber] ?? cached.data ?? undefined;

  // Prefetch the next page while the reader is on the current one.
  useEffect(() => {
    if (translated) void prefetch(pageNumber + 1);
  }, [translated, pageNumber, prefetch]);

  // Scroll the unit matching the hovered PDF sentence into view, so highlighting
  // it is useful even when it is off-screen in a long page.
  useEffect(() => {
    if (!hoveredSentenceId) return;
    outputRef.current
      ?.querySelector('[data-highlighted="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [hoveredSentenceId, view]);

  // A non-Korean (foreign) paper gets its first page translated as soon as the
  // viewer opens. Runs once per paper.
  const primedPaper = useRef<string | null>(null);
  useEffect(() => {
    if (!hasTextLayer || settings.isPending || primedPaper.current === paperId) return;

    let cancelled = false;
    void api.getPageSentences(paperId, 1).then((first) => {
      if (cancelled) return;
      const text = first.map((sentence) => sentence.text).join(" ");
      if (needsTranslation(text, targetLanguage)) {
        primedPaper.current = paperId;
        void prefetch(1);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [paperId, hasTextLayer, settings.isPending, targetLanguage, prefetch]);

  if (!hasTextLayer) {
    return (
      <div className="p-lg">
        <CardDescription>
          이 PDF에는 텍스트 레이어가 없어 번역할 수 없습니다. 스캔 이미지로만 이루어진 논문은
          열람만 가능합니다.
        </CardDescription>
      </div>
    );
  }

  const sourceById = new Map((sentences.data ?? []).map((s) => [s.id, s]));
  const rectsFor = (sentenceIds: string[]): NormalizedRect[] =>
    sentenceIds.flatMap((id) => sourceById.get(id)?.rects ?? []);

  const hover = (unit: TranslatedUnit, jump: boolean) =>
    onHoverSentence(pageNumber, rectsFor(unit.sentenceIds), jump);

  const isHighlighted = (unit: TranslatedUnit) =>
    hoveredSentenceId != null && unit.sentenceIds.includes(hoveredSentenceId);

  const renderUnit = (unit: TranslatedUnit) => {
    const highlighted = isHighlighted(unit);
    return (
      <span
        key={unit.id}
        role="button"
        tabIndex={0}
        data-highlighted={highlighted || undefined}
        onMouseEnter={() => hover(unit, false)}
        onFocus={() => hover(unit, false)}
        onClick={() => hover(unit, true)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            hover(unit, true);
          }
        }}
        className={cn(
          "cursor-pointer rounded-sm hover:bg-primary-soft focus:bg-primary-soft focus:outline-none",
          highlighted && "bg-primary-soft text-primary",
        )}
      >
        {unit.text}{" "}
      </span>
    );
  };

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between gap-md border-b border-line p-md">
        <div className="flex items-center gap-md">
          <span className="text-caption text-ink-body">{pageNumber}쪽</span>
          <div role="group" aria-label="번역 보기" className="flex rounded-control border border-line">
            {VIEWS.map((option) => (
              <button
                key={option.id}
                aria-pressed={view === option.id}
                onClick={() => setView(option.id)}
                className={cn(
                  "px-3 py-1 text-caption transition-colors duration-fast",
                  "first:rounded-l-control last:rounded-r-control",
                  view === option.id
                    ? "bg-primary-soft text-primary"
                    : "text-ink-body hover:text-ink",
                )}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>

        <Button
          size="sm"
          onClick={() => translate.mutate()}
          loading={translate.isPending}
          disabled={(sentences.data?.length ?? 0) === 0}
        >
          <Languages aria-hidden className="h-[18px] w-[18px]" />
          {translated ? "다시 번역" : "현재 페이지 번역"}
        </Button>
      </div>

      {translate.isError && (
        <p role="alert" className="p-md text-caption text-danger">
          {errorMessage(translate.error)}
        </p>
      )}

      {(translate.isPending || (cached.isPending && !translated)) && (
        <div className="flex flex-col gap-sm p-md" aria-busy="true" aria-label="번역 불러오는 중">
          {[0, 1, 2, 3].map((row) => (
            <div key={row} className="h-12 animate-pulse rounded-sm bg-canvas-soft" />
          ))}
        </div>
      )}

      {!translated && !translate.isPending && !cached.isPending && (
        <div className="p-lg">
          <CardDescription>
            현재 페이지를 한 번에 번역합니다. 번역문의 문장에 마우스를 올리면 원문 위치가
            강조되고, 클릭하면 해당 위치로 이동합니다. 한 번 번역한 페이지는 다음에 열 때
            자동으로 불러옵니다.
          </CardDescription>
        </div>
      )}

      {translated && (
        <div
          ref={outputRef}
          className="p-md text-caption leading-relaxed text-ink"
          onMouseLeave={() => onHoverSentence(pageNumber, [], false)}
        >
          {view === "whole" ? (
            <p>{translated.units.map(renderUnit)}</p>
          ) : (
            <div className="flex flex-col gap-md">
              {annotateColumns(
                groupByParagraph(translated.units),
                (group) => rectsFor(group.flatMap((unit) => unit.sentenceIds)),
              ).map(({ group, divider }) => (
                <div key={group[0].id} className="flex flex-col gap-md">
                  {divider && <ColumnDivider side={divider} />}
                  <p>{group.map(renderUnit)}</p>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const COLUMN_LABEL: Record<"left" | "right", string> = {
  left: "왼쪽 단",
  right: "오른쪽 단",
};

/** A labelled rule marking where the translation crosses into the other
 * column of a two-column page — without it the panel reads as one stream. */
function ColumnDivider({ side }: { side: "left" | "right" }) {
  return (
    <div className="flex items-center gap-2 text-[11px] text-ink-body" role="separator">
      <span aria-hidden className="h-px flex-1 bg-line" />
      {COLUMN_LABEL[side]}
      <span aria-hidden className="h-px flex-1 bg-line" />
    </div>
  );
}

/** Groups consecutive units that belong to the same source paragraph. */
function groupByParagraph(units: TranslatedUnit[]): TranslatedUnit[][] {
  const groups: TranslatedUnit[][] = [];
  for (const unit of units) {
    const last = groups[groups.length - 1];
    if (last && last[0].paragraphIndex === unit.paragraphIndex) {
      last.push(unit);
    } else {
      groups.push([unit]);
    }
  }
  return groups;
}

export type ColumnSide = "left" | "right" | "full";

/**
 * Which column of the page a set of source rectangles occupies. A run wider
 * than ~55% of the page spans the gutter (title, abstract, figures) and counts
 * as full-width; otherwise its horizontal centre decides the side.
 */
export function columnOf(rects: NormalizedRect[]): ColumnSide {
  if (rects.length === 0) return "full";
  let left = Infinity;
  let right = -Infinity;
  for (const rect of rects) {
    left = Math.min(left, rect.x);
    right = Math.max(right, rect.x + rect.width);
  }
  if (right - left > 0.55) return "full";
  return (left + right) / 2 < 0.5 ? "left" : "right";
}

/**
 * Marks each paragraph group with the divider to draw before it: on pages that
 * really have both columns, every left↔right transition gets one. Single-column
 * pages get none, and full-width runs never break the current column.
 */
export function annotateColumns<T>(
  groups: T[],
  rectsOf: (group: T) => NormalizedRect[],
): Array<{ group: T; divider: "left" | "right" | null }> {
  const sides = groups.map((group) => columnOf(rectsOf(group)));
  const twoColumn = sides.includes("left") && sides.includes("right");

  let current: "left" | "right" | null = null;
  return groups.map((group, index) => {
    const side = sides[index];
    let divider: "left" | "right" | null = null;
    if (twoColumn && (side === "left" || side === "right")) {
      if (current !== null && side !== current) divider = side;
      current = side;
    }
    return { group, divider };
  });
}
