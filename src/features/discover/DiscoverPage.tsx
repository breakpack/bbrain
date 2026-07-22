import { BookOpen, ExternalLink, Quote, Search } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { errorMessage } from "@/lib/ipc";
import type { DiscoveredPaper, ImportOutcome } from "@/lib/types";
import { useImportDiscoveredPaper, useSearchPapers } from "./queries";

const PAGE_SIZE = 20;

/**
 * "논문 찾기" — searches trustworthy scholarly sources (Semantic Scholar) for
 * papers on a topic and imports the open-access ones straight into the library.
 * The network and the source live in the Rust core (api.searchPapers /
 * api.importDiscoveredPaper); this page only drives the search and renders it.
 */
export function DiscoverPage({ onOpenPaper }: { onOpenPaper: (paperId: string) => void }) {
  const [input, setInput] = useState("");
  const [submitted, setSubmitted] = useState("");
  const [openAccessOnly, setOpenAccessOnly] = useState(false);
  const [hits, setHits] = useState<DiscoveredPaper[]>([]);
  const [total, setTotal] = useState(0);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const search = useSearchPapers();

  const runSearch = async (query: string, offset: number) => {
    setError(null);
    try {
      const results = await search.mutateAsync({
        query,
        offset,
        limit: PAGE_SIZE,
        openAccessOnly: openAccessOnly || undefined,
      });
      setHits((current) => (offset === 0 ? results.hits : [...current, ...results.hits]));
      setTotal(results.total);
      setNextOffset(results.nextOffset);
    } catch (cause) {
      setError(errorMessage(cause));
      if (offset === 0) {
        setHits([]);
        setTotal(0);
        setNextOffset(null);
      }
    }
  };

  const onSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    const query = input.trim();
    if (query.length === 0) return;
    setSubmitted(query);
    setHits([]);
    void runSearch(query, 0);
  };

  const firstSearchPending = search.isPending && hits.length === 0;

  return (
    <div className="h-full overflow-y-auto p-xl">
      <div className="mx-auto flex max-w-[840px] flex-col gap-lg">
        <header className="flex flex-col gap-xs">
          <Eyebrow>논문 찾기</Eyebrow>
          <h1 className="text-subheading text-ink-heading">주제로 논문 검색</h1>
          <p className="text-caption text-ink-body">
            Semantic Scholar에서 신뢰할 만한 학술 논문을 찾아, 무료로 공개된 PDF는 바로
            라이브러리로 가져옵니다.
          </p>
        </header>

        <form onSubmit={onSubmit} className="flex flex-col gap-sm">
          <div className="flex items-end gap-md">
            <div className="relative flex-1">
              <Search
                aria-hidden
                className="pointer-events-none absolute left-3 top-[13px] h-[18px] w-[18px] text-ink-body"
              />
              <Input
                className="pl-10"
                type="search"
                aria-label="주제 또는 키워드"
                placeholder="예: attention mechanism, 확산 모델, protein folding"
                value={input}
                onChange={(event) => setInput(event.target.value)}
              />
            </div>
            <Button type="submit" loading={firstSearchPending} disabled={input.trim().length === 0}>
              <Search aria-hidden className="h-[18px] w-[18px]" />
              검색
            </Button>
          </div>

          <label className="flex items-center gap-2 text-caption text-ink-body">
            <input
              type="checkbox"
              checked={openAccessOnly}
              onChange={(event) => setOpenAccessOnly(event.target.checked)}
              className="h-4 w-4 accent-primary"
            />
            무료로 가져올 수 있는 논문만 보기
          </label>
        </form>

        {error && (
          <p role="alert" className="text-caption text-danger">
            {error} 잠시 후 다시 시도하거나 검색어를 바꿔 보세요.
          </p>
        )}

        {firstSearchPending && <ResultsSkeleton />}

        {!firstSearchPending && submitted && hits.length === 0 && !error && (
          <div className="flex flex-col items-center gap-sm py-section text-center">
            <CardTitle>"{submitted}"에 대한 결과가 없습니다</CardTitle>
            <CardDescription>
              검색어를 더 일반적인 표현으로 바꾸거나 영어 키워드로 시도해 보세요.
            </CardDescription>
          </div>
        )}

        {hits.length > 0 && (
          <>
            <p className="text-caption text-ink-body">
              약 {total.toLocaleString("ko")}건 중 {hits.length}건 표시
            </p>
            <ul className="flex flex-col gap-md">
              {hits.map((paper) => (
                <li key={paper.id}>
                  <ResultCard paper={paper} onOpenPaper={onOpenPaper} />
                </li>
              ))}
            </ul>

            {nextOffset !== null && (
              <div className="flex justify-center pt-sm">
                <Button
                  variant="outline"
                  loading={search.isPending}
                  onClick={() => void runSearch(submitted, nextOffset)}
                >
                  더 보기
                </Button>
              </div>
            )}
          </>
        )}

        {!submitted && !firstSearchPending && (
          <div className="flex flex-col items-center gap-sm py-section text-center">
            <CardTitle>찾고 싶은 논문의 주제를 입력하세요</CardTitle>
            <CardDescription>
              제목, 키워드, 연구 문제 어느 쪽으로 검색해도 됩니다.
            </CardDescription>
          </div>
        )}
      </div>
    </div>
  );
}

function ResultCard({
  paper,
  onOpenPaper,
}: {
  paper: DiscoveredPaper;
  onOpenPaper: (paperId: string) => void;
}) {
  const importPaper = useImportDiscoveredPaper();
  const [outcome, setOutcome] = useState<ImportOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runImport = async () => {
    setError(null);
    try {
      setOutcome(await importPaper.mutateAsync({ paper }));
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  // An imported/duplicate outcome carries the local paper id, so the card can
  // offer to open it in the reader.
  const localPaperId =
    outcome && outcome.outcome !== "rejected" ? outcome.paperId : null;
  const inLibrary = paper.alreadyInLibrary || localPaperId !== null;
  const importable = paper.pdfUrl !== null;

  return (
    <Card className="flex flex-col gap-sm p-md">
      <div className="flex items-start justify-between gap-md">
        <h2 className="text-caption font-medium text-ink-heading">{paper.title}</h2>
        <a
          href={paper.url}
          target="_blank"
          rel="noreferrer"
          className="flex shrink-0 items-center gap-1 text-caption text-ink-body hover:text-primary"
        >
          <ExternalLink aria-hidden className="h-4 w-4" />
          출처
        </a>
      </div>

      <div className="flex flex-wrap items-center gap-x-md gap-y-1 text-caption text-ink-body">
        <span>
          {paper.authors.slice(0, 3).join(", ") || "저자 미상"}
          {paper.authors.length > 3 && ` 외 ${paper.authors.length - 3}명`}
        </span>
        {paper.year !== null && <span>{paper.year}</span>}
        {paper.venue && <span className="truncate">{paper.venue}</span>}
        {paper.citationCount !== null && (
          <span className="flex items-center gap-1">
            <Quote aria-hidden className="h-3.5 w-3.5" />
            인용 {paper.citationCount.toLocaleString("ko")}
          </span>
        )}
      </div>

      {paper.abstract && (
        <p className="line-clamp-3 text-caption text-ink-body">{paper.abstract}</p>
      )}

      {error && (
        <p role="alert" className="text-caption text-danger">
          {error}
        </p>
      )}

      <div className="flex items-center gap-sm pt-xs">
        {localPaperId !== null ? (
          <Button size="sm" onClick={() => onOpenPaper(localPaperId)}>
            <BookOpen aria-hidden className="h-4 w-4" />
            {outcome?.outcome === "duplicate" ? "라이브러리에서 열기" : "읽기"}
          </Button>
        ) : inLibrary ? (
          <span className="text-caption text-ink-body">이미 라이브러리에 있습니다</span>
        ) : importable ? (
          <Button size="sm" loading={importPaper.isPending} onClick={() => void runImport()}>
            라이브러리에 가져오기
          </Button>
        ) : (
          <span className="text-caption text-ink-body">
            무료 PDF가 없어 가져올 수 없습니다 — 출처에서 확인하세요
          </span>
        )}
      </div>
    </Card>
  );
}

function ResultsSkeleton() {
  return (
    <div className="flex flex-col gap-md" aria-busy="true" aria-label="검색하는 중">
      {[0, 1, 2].map((row) => (
        <div key={row} className="flex flex-col gap-2 rounded-card border border-line p-md">
          <div className="h-4 w-2/3 animate-pulse rounded-sm bg-canvas-soft" />
          <div className="h-3 w-1/3 animate-pulse rounded-sm bg-canvas-soft" />
          <div className="h-3 w-full animate-pulse rounded-sm bg-canvas-soft" />
          <div className="h-3 w-5/6 animate-pulse rounded-sm bg-canvas-soft" />
        </div>
      ))}
    </div>
  );
}
