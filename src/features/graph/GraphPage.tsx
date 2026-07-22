import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import cytoscape, { type Core } from "cytoscape";
import { ArrowLeft, GitFork, Maximize2, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { useGroups } from "@/features/library/queries";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import type {
  GraphNode,
  Lineage,
  NeighborEdgeType,
  NeighborNode,
  PaperNeighborhood,
  RelationType,
  TopicEdgeType,
  TopicNode,
} from "@/lib/types";

type Mode = "topic" | "paper";

/**
 * Two ways to see the library: the topic graph (concepts extracted from the AI
 * analyses — the "second brain") and the paper graph (one node per paper). Green
 * marks the active selection only; everything else is grayscale and opacity
 * (DEVELOPMENT.md §12.2).
 */
export function GraphPage({ onOpenPaper }: { onOpenPaper: (paperId: string) => void }) {
  const [mode, setMode] = useState<Mode>("topic");
  const [focusPaperId, setFocusPaperId] = useState<string | null>(null);

  if (focusPaperId) {
    return (
      <NeighborhoodView
        paperId={focusPaperId}
        onBack={() => setFocusPaperId(null)}
        onFocusPaper={setFocusPaperId}
        onOpenPaper={onOpenPaper}
      />
    );
  }

  const toggle = (
    <div role="group" aria-label="그래프 종류" className="flex rounded-control border border-line">
      {(
        [
          ["topic", "토픽"],
          ["paper", "논문"],
        ] as Array<[Mode, string]>
      ).map(([id, label]) => (
        <button
          key={id}
          aria-pressed={mode === id}
          onClick={() => setMode(id)}
          className={cn(
            "px-3 py-1 text-caption transition-colors duration-fast first:rounded-l-control last:rounded-r-control",
            mode === id ? "bg-primary-soft text-primary" : "text-ink-body hover:text-ink",
          )}
        >
          {label}
        </button>
      ))}
    </div>
  );

  return (
    <div className="flex h-full flex-col">
      {mode === "topic" ? (
        <TopicGraphView toggle={toggle} onOpenPaper={onOpenPaper} />
      ) : (
        <PaperGraphView
          toggle={toggle}
          onOpenPaper={onOpenPaper}
          onFocusPaper={setFocusPaperId}
        />
      )}
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
        <Button
          variant="outline"
          size="sm"
          onClick={() => rebuild.mutate()}
          loading={rebuild.isPending}
        >
          <RefreshCw aria-hidden className="h-4 w-4" />
          재구성
        </Button>
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

// --- paper graph (one node per paper) ---------------------------------------

const EDGE_TYPES: Array<{ id: RelationType; label: string }> = [
  { id: "semantic", label: "의미 유사" },
  { id: "citation", label: "인용" },
  { id: "manual", label: "직접 연결" },
];

function PaperGraphView({
  toggle,
  onOpenPaper,
  onFocusPaper,
}: {
  toggle: React.ReactNode;
  onOpenPaper: (paperId: string) => void;
  onFocusPaper: (paperId: string) => void;
}) {
  const graph = useQuery({ queryKey: ["graph"], queryFn: api.getGraph });
  const groups = useGroups();

  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);

  const [selected, setSelected] = useState<GraphNode | null>(null);
  const [groupFilter, setGroupFilter] = useState<string | null>(null);
  const [edgeFilter, setEdgeFilter] = useState<RelationType[]>(["semantic", "citation", "manual"]);
  const reducedMotion = useMemo(reducedMotionQuery, []);

  const elements = useMemo(() => {
    if (!graph.data) return [];

    const nodes = graph.data.nodes
      .filter((node) => !groupFilter || node.groupIds.includes(groupFilter))
      .map((node) => ({ data: { id: node.id, label: node.title, year: node.year } }));
    const ids = new Set(nodes.map((node) => node.data.id));

    const edges = graph.data.edges
      .filter((edge) => edgeFilter.includes(edge.relationType))
      .filter((edge) => ids.has(edge.sourcePaperId) && ids.has(edge.targetPaperId))
      .map((edge, index) => ({
        data: {
          id: `e${index}`,
          source: edge.sourcePaperId,
          target: edge.targetPaperId,
          type: edge.relationType,
        },
      }));

    return [...nodes, ...edges];
  }, [graph.data, groupFilter, edgeFilter]);

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
            "font-size": "10px",
            color: "#333333",
            "text-wrap": "ellipsis",
            "text-max-width": "120px",
            "text-valign": "bottom",
            "text-margin-y": 4,
            width: 18,
            height: 18,
          },
        },
        {
          selector: "node:selected",
          style: { "background-color": "#00c473", width: 24, height: 24, color: "#000000" },
        },
        {
          selector: "edge",
          style: { width: 1, "line-color": "#c2c2c2", "curve-style": "haystack", opacity: 0.5 },
        },
        { selector: 'edge[type="citation"]', style: { "line-color": "#a2a2a2", opacity: 0.8 } },
        {
          selector: 'edge[type="manual"]',
          style: { "line-color": "#333333", opacity: 0.9, width: 2 },
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
        nodeRepulsion: () => 8000,
        idealEdgeLength: () => 80,
      },
      wheelSensitivity: 0.2,
    });

    cy.on("select", "node", (event) => {
      const id = event.target.id();
      setSelected(graph.data?.nodes.find((candidate) => candidate.id === id) ?? null);
      focusNeighborhood(cy, event.target);
    });
    cy.on("unselect", "node", () => {
      setSelected(null);
      clearFocus(cy);
    });
    cy.on("dbltap", "node", (event) => onOpenPaper(event.target.id()));

    cyRef.current = cy;
    return () => {
      cy.destroy();
      cyRef.current = null;
    };
  }, [elements, graph.data, onOpenPaper, reducedMotion]);

  const empty = graph.isSuccess && graph.data.nodes.length === 0;

  return (
    <GraphLayout
      toggle={toggle}
      containerRef={containerRef}
      onFit={() => cyRef.current?.fit(undefined, 40)}
      loading={graph.isPending}
      error={
        graph.isError
          ? {
              description: `${errorMessage(graph.error)} 잠시 후 다시 시도해 주세요.`,
              onRetry: () => graph.refetch(),
              retrying: graph.isFetching,
            }
          : undefined
      }
      empty={
        empty
          ? {
              title: "아직 그래프에 표시할 논문이 없습니다",
              description: "논문을 가져오고 분석이 끝나면 의미가 비슷한 논문끼리 자동으로 연결됩니다.",
            }
          : undefined
      }
      sidebar={
        <>
          <section className="flex flex-col gap-sm">
            <h2 className="text-caption font-medium text-ink-body">관계 유형</h2>
            <div className="flex flex-wrap gap-1.5">
              {EDGE_TYPES.map((type) => {
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
          </section>

          <section className="flex flex-col gap-sm">
            <h2 className="text-caption font-medium text-ink-body">그룹</h2>
            <div className="flex flex-wrap gap-1.5">
              <FilterChip active={groupFilter === null} onClick={() => setGroupFilter(null)}>
                전체
              </FilterChip>
              {groups.data?.map((group) => (
                <FilterChip
                  key={group.id}
                  active={groupFilter === group.id}
                  onClick={() => setGroupFilter(group.id)}
                >
                  {group.name}
                </FilterChip>
              ))}
            </div>
          </section>

          {selected && (
            <Card className="flex flex-col gap-sm p-md">
              <CardTitle>{selected.title}</CardTitle>
              {selected.year && <Badge>{selected.year}</Badge>}
              <CardDescription>두 번 누르면 논문이 열립니다.</CardDescription>
              <div className="flex flex-wrap gap-sm">
                <Button size="sm" onClick={() => onOpenPaper(selected.id)}>
                  논문 열기
                </Button>
                <Button variant="outline" size="sm" onClick={() => onFocusPaper(selected.id)}>
                  <GitFork aria-hidden className="h-4 w-4" />
                  이웃 보기
                </Button>
              </div>
            </Card>
          )}
        </>
      }
    />
  );
}

// --- paper neighborhood (ConnectedPapers-style focus graph) ------------------

const NEIGHBOR_EDGE_TYPES: Array<{ id: NeighborEdgeType; label: string }> = [
  { id: "similarity", label: "의미 유사" },
  { id: "citation", label: "인용" },
];

/**
 * A single paper at the centre with its nearest neighbours laid out on a year
 * axis — earlier work to the left, later work to the right. Green marks only the
 * focus paper; neighbour size grows with similarity. Selecting a neighbour lets
 * the reader open it or re-centre the graph on it.
 */
function NeighborhoodView({
  paperId,
  onBack,
  onFocusPaper,
  onOpenPaper,
}: {
  paperId: string;
  onBack: () => void;
  onFocusPaper: (paperId: string) => void;
  onOpenPaper: (paperId: string) => void;
}) {
  const graph = useQuery({
    queryKey: ["neighborhood", paperId],
    queryFn: () => api.getPaperNeighborhood(paperId),
  });

  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);
  const [selected, setSelected] = useState<NeighborNode | null>(null);
  const [edgeFilter, setEdgeFilter] = useState<NeighborEdgeType[]>(["similarity", "citation"]);
  const reducedMotion = useMemo(reducedMotionQuery, []);

  const center = graph.data?.nodes.find((node) => node.lineage === "focus") ?? null;

  const elements = useMemo(() => {
    if (!graph.data) return [];
    return neighborhoodElements(graph.data, edgeFilter);
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
            "font-size": "10px",
            color: "#333333",
            "text-wrap": "ellipsis",
            "text-max-width": "130px",
            "text-valign": "bottom",
            "text-margin-y": 4,
            width: "data(size)",
            height: "data(size)",
          },
        },
        {
          // The focus paper is the one splash of green (DESIGN.md §12.2).
          selector: 'node[focus = 1]',
          style: { "background-color": "#00c473", color: "#000000", "font-size": "12px" },
        },
        { selector: "node:selected", style: { "border-width": 3, "border-color": "#00c473" } },
        {
          selector: "edge",
          style: { width: "data(width)", "line-color": "#c2c2c2", "curve-style": "bezier", opacity: 0.5 },
        },
        {
          selector: 'edge[type="citation"]',
          style: { "line-color": "#8a8a8a", "line-style": "dashed", opacity: 0.8 },
        },
        { selector: "edge:selected", style: { "line-color": "#00c473", opacity: 1 } },
        { selector: "node.faded", style: { opacity: 0.15 } },
        { selector: "edge.faded", style: { opacity: 0.08 } },
      ],
      // Positions are precomputed (year on x), so honour them rather than run a
      // force layout that would scramble the timeline.
      layout: { name: "preset", animate: !reducedMotion, animationDuration: 320, fit: true, padding: 48 },
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
    cy.on("dbltap", "node", (event) => onOpenPaper(event.target.id()));

    cyRef.current = cy;
    return () => {
      cy.destroy();
      cyRef.current = null;
    };
  }, [elements, graph.data, onOpenPaper, reducedMotion]);

  // Only the focus paper, with no neighbours to show yet.
  const empty = graph.isSuccess && graph.data.nodes.length <= 1;

  return (
    <div className="flex min-h-0 flex-1">
      <div className="relative min-w-0 flex-1">
        <div ref={containerRef} className="h-full w-full bg-canvas-soft" />

        {graph.isError ? (
          <div
            role="alert"
            className="absolute inset-0 flex flex-col items-center justify-center gap-md p-section text-center"
          >
            <Eyebrow>이웃 그래프</Eyebrow>
            <CardTitle>이웃을 불러오지 못했습니다</CardTitle>
            <CardDescription>{errorMessage(graph.error)} 잠시 후 다시 시도해 주세요.</CardDescription>
            <Button size="sm" onClick={() => graph.refetch()} loading={graph.isFetching}>
              다시 시도
            </Button>
          </div>
        ) : empty ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-md p-section text-center">
            <Eyebrow>이웃 그래프</Eyebrow>
            <CardTitle>아직 이어질 이웃이 없어요</CardTitle>
            <CardDescription>
              분석이 끝난 논문이 늘어나면 의미가 비슷한 이웃이 이 논문 주위에 나타납니다.
            </CardDescription>
          </div>
        ) : graph.isPending ? (
          <div
            aria-live="polite"
            className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center p-section text-center"
          >
            <p className="animate-pulse text-body text-ink-body motion-reduce:animate-none">
              이웃을 찾는 중이에요
            </p>
          </div>
        ) : null}

        <div className="absolute left-md top-md flex items-center gap-sm">
          <Button variant="outline" size="sm" onClick={onBack}>
            <ArrowLeft aria-hidden className="h-4 w-4" />
            그래프로
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => cyRef.current?.fit(undefined, 48)}
            disabled={graph.isPending || graph.isError}
          >
            <Maximize2 aria-hidden className="h-4 w-4" />
            전체 보기
          </Button>
        </div>
      </div>

      <aside
        aria-label="이웃 그래프 정보"
        className="flex w-[280px] shrink-0 flex-col gap-lg overflow-y-auto border-l border-line bg-canvas p-md"
      >
        <section className="flex flex-col gap-sm">
          <Eyebrow>중심 논문</Eyebrow>
          <h2 className="text-body font-medium text-ink" title={center?.title}>
            {center?.title ?? "…"}
          </h2>
          <p className="text-caption text-ink-subhead">
            가로축은 연도예요. 왼쪽이 이전 연구(선행), 오른쪽이 이후 연구(파생)입니다.
          </p>
        </section>

        <section className="flex flex-col gap-sm">
          <h2 className="text-caption font-medium text-ink-body">연결 유형</h2>
          <div className="flex flex-wrap gap-1.5">
            {NEIGHBOR_EDGE_TYPES.map((type) => {
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
        </section>

        {selected && selected.lineage !== "focus" && (
          <Card className="flex flex-col gap-sm p-md">
            <CardTitle>{selected.title}</CardTitle>
            <div className="flex flex-wrap gap-1.5">
              {selected.year && <Badge>{selected.year}</Badge>}
              <Badge>{LINEAGE_LABEL[selected.lineage]}</Badge>
              {selected.similarity != null && (
                <Badge>유사도 {Math.round(selected.similarity * 100)}%</Badge>
              )}
            </div>
            <div className="flex flex-wrap gap-sm">
              <Button size="sm" onClick={() => onOpenPaper(selected.id)}>
                논문 열기
              </Button>
              <Button variant="outline" size="sm" onClick={() => onFocusPaper(selected.id)}>
                <GitFork aria-hidden className="h-4 w-4" />
                중심으로
              </Button>
            </div>
          </Card>
        )}
      </aside>
    </div>
  );
}

const LINEAGE_LABEL: Record<Lineage, string> = {
  focus: "중심",
  precedent: "선행 연구",
  derivative: "파생 연구",
  concurrent: "같은 시기",
};

/**
 * Lay the neighbourhood out as a timeline: one column per distinct year (earlier
 * years to the left), papers stacked within a column by similarity. Positions
 * are model coordinates; the preset layout fits them to the viewport.
 */
function neighborhoodElements(data: PaperNeighborhood, edgeFilter: NeighborEdgeType[]) {
  const nodes = data.nodes;
  if (nodes.length === 0) return [];

  const focusYear = nodes.find((node) => node.lineage === "focus")?.year ?? null;
  // Papers with no year share the focus paper's column so nothing flies off.
  const yearOf = (node: NeighborNode) => node.year ?? focusYear ?? 0;

  const years = [...new Set(nodes.map(yearOf))].sort((a, b) => a - b);
  const column = new Map(years.map((year, index) => [year, index]));

  const byColumn = new Map<number, NeighborNode[]>();
  for (const node of nodes) {
    const col = column.get(yearOf(node)) ?? 0;
    const group = byColumn.get(col) ?? [];
    group.push(node);
    byColumn.set(col, group);
  }

  const COL_GAP = 240;
  const ROW_GAP = 96;
  const positions = new Map<string, { x: number; y: number }>();
  for (const [col, group] of byColumn) {
    group.sort(
      (a, b) =>
        Number(b.lineage === "focus") - Number(a.lineage === "focus") ||
        (b.similarity ?? 0) - (a.similarity ?? 0),
    );
    group.forEach((node, index) => {
      positions.set(node.id, {
        x: col * COL_GAP,
        y: (index - (group.length - 1) / 2) * ROW_GAP,
      });
    });
  }

  const maxSimilarity = Math.max(0.001, ...nodes.map((node) => node.similarity ?? 0));
  const cyNodes = nodes.map((node) => ({
    data: {
      id: node.id,
      label: node.title,
      focus: node.lineage === "focus" ? 1 : 0,
      // The focus paper is fixed and large; neighbours grow with similarity.
      size: node.lineage === "focus" ? 42 : 18 + 22 * ((node.similarity ?? 0) / maxSimilarity),
    },
    position: positions.get(node.id),
  }));

  const ids = new Set(nodes.map((node) => node.id));
  const cyEdges = data.edges
    .filter((edge) => edgeFilter.includes(edge.edgeType))
    .filter((edge) => ids.has(edge.source) && ids.has(edge.target))
    .map((edge, index) => ({
      data: {
        id: `e${index}`,
        source: edge.source,
        target: edge.target,
        type: edge.edgeType,
        width: edge.edgeType === "similarity" ? 1 + 3 * edge.weight : 1.5,
      },
    }));

  return [...cyNodes, ...cyEdges];
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
