import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invokeMock, mockCommands, settingsFixture } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import { SettingsPage } from "./SettingsPage";

// Sections that always render on the page but are not what a given test is about.
const BASE = {
  list_blocked_jobs: () => [],
  list_sync_records: () => [],
};

const CONNECTED = settingsFixture({
  onboardingCompletedAt: "2026-07-14T00:00:00Z",
  networkNoticeAcceptedAt: "2026-07-14T00:00:00Z",
  hasAnthropicKey: true,
  anthropicModel: "claude-sonnet-5",
  activeProvider: "anthropic",
});

describe("settings", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("names the provider and model that paper text is sent to", async () => {
    mockCommands({
      ...BASE,
      list_provider_models: () => [
        { id: "claude-sonnet-5", displayName: "Claude Sonnet 5" },
      ],
    });

    renderWithQuery(<SettingsPage settings={CONNECTED} />);

    expect(
      await screen.findByText(/Anthropic의 claude-sonnet-5 모델로/),
    ).toBeInTheDocument();
  });

  it("asks for re-selection when the stored model is no longer offered", async () => {
    mockCommands({
      ...BASE,
      list_provider_models: () => [{ id: "claude-opus-4-8", displayName: "Claude Opus 4.8" }],
    });

    renderWithQuery(<SettingsPage settings={CONNECTED} />);

    expect(
      await screen.findByText(/저장된 모델을 더 이상 사용할 수 없습니다/),
    ).toBeInTheDocument();
  });

  it("surfaces an auth failure against the stored key", async () => {
    mockCommands({
      ...BASE,
      list_provider_models: () => {
        throw {
          code: "provider_auth",
          message: "API 키가 올바르지 않거나 권한이 없습니다.",
          retryAfterSeconds: null,
        };
      },
    });

    renderWithQuery(<SettingsPage settings={CONNECTED} />);

    expect(await screen.findByText(/키를 다시 확인한 뒤 재시도하세요/)).toBeInTheDocument();
  });

  it("removes a key through the credential store command", async () => {
    const removeProvider = vi.fn(() => null);
    mockCommands({
      ...BASE,
      list_provider_models: () => [
        { id: "claude-sonnet-5", displayName: "Claude Sonnet 5" },
      ],
      remove_provider: removeProvider,
    });

    renderWithQuery(<SettingsPage settings={CONNECTED} />);
    await userEvent.click(await screen.findByRole("button", { name: /키 삭제/ }));

    expect(removeProvider).toHaveBeenCalledWith({ provider: "anthropic" });
  });

  it("offers no key field for a connected provider, so a stored key is never read back", async () => {
    mockCommands({
      ...BASE,
      list_provider_models: () => [
        { id: "claude-sonnet-5", displayName: "Claude Sonnet 5" },
      ],
    });

    renderWithQuery(<SettingsPage settings={CONNECTED} />);
    await screen.findByRole("button", { name: /키 삭제/ });

    // Only the unconnected providers (OpenAI, DeepSeek) ask for a key; the
    // connected Anthropic provider shows a model selector instead.
    const keyFields = screen.getAllByLabelText("API 키");
    expect(keyFields).toHaveLength(2);
    expect(keyFields[0]).toHaveAttribute("placeholder", "sk-...");
  });
});
