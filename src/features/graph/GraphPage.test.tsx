import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { TagNote, TopicGraph } from "@/lib/types";
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

  it("exposes adjustable force sliders and persists their values", async () => {
    localStorage.removeItem("bbrain.graph.forces");
    mockCommands({ get_topic_graph: () => TOPIC_GRAPH, get_tag_note: () => null });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);
    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));

    // The four Obsidian-style physics controls are present.
    const repel = await screen.findByRole("slider", { name: "\ubc00\uc5b4\ub0b4\ub294 \ud798" });
    screen.getByRole("slider", { name: "\uc911\uc2ec \ud798" });
    screen.getByRole("slider", { name: "\ub9c1\ud06c \ud798" });
    screen.getByRole("slider", { name: "\ub9c1\ud06c \uac70\ub9ac" });

    // Moving one changes the stored settings, so the feel survives a restart.
    fireEvent.change(repel, { target: { value: "1.6" } });
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem("bbrain.graph.forces") ?? "{}").repel).toBe(1.6);
    });
  });

  it("shows a selected topic's papers and opens one on click", async () => {
    const opened: string[] = [];
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_tag_note: () => null,
    });

    renderWithQuery(<GraphPage onOpenPaper={(id) => opened.push(id)} />);

    // Wait for the graph to mount before driving the fake selection.
    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("select", "node", "t1");

    expect(await screen.findByRole("heading", { name: "Transformer" })).toBeInTheDocument();
    expect(screen.getByText("2편의 논문")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Attention Is All You Need" }));
    expect(opened).toEqual(["p1"]);
  });

  it("renders a selected topic's concept note with its evidence pages, and jumps to the paper on click", async () => {
    const opened: string[] = [];
    const note: TagNote = {
      label: "Transformer",
      entries: [
        {
          paperId: "p1",
          paperTitle: "Attention Is All You Need",
          insight: "셀프 어텐션으로 순차 연산 없이 문맥을 인코딩한다.",
          evidencePages: [3, 5],
          updatedAt: "2026-07-01T00:00:00Z",
        },
      ],
    };
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_tag_note: () => note,
    });

    renderWithQuery(<GraphPage onOpenPaper={(id) => opened.push(id)} />);

    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("select", "node", "t1");

    expect(
      await screen.findByText("셀프 어텐션으로 순차 연산 없이 문맥을 인코딩한다."),
    ).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "p.3" }));
    expect(opened).toEqual(["p1"]);
  });

  it("explains that a concept has no note yet when entries are empty", async () => {
    mockCommands({
      get_topic_graph: () => TOPIC_GRAPH,
      get_tag_note: () => ({ label: "Transformer", entries: [] }),
    });

    renderWithQuery(<GraphPage onOpenPaper={noop} />);

    await waitFor(() => expect(cyHarness.instances).toBeGreaterThan(0));
    fire("select", "node", "t1");

    expect(
      await screen.findByText(
        "아직 이 개념에 대한 노트가 없습니다. 논문을 분석하면 여기에 쌓입니다.",
      ),
    ).toBeInTheDocument();
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
