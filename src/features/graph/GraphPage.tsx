import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import cytoscape, { type Core } from "cytoscape";
import { FileDown, Maximize2, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import type { TopicEdgeType, TopicNode } from "@/lib/types";

/**
 * The library's second brain: one graph of concepts (tags) distilled from the
 * AI analyses. Selecting a node opens its accumulated concept note — what the
 * concept is, and what it means in each paper that touched it. Green marks the
 * active selection only; everything else is grayscale and opacity
 * (DEVELOPMENT.md §12.2).
 */
export function GraphPage({ onOpenPaper }: { onOpenPaper: (paperId: string) => void }) {
  return (
    <div className="flex h-full flex-col">
      <TopicGraphView toggle={null} onOpenPaper={onOpenPaper} />
    </div>
  );
}

const reducedMotionQuery = () =>
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * Focus a selection: fade everything, then restore the selected node, its direct
 * neighbours, and the edges between them. Green marks the selection, the rest
 * recedes (DESIGN.md §12.2). Guarded so the jsdom cytoscape stub — whose event
 * targets only expose `id()` — is a no-op instead of a crash.
 */
function focusNeighborhood(cy: Core, node: { closedNeighborhood?: () => { removeClass: (c: string) => void } }) {
  if (typeof cy.elements !== "function" || typeof node.closedNeighborhood !== "function") return;
  cy.elements().addClass("faded");
  // closedNeighborhood = the node itself plus its adjacent nodes and the edges
  // connecting them.
  node.closedNeighborhood().removeClass("faded");
}

function clearFocus(cy: Core) {
  if (typeof cy.elements !== "function") return;
  cy.elements().removeClass("faded");
}

// --- topic graph -------------------------------------------------------------

const TOPIC_EDGE_TYPES: Array<{ id: TopicEdgeType; label: string }> = [
  { id: "cooccurrence", label: "함께 등장" },
  { id: "semantic", label: "의미 유사" },
];

function TopicGraphView({
  toggle,
  onOpenPaper,
}: {
  toggle: React.ReactNode;
  onOpenPaper: (paperId: string) => void;
}) {
  const client = useQueryClient();
  const graph = useQuery({ queryKey: ["topic-graph"], queryFn: () => api.getTopicGraph(false) });
  const rebuild = useMutation({
    mutationFn: () => api.getTopicGraph(true),
    onSuccess: (data) => client.setQueryData(["topic-graph"], data),
  });
  const exportObsidian = useMutation({ mutationFn: () => api.exportGraphToObsidian() });

  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);
  const [selected, setSelected] = useState<TopicNode | null>(null);
  const [edgeFilter, setEdgeFilter] = useState<TopicEdgeType[]>(["cooccurrence", "semantic"]);
  const reducedMotion = useMemo(reducedMotionQuery, []);

  const elements = useMemo(() => {
    if (!graph.data) return [];
    const maxCount = Math.max(1, ...graph.data.nodes.map((node) => node.paperCount));
    const maxWeight = Math.max(
      1,
      ...graph.data.edges.filter((e) => e.edgeType === "cooccurrence").map((e) => e.weight),
    );

    const nodes = graph.data.nodes.map((node) => ({
      data: {
        id: node.id,
        label: node.label,
        // Size scales with how many papers discuss the concept.
        size: 16 + 30 * (node.paperCount / maxCount),
      },
    }));
    const ids = new Set(nodes.map((node) => node.data.id));

    const edges = graph.data.edges
      .filter((edge) => edgeFilter.includes(edge.edgeType))
      .filter((edge) => ids.has(edge.source) && ids.has(edge.target))
      .map((edge, index) => ({
        data: {
          id: `e${index}`,
          source: edge.source,
          target: edge.target,
          type: edge.edgeType,
          width: edge.edgeType === "cooccurrence" ? 1 + 3 * (edge.weight / maxWeight) : 1,
        },
      }));

    return [...nodes, ...edges];
  }, [graph.data, edgeFilter]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || elements.length === 0) return;

    const cy = cytoscape({
      container,
      elements,
      style: [
        {
          selector: "node",
          style: {
            "background-color": "#c2c2c2",
            label: "data(label)",
            "font-size": "11px",
            color: "#333333",
            "text-wrap": "ellipsis",
            "text-max-width": "120px",
            "text-valign": "bottom",
            "text-margin-y": 4,
            width: "data(size)",
            height: "data(size)",
          },
        },
        {
          selector: "node:selected",
          style: { "background-color": "#00c473", color: "#000000" },
        },
        {
          selector: "edge",
          style: { "curve-style": "haystack", "line-color": "#c2c2c2", opacity: 0.5 },
        },
        {
          selector: 'edge[type="cooccurrence"]',
          style: { "line-color": "#8a8a8a", width: "data(width)", opacity: 0.7 },
        },
        {
          selector: 'edge[type="semantic"]',
          style: { "line-color": "#c2c2c2", "line-style": "dashed", opacity: 0.45 },
        },
        { selector: "edge:selected", style: { "line-color": "#00c473", opacity: 1 } },
        // Everything outside the current selection recedes (DESIGN.md §12.2).
        { selector: "node.faded", style: { opacity: 0.15 } },
        { selector: "edge.faded", style: { opacity: 0.08 } },
      ],
      layout: {
        name: "cose",
        animate: !reducedMotion,
        animationDuration: 320,
        nodeRepulsion: () => 9000,
        idealEdgeLength: () => 90,
      },
      wheelSensitivity: 0.2,
    });

    cy.on("select", "node", (event) => {
      const id = event.target.id();
      setSelected(graph.data?.nodes.find((node) => node.id === id) ?? null);
      focusNeighborhood(cy, event.target);
    });
    cy.on("unselect", "node", () => {
      setSelected(null);
      clearFocus(cy);
    });

    cyRef.current = cy;
    return () => {
      cy.destroy();
      cyRef.current = null;
    };
  }, [elements, graph.data, reducedMotion]);

  const empty = graph.isSuccess && graph.data.nodes.length === 0;

  return (
    <GraphLayout
      toggle={toggle}
      containerRef={containerRef}
      onFit={() => cyRef.current?.fit(undefined, 40)}
      extraToolbar={
        <>
          <Button
            variant="outline"
            size="sm"
            onClick={() => rebuild.mutate()}
            loading={rebuild.isPending}
          >
            <RefreshCw aria-hidden className="h-4 w-4" />
            재구성
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => exportObsidian.mutate()}
            loading={exportObsidian.isPending}
            title={
              exportObsidian.isError
                ? errorMessage(exportObsidian.error)
                : exportObsidian.data != null
                  ? `${exportObsidian.data}개 토픽 노트를 내보냈습니다`
                  : "토픽 그래프를 Obsidian 보관함으로 내보내기"
            }
          >
            <FileDown aria-hidden className="h-4 w-4" />
            {exportObsidian.data != null ? `Obsidian ✓ ${exportObsidian.data}` : "Obsidian 내보내기"}
          </Button>
        </>
      }
      empty={
        empty
          ? {
              title: "아직 토픽이 없습니다",
              description:
                "논문을 가져오고 AI 정리가 끝나면 요약에서 토픽을 뽑아 개념 지도를 만듭니다.",
            }
          : undefined
      }
      error={
        graph.isError
          ? {
              description: `${errorMessage(graph.error)} 잠시 후 다시 시도해 주세요.`,
              onRetry: () => graph.refetch(),
              retrying: graph.isFetching,
            }
          : undefined
      }
      loading={graph.isPending}
      sidebar={
        <>
          {rebuild.isError && (
            <p role="alert" className="text-caption text-danger">
              {errorMessage(rebuild.error)} 개념 지도를 다시 만들지 못했습니다. 아래 재구성을 다시
              눌러 주세요.
            </p>
          )}
          <section className="flex flex-col gap-sm">
            <h2 className="text-caption font-medium text-ink-body">연결 유형</h2>
            <div className="flex flex-wrap gap-1.5">
              {TOPIC_EDGE_TYPES.map((type) => {
                const active = edgeFilter.includes(type.id);
                return (
                  <FilterChip
                    key={type.id}
                    active={active}
                    onClick={() =>
                      setEdgeFilter((current) =>
                        active ? current.filter((id) => id !== type.id) : [...current, type.id],
                      )
                    }
                  >
                    {type.label}
                  </FilterChip>
                );
              })}
            </div>
            <p className="text-caption text-ink-subhead">
              원의 크기는 그 개념을 다룬 논문 수예요. 개념을 누르면 이어진 개념만 밝게 남습니다.
            </p>
          </section>

          {selected && (
            <Card className="flex flex-col gap-sm p-md">
              <CardTitle>{selected.label}</CardTitle>
              <Badge>{selected.paperCount}편의 논문</Badge>
              <CardDescription>이 토픽을 다룬 논문 — 눌러서 엽니다.</CardDescription>
              <ul className="flex flex-col gap-1">
                {selected.papers.map((paper) => (
                  <li key={paper.id}>
                    <button
                      onClick={() => onOpenPaper(paper.id)}
                      className="w-full rounded-sm px-2 py-1 text-left text-caption text-ink-body hover:bg-canvas-soft hover:text-primary"
                    >
                      {paper.title}
                    </button>
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </>
      }
    />
  );
}

// --- shared layout -----------------------------------------------------------

function GraphLayout({
  toggle,
  containerRef,
  onFit,
  sidebar,
  extraToolbar,
  empty,
  error,
  loading,
}: {
  toggle: React.ReactNode;
  containerRef: React.RefObject<HTMLDivElement | null>;
  onFit: () => void;
  sidebar: React.ReactNode;
  extraToolbar?: React.ReactNode;
  empty?: { title: string; description: string };
  error?: { description: string; onRetry: () => void; retrying?: boolean };
  loading?: boolean;
}) {
  // One overlay at a time; an error takes precedence over an in-flight load.
  return (
    <div className="flex min-h-0 flex-1">
      <div className="relative min-w-0 flex-1">
        {/* The canvas always mounts so its ref is available; the state overlays
            sit on top of it. */}
        <div ref={containerRef} className="h-full w-full bg-canvas-soft" />

        {error ? (
          <div
            role="alert"
            className="absolute inset-0 flex flex-col items-center justify-center gap-md p-section text-center"
          >
            <Eyebrow>관계 그래프</Eyebrow>
            <CardTitle>그래프를 불러오지 못했습니다</CardTitle>
            <CardDescription>{error.description}</CardDescription>
            <Button size="sm" onClick={error.onRetry} loading={error.retrying}>
              다시 시도
            </Button>
          </div>
        ) : empty ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-md p-section text-center">
            <Eyebrow>관계 그래프</Eyebrow>
            <CardTitle>{empty.title}</CardTitle>
            <CardDescription>{empty.description}</CardDescription>
          </div>
        ) : loading ? (
          <div
            aria-live="polite"
            className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-sm p-section text-center"
          >
            <p className="animate-pulse text-body text-ink-body motion-reduce:animate-none">
              그래프를 준비하는 중이에요
            </p>
          </div>
        ) : null}

        <div className="absolute left-md top-md flex items-center gap-sm">
          {toggle}
          <Button variant="outline" size="sm" onClick={onFit} disabled={loading || !!error}>
            <Maximize2 aria-hidden className="h-4 w-4" />
            전체 보기
          </Button>
          {extraToolbar}
        </div>
      </div>

      <aside
        aria-label="그래프 필터"
        className="flex w-[280px] shrink-0 flex-col gap-lg overflow-y-auto border-l border-line bg-canvas p-md"
      >
        {sidebar}
      </aside>
    </div>
  );
}

function FilterChip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "rounded-sm border px-2 py-1 text-caption transition-colors duration-fast",
        active
          ? "border-primary bg-primary-soft text-primary"
          : "border-line text-ink-body hover:border-primary",
      )}
    >
      {children}
    </button>
  );
}
