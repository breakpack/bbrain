import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { ImportStatus, Paper } from "@/lib/types";
import { LibraryPage } from "./LibraryPage";

function paper(overrides: Partial<Paper> = {}): Paper {
  return {
    id: "p1",
    sha256: "hash",
    title: "Attention Is All You Need",
    importStatus: "ready" as ImportStatus,
    pageCount: 11,
    isFavorite: false,
    lastOpenedAt: null,
    createdAt: "2026-07-14T00:00:00Z",
    updatedAt: "2026-07-14T00:00:00Z",
    authors: ["Vaswani", "Shazeer", "Parmar"],
    year: 2017,
    venue: null,
    doi: null,
    abstractText: null,
    groupIds: [],
    tags: [{ id: "t1", displayName: "transformer", source: "ai" }],
    ...overrides,
  };
}

const BASE = {
  list_groups: () => [],
  list_tags: () => [],
};

describe("library", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    localStorage.clear();
  });

  it("tells the user how to import when the library is empty", async () => {
    mockCommands({ ...BASE, list_papers: () => [] });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);

    expect(await screen.findByText(/아직 가져온 논문이 없습니다/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /PDF 가져오기/ }).length).toBeGreaterThan(0);
  });

  it("shows processing state with an icon and words, not color alone", async () => {
    mockCommands({
      ...BASE,
      list_papers: () => [paper({ importStatus: "analyzing" })],
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);

    expect(await screen.findByText("분석 중")).toBeInTheDocument();
  });

  it("opens a paper when its title is activated", async () => {
    const opened: string[] = [];
    mockCommands({ ...BASE, list_papers: () => [paper()] });

    renderWithQuery(<LibraryPage onOpenPaper={(id) => opened.push(id)} />);
    await userEvent.click(await screen.findByRole("button", { name: paper().title }));

    expect(opened).toEqual(["p1"]);
  });

  it("switches between list and icon views and remembers the choice", async () => {
    mockCommands({ ...BASE, list_papers: () => [paper()] });

    const { unmount } = renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "아이콘 보기" }));

    expect(localStorage.getItem("bbrain.library.layout")).toBe("icon");

    unmount();
    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);

    expect(
      await screen.findByRole("button", { name: "아이콘 보기", pressed: true }),
    ).toBeInTheDocument();
  });

  it("passes the selected system view to the query", async () => {
    const queries: unknown[] = [];
    mockCommands({
      ...BASE,
      list_papers: ({ query }) => {
        queries.push(query);
        return [];
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "즐겨찾기" }));

    await screen.findByText(/즐겨찾기한 논문이 없습니다/);
    expect(queries.at(-1)).toMatchObject({ view: "favorites" });
  });

  it("marks a paper as a favourite through the backend", async () => {
    let patched: unknown = null;
    mockCommands({
      ...BASE,
      list_papers: () => [paper()],
      update_paper: (args) => {
        patched = args;
        return paper({ isFavorite: true });
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "즐겨찾기에 추가" }));

    expect(patched).toMatchObject({ paperId: "p1", patch: { isFavorite: true } });
  });

  it("renames a paper inline and saves on Enter", async () => {
    let patched: unknown = null;
    mockCommands({
      ...BASE,
      list_papers: () => [paper()],
      update_paper: (args) => {
        patched = args;
        return paper({ title: "내가 정한 제목" });
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(
      await screen.findByRole("button", { name: "Attention Is All You Need 이름 바꾸기" }),
    );

    const field = screen.getByLabelText("논문 제목 편집");
    await userEvent.clear(field);
    await userEvent.type(field, "내가 정한 제목{Enter}");

    expect(patched).toMatchObject({ paperId: "p1", patch: { title: "내가 정한 제목" } });
  });

  it("cancels an inline rename with Escape without touching the backend", async () => {
    let patched: unknown = null;
    mockCommands({
      ...BASE,
      list_papers: () => [paper()],
      update_paper: (args) => {
        patched = args;
        return paper();
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(
      await screen.findByRole("button", { name: "Attention Is All You Need 이름 바꾸기" }),
    );
    await userEvent.type(screen.getByLabelText("논문 제목 편집"), " 수정{Escape}");

    expect(screen.queryByLabelText("논문 제목 편집")).toBeNull();
    expect(patched).toBeNull();
    expect(screen.getByText("Attention Is All You Need")).toBeInTheDocument();
  });

  it("files a paper into a group from the row's group menu", async () => {
    let patched: unknown = null;
    mockCommands({
      ...BASE,
      list_groups: () => [
        { id: "g1", name: "통계 교육", color: null, sortOrder: 0, paperCount: 0 },
      ],
      list_papers: () => [paper()],
      update_paper: (args) => {
        patched = args;
        return paper({ groupIds: ["g1"] });
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(
      await screen.findByRole("button", { name: /그룹에 추가/ }),
    );
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: "통계 교육" }));

    expect(patched).toMatchObject({ paperId: "p1", patch: { groupIds: ["g1"] } });
  });

  it("removes a paper from a group it already belongs to via the same menu", async () => {
    let patched: unknown = null;
    mockCommands({
      ...BASE,
      list_groups: () => [
        { id: "g1", name: "통계 교육", color: null, sortOrder: 0, paperCount: 1 },
      ],
      list_papers: () => [paper({ groupIds: ["g1"] })],
      update_paper: (args) => {
        patched = args;
        return paper({ groupIds: [] });
      },
    });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(
      await screen.findByRole("button", { name: /그룹에 추가/ }),
    );

    const item = await screen.findByRole("menuitemcheckbox", { name: "통계 교육" });
    expect(item).toHaveAttribute("aria-checked", "true");
    await userEvent.click(item);

    expect(patched).toMatchObject({ paperId: "p1", patch: { groupIds: [] } });
  });

  it("explains how to create a group when the menu is opened with none", async () => {
    mockCommands({ ...BASE, list_papers: () => [paper()] });

    renderWithQuery(<LibraryPage onOpenPaper={() => {}} />);
    await userEvent.click(
      await screen.findByRole("button", { name: /그룹에 추가/ }),
    );

    expect(await screen.findByText(/그룹이 없습니다/)).toBeInTheDocument();
  });
});
