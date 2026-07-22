import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { Graph, PaperNeighborhood, TopicGraph } from "@/lib/types";
import { GraphPage } from "./GraphPage";

/**
 * Cytoscape needs a real canvas, which jsdom lacks, so we swap it for a fake that
 * records the event handlers the component registers. Tests then drive selection
 * and double-tap by invoking those handlers, exercising the React contract (the
 * sidebar, filters, and callbacks) without a rendering engine.
 */
const cyHarness = vi.hoisted(() => {
  const state = {
    handlers: [] as Array<{ ev: string; sel: string; cb: (e: unknown) => void }>,
    fit: vi.fn(),
    instances: 0,
    destroyed: 0,
  };
  return state;
});

vi.mock("cytoscape", () => ({
  default: () => {
    cyHarness.instances += 1;
    cyHarness.handlers = [];
    return {
      on: (ev: string, sel: string, cb: (e: unknown) => void) => {
        cyHarness.handlers.push({ ev, sel, cb });
      },
      fit: cyHarness.fit,
      destroy: () => {
        cyHarness.destroyed += 1;
      },
    };
  },
}));

function fire(ev: string, sel: string, nodeId: string) {
  const handler = cyHarness.handlers.find((h) => h.ev === ev && h.sel === sel);
  if (!handler) throw new Error(`no ${ev}/${sel} handler registered`);
  act(() => handler.cb({ target: { id: () => nodeId } }));
}

const TOPIC_GRAPH: TopicGraph = {
  nodes: [
    {
      id: "t1",
      label: "Transformer",
      paperCount: 2,
      papers: [
        { id: "p1", title: "Attention Is All You Need" },
        { id: "p2", title: "BERT" },
      ],
    },
    { id: "t2", label: "Retrieval", paperCount: 1, papers: [{ id: "p3", title: "DPR" }] },
  ],
  edges: [
    { source: "t1", target: "t2", edgeType: "cooccurrence", weight: 2 },
    { source: "t1", target: "t2", edgeType: "semantic", weight: 0.82 },
  ],
};

const PAPER_GRAPH: Graph = {
  nodes: [
    { id: "p1", title: "Attention Is All You Need", year: 2017, groupIds: ["g1"], tagIds: [] },
    { id: "p2", title: "BERT", year: 2018, groupIds: [], tagIds: [] },
  ],
  edges: [
    { sourcePaperId: "p1", targetPaperId: "p2", relationType: "semantic", score: 0.8 },
  ],
};

const NEIGHBORHOOD: PaperNeighborhood = {
  centerId: "p1",
  nodes: [
    {
      id: "p1",
      title: "Attention Is All You Need",
      year: 2017,
      similarity: null,
      lineage: "focus",
      citesFocus: false,
    },
    { id: "p2", title: "BERT", year: 2018, similarity: 0.88, lineage: "derivative", citesFocus: false },
    { id: "p0", title: "Seq2Seq", year: 2014, similarity: 0.79, lineage: "precedent", citesFocus: true },
  ],
  edges: [
    { source: "p1", target: "p2", edgeType: "similarity", weight: 0.88 },
    { source: "p1", target: "p0", edgeType: "similarity", weight: 0.79 },
    { source: "p1", target: "p0", edgeType: "citation", weight: 1 },
  ],
};

beforeEach(() => {
  invokeMock.mockReset();
  cyHarness.handlers = [];
  cyHarness.fit.mockReset();
  cyHarness.instances = 0;
  cyHarness.destroyed = 0;
});

const noop = () => undefined;

describe("graph page — topic view (default)", () => {
  it("disables the fit control while the graph is still loading", () => {
    mockCommands({ get_topic_graph: () => new Promise<never>(() => {}) });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    expect(screen.getByRole("button", { name: "전체 보기" })).toBeDisabled();
  });

  it("explains the empty state when no topics exist yet", async () => {
    mockCommands({ get_topic_graph: () => ({ nodes: [], edges: [] }) });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    expect(await screen.findByText("아직 토픽이 없습니다")).toBeInTheDocument();
  });

  it("shows a selected topic's papers and opens one on click", async () => {
    const opened: string[] = [];
    mockCommands({ get_topic_graph: () => TOPIC_GRAPH });

    renderWithQuery(<GraphPage onOpenPaper={(id) => opened.push(id)} />);

    // Wait for the graph to mount before driving the fake selection.
    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("select", "node", "t1");

    expect(await screen.findByRole("heading", { name: "Transformer" })).toBeInTheDocument();
    expect(screen.getByText("2편의 논문")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Attention Is All You Need" }));
    expect(opened).toEqual(["p1"]);
  });

  it("forces a rebuild when the user presses 재구성", async () => {
    const rebuildCalls: boolean[] = [];
    mockCommands({
      get_topic_graph: (args: { rebuild: boolean }) => {
        rebuildCalls.push(args.rebuild);
        return TOPIC_GRAPH;
      },
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    await waitFor(() => expect(rebuildCalls).toContain(false));
    await userEvent.click(screen.getByRole("button", { name: "재구성" }));

    await waitFor(() => expect(rebuildCalls).toContain(true));
  });

  it("shows a calm loading line while the graph is being prepared", () => {
    mockCommands({ get_topic_graph: () => new Promise<never>(() => {}) });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    expect(screen.getByText("그래프를 준비하는 중이에요")).toBeInTheDocument();
  });

  it("surfaces a load failure with a retry that refetches", async () => {
    let attempt = 0;
    mockCommands({
      get_topic_graph: () => {
        attempt += 1;
        if (attempt === 1) throw { code: "internal", message: "임베딩 모델을 불러오지 못했습니다." };
        return TOPIC_GRAPH;
      },
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("임베딩 모델을 불러오지 못했습니다.");

    await userEvent.click(screen.getByRole("button", { name: "다시 시도" }));

    // After a successful retry the graph mounts and the error clears.
    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
  });

  it("keeps the graph and shows an inline alert when a rebuild fails", async () => {
    let calls = 0;
    mockCommands({
      get_topic_graph: (args: { rebuild: boolean }) => {
        calls += 1;
        if (args.rebuild) throw { code: "internal", message: "재구성에 실패했습니다." };
        return TOPIC_GRAPH;
      },
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    await userEvent.click(screen.getByRole("button", { name: "재구성" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("재구성에 실패했습니다.");
    // The graph itself stays mounted — a failed rebuild does not wipe the canvas.
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
    expect(calls).toBeGreaterThanOrEqual(2);
  });

  it("toggles an edge-type filter via its aria-pressed state", async () => {
    mockCommands({ get_topic_graph: () => TOPIC_GRAPH });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    const chip = await screen.findByRole("button", { name: "함께 등장" });
    expect(chip).toHaveAttribute("aria-pressed", "true");

    await userEvent.click(chip);
    expect(chip).toHaveAttribute("aria-pressed", "false");
  });
});

describe("graph page — paper view", () => {
  async function switchToPaper() {
    await userEvent.click(screen.getByRole("button", { name: "논문" }));
  }

  it("switches to the paper graph from the toggle", async () => {
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => PAPER_GRAPH,
      list_groups: () => [{ id: "g1", name: "읽는 중" }],
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);
    await switchToPaper();

    // The paper-only "인용" filter proves we are on the paper view.
    expect(await screen.findByRole("button", { name: "인용" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "읽는 중" })).toBeInTheDocument();
  });

  it("explains the empty state when no papers are in the graph", async () => {
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => ({ nodes: [], edges: [] }),
      list_groups: () => [],
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);
    await switchToPaper();

    expect(
      await screen.findByText("아직 그래프에 표시할 논문이 없습니다"),
    ).toBeInTheDocument();
  });

  it("opens a paper on double-tap and from the selection card", async () => {
    const opened: string[] = [];
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => PAPER_GRAPH,
      list_groups: () => [],
    });

    renderWithQuery(<GraphPage onOpenPaper={(id) => opened.push(id)} />);
    await switchToPaper();

    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("dbltap", "node", "p1");
    expect(opened).toEqual(["p1"]);

    // Single selection reveals a card with an explicit "열기" button.
    fire("select", "node", "p2");
    await userEvent.click(await screen.findByRole("button", { name: "논문 열기" }));
    expect(opened).toEqual(["p1", "p2"]);
  });
});

describe("graph page — paper neighborhood (ConnectedPapers)", () => {
  async function enterNeighborhood(onOpenPaper: (id: string) => void = noop) {
    renderWithQuery(<GraphPage onOpenPaper={onOpenPaper} />);
    await userEvent.click(screen.getByRole("button", { name: "논문" }));
    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("select", "node", "p1");
    await userEvent.click(await screen.findByRole("button", { name: "이웃 보기" }));
  }

  it("opens a focus graph and lets a neighbour be opened or re-centred", async () => {
    const opened: string[] = [];
    const focusRequests: string[] = [];
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => PAPER_GRAPH,
      list_groups: () => [],
      get_paper_neighborhood: (args: { paperId: string }) => {
        focusRequests.push(args.paperId);
        return NEIGHBORHOOD;
      },
    });

    await enterNeighborhood((id) => opened.push(id));

    // The centre paper names the view; the year-axis hint orients the reader.
    expect(await screen.findByRole("heading", { name: "Attention Is All You Need" })).toBeInTheDocument();
    expect(focusRequests).toEqual(["p1"]);

    // Selecting a neighbour surfaces its lineage and similarity.
    await waitFor(() => expect(cyHarness.handlers.length).toBeGreaterThan(0));
    fire("select", "node", "p0");
    expect(await screen.findByText("선행 연구")).toBeInTheDocument();
    expect(screen.getByText("유사도 79%")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "논문 열기" }));
    expect(opened).toEqual(["p0"]);

    // Re-centring refetches the neighbourhood for the chosen neighbour.
    fire("select", "node", "p0");
    await userEvent.click(screen.getByRole("button", { name: "중심으로" }));
    await waitFor(() => expect(focusRequests).toContain("p0"));
  });

  it("explains the empty state when the focus paper has no neighbours yet", async () => {
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => PAPER_GRAPH,
      list_groups: () => [],
      get_paper_neighborhood: () => ({
        centerId: "p1",
        nodes: [NEIGHBORHOOD.nodes[0]],
        edges: [],
      }),
    });

    await enterNeighborhood();

    expect(await screen.findByText("아직 이어질 이웃이 없어요")).toBeInTheDocument();
  });

  it("returns to the graph from the back control", async () => {
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_graph: () => PAPER_GRAPH,
      list_groups: () => [],
      get_paper_neighborhood: () => NEIGHBORHOOD,
    });

    await enterNeighborhood();
    await userEvent.click(await screen.findByRole("button", { name: "그래프로" }));

    // Back on the paper view, its toggle is available again.
    expect(await screen.findByRole("button", { name: "인용" })).toBeInTheDocument();
  });
});
