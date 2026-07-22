import { vi } from "vitest";

import type { Settings } from "@/lib/types";

/**
 * Mock Tauri bridge: tests exercise the real components against a fake command
 * layer, so the UI contract stays honest without launching the Rust core.
 */
export type CommandHandlers = Record<string, (args: any) => unknown>;

export const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => invokeMock(command, args),
}));

// A shared in-test event bus so a test can drive backend events (chat://delta,
// library://changed, …) itself. Tests that don't care simply never call emit.
type EventHandler = (event: { payload: unknown }) => void;
const eventListeners = new Map<string, Set<EventHandler>>();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: EventHandler) => {
    const set = eventListeners.get(name) ?? new Set();
    set.add(handler);
    eventListeners.set(name, set);
    return Promise.resolve(() => set.delete(handler));
  },
  emit: () => Promise.resolve(),
}));

/** Fires a backend event to every component listening for it. */
export function emitEvent(name: string, payload: unknown): void {
  for (const handler of eventListeners.get(name) ?? []) {
    handler({ payload });
  }
}

export function resetEvents(): void {
  eventListeners.clear();
}

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: () => Promise.resolve(null),
}));

// OS file drops arrive through the window, which only exists inside the webview.
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    onDragDropEvent: () => Promise.resolve(() => undefined),
  }),
}));

export function mockCommands(handlers: CommandHandlers) {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (command: string, args: any) => {
    const handler = handlers[command];
    if (!handler) throw new Error(`unmocked command: ${command}`);
    return handler(args ?? {});
  });
}

export function settingsFixture(overrides: Partial<Settings> = {}): Settings {
  return {
    language: "ko",
    activeProvider: null,
    openaiModel: null,
    anthropicModel: null,
    hasOpenaiKey: false,
    hasAnthropicKey: false,
    translationLanguage: "ko",
    obsidianVaultPath: null,
    embeddingModelId: "intfloat/multilingual-e5-small",
    embeddingDimension: 384,
    indexGeneration: 1,
    networkNoticeAcceptedAt: null,
    onboardingCompletedAt: null,
    ...overrides,
  };
}
