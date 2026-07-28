import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { DiscoveredPaper } from "@/lib/types";
import { DiscoverPage } from "./DiscoverPage";

function hit(overrides: Partial<DiscoveredPaper> = {}): DiscoveredPaper {
  return {
    id: "semantic-scholar:abc",
    title: "Attention Is All You Need",
    authors: ["Vaswani", "Shazeer", "Parmar"],
    year: 2017,
    venue: "NeurIPS",
    abstract: "The dominant sequence transduction models…",
    pdfUrl: "https://arxiv.org/pdf/1706.03762.pdf",
    url: "https://www.semanticscholar.org/paper/abc",
    doi: "10.48550/arXiv.1706.03762",
    citationCount: 100000,
    alreadyInLibrary: false,
    localPaperId: null,
    ...overrides,
  };
}

async function submitSearch(term = "attention") {
  await userEvent.type(screen.getByRole("searchbox", { name: "주제 또는 키워드" }), term);
  await userEvent.click(screen.getByRole("button", { name: "검색" }));
}

describe("discover", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("prompts for a topic before any search", () => {
    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);

    expect(screen.getByText(/찾고 싶은 논문의 주제를 입력하세요/)).toBeInTheDocument();
    // The search button is disabled until something is typed.
    expect(screen.getByRole("button", { name: "검색" })).toBeDisabled();
  });

  it("shows results returned by the backend search", async () => {
    mockCommands({
      search_papers: () => ({ hits: [hit()], total: 1, nextOffset: null }),
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await submitSearch();

    expect(await screen.findByText("Attention Is All You Need")).toBeInTheDocument();
    expect(screen.getByText(/약 1건 중 1건 표시/)).toBeInTheDocument();
  });

  it("passes the query and open-access filter to the backend", async () => {
    let received: unknown = null;
    mockCommands({
      search_papers: (args) => {
        received = args;
        return { hits: [], total: 0, nextOffset: null };
      },
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await userEvent.click(screen.getByRole("checkbox", { name: /무료로 가져올 수 있는 논문만/ }));
    await submitSearch("확산 모델");

    expect(received).toMatchObject({
      query: { query: "확산 모델", offset: 0, openAccessOnly: true },
    });
  });

  it("tells the user when nothing matches", async () => {
    mockCommands({
      search_papers: () => ({ hits: [], total: 0, nextOffset: null }),
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await submitSearch("qzxqzx");

    expect(await screen.findByText(/결과가 없습니다/)).toBeInTheDocument();
  });

  it("surfaces a backend error without wiping the prompt", async () => {
    mockCommands({
      search_papers: () => {
        throw { code: "network", message: "네트워크에 연결할 수 없습니다." };
      },
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await submitSearch();

    expect(await screen.findByRole("alert")).toHaveTextContent("네트워크에 연결할 수 없습니다.");
  });

  it("imports an open-access paper and offers to open it", async () => {
    let imported: unknown = null;
    mockCommands({
      search_papers: () => ({ hits: [hit()], total: 1, nextOffset: null }),
      import_discovered_paper: (args) => {
        imported = args;
        return { outcome: "imported", paperId: "p-new", title: hit().title };
      },
    });

    const opened: string[] = [];
    renderWithQuery(<DiscoverPage onOpenPaper={(id) => opened.push(id)} />);
    await submitSearch();

    await userEvent.click(await screen.findByRole("button", { name: "라이브러리에 가져오기" }));
    expect(imported).toMatchObject({ paperId: "semantic-scholar:abc" });

    await userEvent.click(await screen.findByRole("button", { name: "읽기" }));
    expect(opened).toEqual(["p-new"]);
  });

  it("cannot import a paper without a downloadable PDF", async () => {
    mockCommands({
      search_papers: () => ({ hits: [hit({ pdfUrl: null })], total: 1, nextOffset: null }),
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await submitSearch();

    expect(await screen.findByText(/무료 PDF가 없어 가져올 수 없습니다/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "라이브러리에 가져오기" })).not.toBeInTheDocument();
  });

  it("marks a paper already in the library instead of importing it again", async () => {
    mockCommands({
      search_papers: () => ({
        hits: [hit({ alreadyInLibrary: true, localPaperId: "p-existing" })],
        total: 1,
        nextOffset: null,
      }),
    });

    const opened: string[] = [];
    renderWithQuery(<DiscoverPage onOpenPaper={(id) => opened.push(id)} />);
    await submitSearch();

    await userEvent.click(await screen.findByRole("button", { name: "읽기" }));
    expect(opened).toEqual(["p-existing"]);
  });

  it("loads the next page and appends to the list", async () => {
    const calls: number[] = [];
    mockCommands({
      search_papers: ({ query }) => {
        calls.push(query.offset);
        return query.offset === 0
          ? { hits: [hit({ id: "a", title: "First paper" })], total: 2, nextOffset: 20 }
          : { hits: [hit({ id: "b", title: "Second paper" })], total: 2, nextOffset: null };
      },
    });

    renderWithQuery(<DiscoverPage onOpenPaper={() => {}} />);
    await submitSearch();

    await screen.findByText("First paper");
    await userEvent.click(screen.getByRole("button", { name: "더 보기" }));

    const list = await screen.findByRole("list");
    expect(within(list).getByText("First paper")).toBeInTheDocument();
    expect(within(list).getByText("Second paper")).toBeInTheDocument();
    expect(calls).toEqual([0, 20]);
    expect(screen.queryByRole("button", { name: "더 보기" })).not.toBeInTheDocument();
  });
});
