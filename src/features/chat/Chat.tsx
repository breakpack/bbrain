import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { MessageSquare, Send, Square, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/Button";
import { cn } from "@/lib/cn";
import { api, errorMessage } from "@/lib/ipc";
import type {
  ChatCompletedEvent,
  ChatDeltaEvent,
  ChatFailedEvent,
  ChatScope,
  Citation,
  StoredMessage,
} from "@/lib/types";

/**
 * Floating chat: 48×48 button 24px from the bottom-right, opening into a
 * ~380×560 panel that expands upward (DEVELOPMENT.md §11.3). Collapsing keeps
 * the session.
 */
export function Chat({
  scope,
  onOpenCitation,
}: {
  scope: ChatScope;
  onOpenCitation?: (paperId: string, pageNumber: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const [question, setQuestion] = useState("");
  const [streaming, setStreaming] = useState<{ requestId: string; text: string } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const client = useQueryClient();
  const bottomRef = useRef<HTMLDivElement>(null);

  const scopeKey = scope.type === "library" ? "library" : `${scope.type}:${scope.id}`;

  // One session per scope, created lazily on first use.
  const session = useQuery({
    queryKey: ["chat-session", scopeKey],
    queryFn: () =>
      api.createChatSession(
        scope,
        scope.type === "paper" ? "논문 대화" : "라이브러리 대화",
      ),
    enabled: open,
    staleTime: Infinity,
    gcTime: Infinity,
  });

  const messages = useQuery({
    queryKey: ["chat-messages", session.data],
    queryFn: () => api.listChatMessages(session.data!),
    enabled: Boolean(session.data),
  });

  const ask = useMutation({
    mutationFn: (input: { requestId: string; question: string }) =>
      api.startChat({
        requestId: input.requestId,
        sessionId: session.data!,
        question: input.question,
        scope,
      }),
  });

  const refresh = useCallback(() => {
    void client.invalidateQueries({ queryKey: ["chat-messages", session.data] });
  }, [client, session.data]);

  useEffect(() => {
    const offs = [
      listen<ChatDeltaEvent>("chat://delta", (event) => {
        setStreaming((current) =>
          current && current.requestId === event.payload.requestId
            ? { ...current, text: current.text + event.payload.delta }
            : current,
        );
      }),
      listen<ChatCompletedEvent>("chat://completed", (event) => {
        setStreaming((current) =>
          current?.requestId === event.payload.requestId ? null : current,
        );
        refresh();
      }),
      listen<ChatFailedEvent>("chat://failed", (event) => {
        setStreaming((current) =>
          current?.requestId === event.payload.requestId ? null : current,
        );
        setError(event.payload.message);
        refresh();
      }),
    ];

    return () => {
      for (const off of offs) void off.then((fn) => fn());
    };
  }, [refresh]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.data, streaming?.text]);

  const send = () => {
    const text = question.trim();
    if (!text || !session.data || streaming) return;

    const requestId = crypto.randomUUID();
    setError(null);
    setQuestion("");
    setStreaming({ requestId, text: "" });
    refresh();

    ask.mutate(
      { requestId, question: text },
      {
        onError: (cause) => {
          setStreaming(null);
          setError(errorMessage(cause));
        },
      },
    );
  };

  const cancel = () => {
    if (!streaming) return;
    void api.cancelChat(streaming.requestId);
    setStreaming(null);
  };

  if (!open) {
    return (
      <button
        aria-label="AI에게 질문하기"
        onClick={() => setOpen(true)}
        className={cn(
          "fixed bottom-[24px] right-[24px] z-30 flex h-12 w-12 items-center justify-center",
          "rounded-control bg-primary text-on-primary shadow-card",
          "transition-colors duration-fast ease-standard hover:bg-primary-hover",
        )}
      >
        <MessageSquare aria-hidden className="h-[22px] w-[22px]" />
      </button>
    );
  }

  return (
    <section
      aria-label="AI 대화"
      className={cn(
        "fixed bottom-[24px] right-[24px] z-30 flex w-[380px] flex-col",
        "h-[560px] max-h-[calc(100vh-48px)] rounded-card border border-line bg-canvas shadow-card",
      )}
    >
      <header className="flex items-center justify-between gap-md border-b border-line px-md py-sm">
        <h2 className="text-caption font-bold text-ink-heading">
          {scope.type === "paper" ? "이 논문에 질문" : "라이브러리 전체에 질문"}
        </h2>
        <button
          aria-label="대화 닫기"
          onClick={() => setOpen(false)}
          className="rounded-sm p-1 text-ink-body hover:text-ink"
        >
          <X aria-hidden className="h-[18px] w-[18px]" />
        </button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-md">
        {(messages.data ?? []).length === 0 && !streaming && (
          <p className="text-caption text-ink-body">
            가져온 논문에서 찾은 근거로만 답합니다. 답변의 출처를 누르면 해당 페이지로
            이동합니다.
          </p>
        )}

        <ul className="flex flex-col gap-md">
          {(messages.data ?? [])
            .filter((message) => message.status !== "streaming")
            .map((message) => (
              <MessageBubble
                key={message.id}
                message={message}
                onOpenCitation={onOpenCitation}
              />
            ))}

          {streaming && (
            <li className="flex flex-col gap-1">
              <span className="text-caption text-ink-body">Bbrain</span>
              <p className="whitespace-pre-wrap rounded-card bg-canvas-soft p-md text-caption text-ink">
                {streaming.text || "근거를 찾는 중…"}
              </p>
            </li>
          )}
        </ul>

        {error && (
          <p role="alert" className="mt-md text-caption text-danger">
            {error}
          </p>
        )}

        <div ref={bottomRef} />
      </div>

      <form
        className="flex items-end gap-sm border-t border-line p-md"
        onSubmit={(event) => {
          event.preventDefault();
          send();
        }}
      >
        <label className="sr-only" htmlFor="chat-question">
          질문
        </label>
        <textarea
          id="chat-question"
          rows={2}
          value={question}
          onChange={(event) => setQuestion(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              send();
            }
          }}
          placeholder="무엇이든 물어보세요"
          className="flex-1 resize-none rounded-control border border-line px-md py-sm text-caption text-ink placeholder:text-ink-body focus:border-primary focus:outline-none"
        />

        {streaming ? (
          <Button type="button" size="sm" variant="outline" onClick={cancel}>
            <Square aria-hidden className="h-4 w-4" />
            중지
          </Button>
        ) : (
          <Button type="submit" size="sm" disabled={!question.trim() || !session.data}>
            <Send aria-hidden className="h-4 w-4" />
            보내기
          </Button>
        )}
      </form>
    </section>
  );
}

function MessageBubble({
  message,
  onOpenCitation,
}: {
  message: StoredMessage;
  onOpenCitation?: (paperId: string, pageNumber: number) => void;
}) {
  const isUser = message.role === "user";

  return (
    <li className={cn("flex flex-col gap-1", isUser && "items-end")}>
      <span className="text-caption text-ink-body">{isUser ? "나" : "Bbrain"}</span>
      <p
        className={cn(
          "max-w-[85%] whitespace-pre-wrap rounded-card p-md text-caption",
          isUser ? "bg-primary-soft text-ink" : "bg-canvas-soft text-ink",
        )}
      >
        {message.content}
      </p>

      {message.citations.length > 0 && (
        <ul className="flex flex-wrap gap-1.5">
          {message.citations.map((citation: Citation) => (
            <li key={citation.chunkId}>
              <button
                onClick={() => onOpenCitation?.(citation.paperId, citation.pageStart)}
                className="rounded-sm border border-line px-2 py-1 text-caption text-ink-body transition-colors duration-fast hover:border-primary hover:text-primary"
              >
                {citation.paperTitle} · {citation.pageStart}쪽
              </button>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}
