import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import cytoscape, { type Core } from "cytoscape";
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationNodeDatum,
} from "d3-force";
import { FileDown, Maximize2, RefreshCw, ZoomIn, ZoomOut } from "lucide-react";
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
 * DESIGN.md palette for the graph: grayscale dots and hairline edges on the
 * light canvas, green (the app's only accent) reserved for the active concept.
 * These feed cytoscape's canvas renderer, so they live here rather than CSS.
 */
const GRAPH = {
  node: "#c2c2c2",
  nodeLit: "#8a8a8a",
  label: "#a2a2a2",
  labelLit: "#333333",
  edge: "#e0e0e4",
  edgeLit: "#b4b4ba",
  accent: "#00c473",
};

/** Labels clutter the view when zoomed out; below this zoom they vanish. */
const LABEL_ZOOM = 0.65;

// --- live physics (Obsidian-style force controls) ----------------------------

/** The tunable forces and view settings, in the same terms Obsidian exposes them. */
type ForceSettings = {
  /** 노드끼리 밀어내는 힘 (0–2). */
  repel: number;
  /** 흩어진 그래프를 화면 중심으로 끌어당기는 힘 (0–1). */
  centerForce: number;
  /** 연결선이 양끝을 당기는 힘 (0–1). */
  linkStrength: number;
  /** 연결된 노드가 유지하려는 거리 (px). */
  linkDistance: number;
  /** 노드 크기 배율 (0.5–2). */
  nodeScale: number;
};

const DEFAULT_FORCES: ForceSettings = {
  repel: 1,
  centerForce: 0.5,
  linkStrength: 0.6,
  linkDistance: 90,
  nodeScale: 1,
};

/** Shared zoom bounds for the wheel and the on-canvas zoom bar. */
const ZOOM_MIN = 0.15;
const ZOOM_MAX = 3;

/** Zoom is perceptual, so the bar runs on a log scale: t∈[0,1] ↔ zoom level. */
const zoomToSlider = (zoom: number) =>
  Math.log(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom)) / ZOOM_MIN) / Math.log(ZOOM_MAX / ZOOM_MIN);
const sliderToZoom = (t: number) => ZOOM_MIN * Math.pow(ZOOM_MAX / ZOOM_MIN, t);

const FORCES_KEY = "bbrain.graph.forces";

function loadForces(): ForceSettings {
  try {
    const raw = localStorage.getItem(FORCES_KEY);
    if (!raw) return DEFAULT_FORCES;
    return { ...DEFAULT_FORCES, ...(JSON.parse(raw) as Partial<ForceSettings>) };
  } catch {
    return DEFAULT_FORCES;
  }
}

type SimNode = SimulationNodeDatum & { id: string; size: number };

/** Maps the 0–1/0–2 slider values onto d3-force parameter ranges. */
function applyForces(sim: Simulation<SimNode, undefined>, forces: ForceSettings) {
  (sim.force("charge") as ReturnType<typeof forceManyBody<SimNode>>).strength(
    -40 - 220 * forces.repel,
  );
  (sim.force("link") as ReturnType<typeof forceLink<SimNode, { source: string; target: string }>>)
    .distance(forces.linkDistance)
    .strength(0.05 + 0.95 * forces.linkStrength);
  (sim.force("x") as ReturnType<typeof forceX<SimNode>>).strength(0.01 + 0.19 * forces.centerForce);
  (sim.force("y") as ReturnType<typeof forceY<SimNode>>).strength(0.01 + 0.19 * forces.centerForce);
}

/** Rescales every node's rendered size; `data(size)` in the style picks it up. */
function applyNodeScale(cy: Core, scale: number) {
  if (typeof cy.nodes !== "function") return;
  cy.nodes().forEach((node) => {
    node.data("size", ((node.data("base") as number | undefined) ?? 20) * scale);
  });
}

/**
 * Focus a selection: fade everything, then restore the selected node, its direct
 * neighbours, and the edges between them. Green marks the selection, the rest
 * recedes into the void (DESIGN.md §12.2). Guarded so the jsdom cytoscape stub —
 * whose event targets only expose `id()` — is a no-op instead of a crash.
 */
function focusNeighborhood(
  cy: Core,
  node: {
    closedNeighborhood?: () => { removeClass: (c: string) => void; addClass: (c: string) => void };
  },
) {
  if (typeof cy.elements !== "function" || typeof node.closedNeighborhood !== "function") return;
  cy.elements().addClass("faded").removeClass("lit");
  // closedNeighborhood = the node itself plus its adjacent nodes and the edges
  // connecting them.
  const neighborhood = node.closedNeighborhood();
  neighborhood.removeClass("faded");
  neighborhood.addClass("lit");
}

function clearFocus(cy: Core) {
  if (typeof cy.elements !== "function") return;
  cy.elements().removeClass("faded").removeClass("lit");
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
  const simRef = useRef<Simulation<SimNode, undefined> | null>(null);
  const renderRef = useRef<(() => void) | null>(null);
  const [selected, setSelected] = useState<TopicNode | null>(null);
  const [edgeFilter, setEdgeFilter] = useState<TopicEdgeType[]>(["cooccurrence", "semantic"]);
  const [forces, setForces] = useState<ForceSettings>(loadForces);
  const forcesRef = useRef(forces);
  const [zoom, setZoom] = useState(1);
  const reducedMotion = useMemo(reducedMotionQuery, []);

  /** Zooms about the viewport centre — what both the bar and ± buttons use. */
  const applyZoom = (level: number) => {
    const cy = cyRef.current;
    const container = containerRef.current;
    if (!cy || typeof cy.zoom !== "function") return;
    cy.zoom({
      level: Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, level)),
      renderedPosition: {
        x: (container?.clientWidth ?? 0) / 2,
        y: (container?.clientHeight ?? 0) / 2,
      },
    });
  };

  const elements = useMemo(() => {
    if (!graph.data) return [];
    const maxCount = Math.max(1, ...graph.data.nodes.map((node) => node.paperCount));
    const maxWeight = Math.max(
      1,
      ...graph.data.edges.filter((e) => e.edgeType === "cooccurrence").map((e) => e.weight),
    );

    const nodes = graph.data.nodes.map((node) => {
      // Base size scales with how many papers discuss the concept; the 노드
      // 크기 slider multiplies it after mount (data(size) in the style).
      const base = 16 + 30 * (node.paperCount / maxCount);
      return { data: { id: node.id, label: node.label, base, size: base } };
    });
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
            "background-color": GRAPH.node,
            // The soft halo around each dot — Obsidian's glow, done with
            // cytoscape's underlay ring.
            "underlay-color": GRAPH.node,
            "underlay-opacity": 0.14,
            "underlay-padding": 6,
            "underlay-shape": "ellipse",
            "border-width": 0,
            label: "data(label)",
            "font-size": "10px",
            color: GRAPH.label,
            "text-wrap": "ellipsis",
            "text-max-width": "130px",
            "text-valign": "bottom",
            "text-margin-y": 6,
            width: "data(size)",
            height: "data(size)",
            "transition-property": "background-color, underlay-opacity, opacity",
            "transition-duration": 120,
          },
        },
        {
          selector: "edge",
          style: {
            "curve-style": "haystack",
            "line-color": GRAPH.edge,
            width: 1,
            opacity: 0.9,
            "transition-property": "line-color, opacity",
            "transition-duration": 120,
          },
        },
        {
          selector: 'edge[type="cooccurrence"]',
          style: { width: "data(width)" },
        },
        {
          selector: 'edge[type="semantic"]',
          style: { "line-style": "dashed", opacity: 0.55 },
        },
        // Hover/selection: the neighbourhood darkens into focus, the accent
        // marks the concept itself, and everything else recedes.
        {
          selector: "node.lit",
          style: {
            "background-color": GRAPH.nodeLit,
            "underlay-color": GRAPH.nodeLit,
            "underlay-opacity": 0.14,
            color: GRAPH.labelLit,
          },
        },
        { selector: "edge.lit", style: { "line-color": GRAPH.edgeLit, opacity: 1 } },
        {
          selector: "node:selected, node.hovered",
          style: {
            "background-color": GRAPH.accent,
            "underlay-color": GRAPH.accent,
            "underlay-opacity": 0.28,
            "underlay-padding": 9,
            color: GRAPH.accent,
          },
        },
        { selector: "edge:selected", style: { "line-color": GRAPH.accent, opacity: 1 } },
        { selector: "node.faded", style: { opacity: 0.15, "text-opacity": 0 } },
        { selector: "edge.faded", style: { opacity: 0.06 } },
        // Zoomed far out, labels are noise — only the constellation remains.
        { selector: "node.nolabel", style: { "text-opacity": 0 } },
      ],
      // Positions come from the live d3-force simulation below, not a one-shot
      // layout — that is what makes dragging a node push its neighbours around
      // and the force sliders act in real time.
      layout: { name: "preset" },
      wheelSensitivity: 0.2,
      // Wheel and zoom bar share the same bounds.
      minZoom: ZOOM_MIN,
      maxZoom: ZOOM_MAX,
    });

    applyNodeScale(cy, forcesRef.current.nodeScale);

    cy.on("select", "node", (event) => {
      const id = event.target.id();
      setSelected(graph.data?.nodes.find((node) => node.id === id) ?? null);
      focusNeighborhood(cy, event.target);
    });
    cy.on("unselect", "node", () => {
      setSelected(null);
      clearFocus(cy);
    });

    // Obsidian-style hover: the pointed-at concept and its neighbours glow,
    // the rest fades. On leave, fall back to the selection's focus (if any).
    cy.on("mouseover", "node", (event) => {
      if (typeof event.target.addClass === "function") event.target.addClass("hovered");
      focusNeighborhood(cy, event.target);
    });
    cy.on("mouseout", "node", (event) => {
      if (typeof event.target.removeClass === "function") event.target.removeClass("hovered");
      if (typeof cy.$ !== "function") return;
      const selectedNode = cy.$("node:selected");
      if (selectedNode.length > 0) {
        focusNeighborhood(cy, selectedNode as unknown as Parameters<typeof focusNeighborhood>[1]);
      } else {
        clearFocus(cy);
      }
    });

    // Fade labels out when the whole constellation is in view, and keep the
    // zoom bar in step with wheel/pinch zooming.
    const syncZoom = () => {
      if (typeof cy.zoom !== "function" || typeof cy.nodes !== "function") return;
      const level = cy.zoom();
      cy.nodes().toggleClass("nolabel", level < LABEL_ZOOM);
      setZoom(level);
    };
    cy.on("zoom", syncZoom);
    syncZoom();

    // --- live force simulation (guarded so the jsdom stub skips it) ----------
    let sim: Simulation<SimNode, undefined> | null = null;
    if (typeof cy.getElementById === "function" && typeof cy.batch === "function") {
      const width = container.clientWidth || 800;
      const height = container.clientHeight || 600;

      type ElementDef = { data: { id: string; size?: number; source?: string; target?: string } };
      const nodeEls = (elements as ElementDef[]).filter((el) => el.data.source === undefined);
      const edgeEls = (elements as ElementDef[]).filter((el) => el.data.source !== undefined);

      // Deterministic spiral seed: stable layouts across opens, no jump cuts.
      const simNodes: SimNode[] = nodeEls.map((el, i) => ({
        id: el.data.id,
        size: el.data.size ?? 20,
        x: width / 2 + 30 * Math.sqrt(i + 1) * Math.cos(i * 2.4),
        y: height / 2 + 30 * Math.sqrt(i + 1) * Math.sin(i * 2.4),
      }));
      const byId = new Map(simNodes.map((node) => [node.id, node]));

      sim = forceSimulation(simNodes)
        .force("charge", forceManyBody<SimNode>())
        .force(
          "link",
          forceLink<SimNode, { source: string; target: string }>(
            edgeEls.map((el) => ({ source: el.data.source!, target: el.data.target! })),
          ).id((node) => node.id),
        )
        .force("x", forceX<SimNode>(width / 2))
        .force("y", forceY<SimNode>(height / 2))
        .force(
          "collide",
          // Reads the live multiplier so the 노드 크기 slider also widens the
          // personal space nodes keep from each other.
          forceCollide<SimNode>().radius(
            (node) => (node.size * forcesRef.current.nodeScale) / 2 + 5,
          ),
        )
        .stop();
      applyForces(sim, forcesRef.current);

      const render = () => {
        cy.batch(() => {
          for (const node of simNodes) {
            cy.getElementById(node.id).position({ x: node.x ?? 0, y: node.y ?? 0 });
          }
        });
      };
      renderRef.current = render;

      // Pre-settle a moment so the first visible frame is a constellation, not
      // a random scatter; then either animate the rest or finish instantly.
      sim.tick(40);
      render();
      cy.fit(undefined, 60);

      if (reducedMotion) {
        sim.tick(260);
        render();
        cy.fit(undefined, 60);
      } else {
        sim.on("tick", render);
        sim.restart();

        // Dragging a node pins it under the cursor while the simulation keeps
        // running, so its neighbours are pushed and pulled live.
        cy.on("grab", "node", (event) => {
          const node = byId.get(event.target.id());
          if (!node || !sim) return;
          sim.alphaTarget(0.3).restart();
          const position = event.target.position();
          node.fx = position.x;
          node.fy = position.y;
        });
        cy.on("drag", "node", (event) => {
          const node = byId.get(event.target.id());
          if (!node) return;
          const position = event.target.position();
          node.fx = position.x;
          node.fy = position.y;
        });
        cy.on("free", "node", (event) => {
          const node = byId.get(event.target.id());
          if (!node || !sim) return;
          node.fx = null;
          node.fy = null;
          sim.alphaTarget(0);
        });
      }
      simRef.current = sim;
    }

    cyRef.current = cy;
    return () => {
      sim?.stop();
      simRef.current = null;
      renderRef.current = null;
      cy.destroy();
      cyRef.current = null;
    };
  }, [elements, graph.data, reducedMotion]);

  // Slider changes reshape the live simulation immediately and persist.
  useEffect(() => {
    forcesRef.current = forces;
    try {
      localStorage.setItem(FORCES_KEY, JSON.stringify(forces));
    } catch {
      // Persistence is a convenience; the sliders still work without it.
    }
    if (cyRef.current) applyNodeScale(cyRef.current, forces.nodeScale);
    const sim = simRef.current;
    if (!sim) return;
    applyForces(sim, forces);
    if (reducedMotion) {
      sim.tick(200);
      renderRef.current?.();
    } else {
      sim.alpha(0.5).restart();
    }
  }, [forces, reducedMotion]);

  const empty = graph.isSuccess && graph.data.nodes.length === 0;

  return (
    <GraphLayout
      toggle={toggle}
      containerRef={containerRef}
      onFit={() => cyRef.current?.fit(undefined, 40)}
      zoomBar={
        <div className="absolute bottom-md right-md flex items-center gap-sm rounded-md border border-line bg-canvas px-sm py-1.5 shadow-card">
          <button
            onClick={() => applyZoom(zoom / 1.25)}
            aria-label="축소"
            className="text-ink-body transition-colors duration-fast hover:text-primary"
          >
            <ZoomOut aria-hidden className="h-4 w-4" />
          </button>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={zoomToSlider(zoom)}
            onChange={(event) => applyZoom(sliderToZoom(Number(event.target.value)))}
            aria-label="확대/축소"
            className="h-1.5 w-32 cursor-pointer accent-primary"
          />
          <button
            onClick={() => applyZoom(zoom * 1.25)}
            aria-label="확대"
            className="text-ink-body transition-colors duration-fast hover:text-primary"
          >
            <ZoomIn aria-hidden className="h-4 w-4" />
          </button>
        </div>
      }
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

          <section className="flex flex-col gap-sm">
            <div className="flex items-center justify-between">
              <h2 className="text-caption font-medium text-ink-body">그래프 물리</h2>
              <button
                onClick={() => setForces(DEFAULT_FORCES)}
                className="text-caption text-ink-subhead transition-colors duration-fast hover:text-primary"
              >
                초기화
              </button>
            </div>
            <ForceSlider
              label="밀어내는 힘"
              min={0}
              max={2}
              step={0.05}
              value={forces.repel}
              onChange={(repel) => setForces((current) => ({ ...current, repel }))}
            />
            <ForceSlider
              label="중심 힘"
              min={0}
              max={1}
              step={0.05}
              value={forces.centerForce}
              onChange={(centerForce) => setForces((current) => ({ ...current, centerForce }))}
            />
            <ForceSlider
              label="링크 힘"
              min={0}
              max={1}
              step={0.05}
              value={forces.linkStrength}
              onChange={(linkStrength) => setForces((current) => ({ ...current, linkStrength }))}
            />
            <ForceSlider
              label="링크 거리"
              min={30}
              max={200}
              step={5}
              value={forces.linkDistance}
              onChange={(linkDistance) => setForces((current) => ({ ...current, linkDistance }))}
            />
            <ForceSlider
              label="노드 크기"
              min={0.5}
              max={2}
              step={0.05}
              value={forces.nodeScale}
              onChange={(nodeScale) => setForces((current) => ({ ...current, nodeScale }))}
            />
          </section>

          {selected && (
            <Card className="flex flex-col gap-sm p-md">
              <CardTitle>{selected.label}</CardTitle>
              <Badge>{selected.paperCount}편의 논문</Badge>

              <TagNoteSection label={selected.label} onOpenPaper={onOpenPaper} />

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

/**
 * The concept's second-brain note: what it means in each paper that discussed
 * it, distilled from AI analyses. Refetches whenever the selected topic changes.
 */
function TagNoteSection({
  label,
  onOpenPaper,
}: {
  label: string;
  onOpenPaper: (paperId: string) => void;
}) {
  const note = useQuery({
    queryKey: ["tag-note", label],
    queryFn: () => api.getTagNote(label),
  });

  if (note.isPending) {
    return (
      <div className="flex flex-col gap-xs" aria-busy="true" aria-label="개념 노트 불러오는 중">
        {[0, 1].map((row) => (
          <div key={row} className="h-12 animate-pulse rounded-sm bg-canvas-soft" />
        ))}
      </div>
    );
  }

  if (note.isError) {
    return (
      <p role="alert" className="text-caption text-danger">
        {errorMessage(note.error)} 개념 노트를 불러오지 못했습니다.
      </p>
    );
  }

  const entries = note.data?.entries ?? [];

  return (
    <section className="flex flex-col gap-sm">
      <h3 className="text-caption font-bold text-ink-heading">개념 노트</h3>
      {entries.length === 0 ? (
        <CardDescription>
          아직 이 개념에 대한 노트가 없습니다. 논문을 분석하면 여기에 쌓입니다.
        </CardDescription>
      ) : (
        <ul className="flex flex-col gap-sm">
          {entries.map((entry) => (
            <li key={entry.paperId} className="flex flex-col gap-1">
              <button
                onClick={() => onOpenPaper(entry.paperId)}
                className="w-full rounded-sm text-left text-caption font-medium text-ink hover:text-primary"
              >
                {entry.paperTitle}
              </button>
              <p className="text-caption text-ink-body">{entry.insight}</p>
              {entry.evidencePages.length > 0 && (
                <div className="flex flex-wrap gap-1">
                  {entry.evidencePages.map((page) => (
                    <button
                      key={page}
                      onClick={() => onOpenPaper(entry.paperId)}
                      className="rounded-sm border border-line px-1.5 py-0.5 text-caption text-ink-body transition-colors duration-fast hover:border-primary hover:text-primary"
                    >
                      p.{page}
                    </button>
                  ))}
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// --- shared layout -----------------------------------------------------------

function GraphLayout({
  toggle,
  containerRef,
  onFit,
  sidebar,
  extraToolbar,
  zoomBar,
  empty,
  error,
  loading,
}: {
  toggle: React.ReactNode;
  containerRef: React.RefObject<HTMLDivElement | null>;
  onFit: () => void;
  sidebar: React.ReactNode;
  extraToolbar?: React.ReactNode;
  zoomBar?: React.ReactNode;
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

        {!loading && !error && !empty && zoomBar}
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

/** One Obsidian-style force control: a labelled range slider with live value. */
function ForceSlider({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="flex items-center justify-between text-caption text-ink-body">
        {label}
        <span className="tabular-nums text-ink-subhead">
          {step >= 1 ? Math.round(value) : value.toFixed(2)}
        </span>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
        className="h-1.5 w-full cursor-pointer accent-primary"
        aria-label={label}
      />
    </label>
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
