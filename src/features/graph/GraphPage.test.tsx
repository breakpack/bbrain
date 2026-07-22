import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { Graph, TopicGraph } from "@/lib/types";
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
