import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emitEvent, invokeMock, mockCommands, resetEvents } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { ChatFailedEvent } from "@/lib/types";
import { Chat } from "./Chat";

function emit(name: string, payload: unknown) {
  act(() => emitEvent(name, payload));
}

beforeEach(() => {
  invokeMock.mockReset();
  resetEvents();
  vi.stubGlobal("crypto", { randomUUID: () => "req-1" });
});

describe("floating chat", () => {
  it("opens from the launcher and starts a session", async () => {
    mockCommands({
      create_chat_session: () => "session-1",
      list_chat_messages: () => [],
    });

    renderWithQuery(<Chat scope={{ type: "library" }} />);
    await userEvent.click(screen.getByRole("button", { name: "AI에게 질문하기" }));

    expect(await screen.findByRole("heading", { name: /질문/ })).toBeInTheDocument();
  });

  it("keeps an open paper chat scoped to that paper", async () => {
    let created: unknown = null;
    let started: unknown = null;
    mockCommands({
      create_chat_session: (args) => {
        created = args;
        return "session-paper";
      },
      list_chat_messages: () => [],
      start_chat: (args) => {
        started = args;
        return null;
      },
    });

    renderWithQuery(<Chat scope={{ type: "paper", id: "p42" }} />);
    await userEvent.click(screen.getByRole("button", { name: "AI에게 질문하기" }));
    await userEvent.type(await screen.findByLabelText("질문"), "이 논문의 결론은?");
    const send = await screen.findByRole("button", { name: "보내기" });
    await waitFor(() => expect(send).not.toBeDisabled());
    await userEvent.click(send);

    expect(created).toMatchObject({
      input: { scope: { type: "paper", id: "p42" } },
    });
    expect(started).toMatchObject({
      request: { scope: { type: "paper", id: "p42" } },
    });
  });

  it("streams deltas into the panel and clears them on completion", async () => {
    mockCommands({
      create_chat_session: () => "session-1",
      list_chat_messages: () => [
        {
          id: "m1",
          role: "assistant",
          content: "BLEU 28.4입니다 [S1].",
          status: "complete",
          createdAt: "2026-07-14T00:00:00Z",
          citations: [
            {
              chunkId: "c1",
              paperId: "p1",
              paperTitle: "Attention Is All You Need",
              pageStart: 2,
              pageEnd: 2,
            },
          ],
        },
      ],
      start_chat: () => null,
    });

    renderWithQuery(<Chat scope={{ type: "library" }} />);
    await userEvent.click(screen.getByRole("button", { name: "AI에게 질문하기" }));
    const send = await screen.findByRole("button", { name: "보내기" });
    await userEvent.type(await screen.findByLabelText("질문"), "BLEU?");
    await waitFor(() => expect(send).not.toBeDisabled());
    await userEvent.click(send);

    emit("chat://delta", { requestId: "req-1", messageId: "a1", delta: "답변 생성 중 조각" });
    expect(await screen.findByText(/답변 생성 중 조각/)).toBeInTheDocument();

    emit("chat://completed", { requestId: "req-1", messageId: "a1", content: "", citations: [] });

    // The persisted message (refetched) carries the citation chip.
    expect(
      await screen.findByRole("button", { name: /Attention Is All You Need · 2쪽/ }),
    ).toBeInTheDocument();
  });

  it("shows a failure message and re-enables sending", async () => {
    mockCommands({
      create_chat_session: () => "session-1",
      list_chat_messages: () => [],
      start_chat: () => null,
    });

    renderWithQuery(<Chat scope={{ type: "library" }} />);
    await userEvent.click(screen.getByRole("button", { name: "AI에게 질문하기" }));

    // The send button is disabled until the session exists; wait for it.
    const send = await screen.findByRole("button", { name: "보내기" });
    await userEvent.type(await screen.findByLabelText("질문"), "BLEU?");
    await waitFor(() => expect(send).not.toBeDisabled());
    await userEvent.click(send);

    const failure: ChatFailedEvent = {
      requestId: "req-1",
      messageId: "a1",
      message: "네트워크에 연결하지 못했습니다.",
    };
    emit("chat://failed", failure);

    expect(await screen.findByRole("alert")).toHaveTextContent(/네트워크에 연결하지 못했습니다/);
    // The stop button reverts to send, so the user can retry.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "보내기" })).toBeInTheDocument(),
    );
  });

  it("opens a citation at its paper and page", async () => {
    const opened: Array<[string, number]> = [];
    mockCommands({
      create_chat_session: () => "session-1",
      list_chat_messages: () => [
        {
          id: "m1",
          role: "assistant",
          content: "답변 [S1].",
          status: "complete",
          createdAt: "2026-07-14T00:00:00Z",
          citations: [
            {
              chunkId: "c1",
              paperId: "p9",
              paperTitle: "Some Paper",
              pageStart: 5,
              pageEnd: 5,
            },
          ],
        },
      ],
    });

    renderWithQuery(
      <Chat
        scope={{ type: "library" }}
        onOpenCitation={(paperId, page) => opened.push([paperId, page])}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "AI에게 질문하기" }));
    await userEvent.click(await screen.findByRole("button", { name: /Some Paper · 5쪽/ }));

    expect(opened).toEqual([["p9", 5]]);
  });
});
