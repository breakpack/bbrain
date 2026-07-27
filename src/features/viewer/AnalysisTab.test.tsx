import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { emitEvent, invokeMock, mockCommands, resetEvents } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import { AnalysisTab } from "./AnalysisTab";

function storedAnalysis(summary: string) {
  return {
    analysis: {
      shortSummary: summary,
      detailedSummary: "",
      researchProblem: "문제",
      contributions: [],
      methodology: "방법",
      results: [],
      limitations: [],
      keywords: [],
      suggestedTags: [],
      tagInsights: [],
      followUpQuestions: [],
    },
    markdown: "",
    provider: "anthropic",
    model: "claude-sonnet-5",
    createdAt: "2026-07-27T00:00:00Z",
  };
}

const progress = (status: string) => ({
  jobId: "j1",
  paperId: "p1",
  jobType: "analyze",
  status,
  errorCode: null,
  pending: 0,
});

describe("re-analysis loading state", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetEvents();
  });

  it("shows the loading state from the button press until the job completes", async () => {
    let summary = "이전 분석";
    mockCommands({
      get_analysis: () => storedAnalysis(summary),
      reanalyze_paper: () => undefined,
    });

    renderWithQuery(<AnalysisTab paperId="p1" onJump={() => {}} />);
    await screen.findByText("이전 분석");

    // The IPC call resolves immediately — the loading state must persist
    // anyway, because the actual analysis runs as a background job.
    await userEvent.click(screen.getByRole("button", { name: /다시 분석/ }));
    await screen.findByLabelText("분석 중");
    expect(screen.queryByText("이전 분석")).toBeNull();

    // Progress for another paper or another job type must not end the wait.
    act(() => {
      emitEvent("job://progress", { ...progress("completed"), paperId: "other" });
      emitEvent("job://progress", { ...progress("running"), jobType: "embed" });
    });
    expect(screen.getByLabelText("분석 중")).toBeInTheDocument();

    summary = "새 분석";
    act(() => emitEvent("job://progress", progress("completed")));

    await screen.findByText("새 분석");
    expect(screen.queryByLabelText("분석 중")).toBeNull();
  });

  it("reports a failed job in Korean instead of loading forever", async () => {
    mockCommands({
      get_analysis: () => storedAnalysis("이전 분석"),
      reanalyze_paper: () => undefined,
    });

    renderWithQuery(<AnalysisTab paperId="p1" onJump={() => {}} />);
    await screen.findByText("이전 분석");

    await userEvent.click(screen.getByRole("button", { name: /다시 분석/ }));
    await screen.findByLabelText("분석 중");

    act(() => emitEvent("job://progress", progress("failed")));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("분석에 실패했습니다");
    });
    // The previous analysis comes back — it is still the latest good one.
    expect(screen.getByText("이전 분석")).toBeInTheDocument();
  });
});
