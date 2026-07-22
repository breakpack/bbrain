import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { invokeMock, mockCommands, settingsFixture } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import { App } from "@/App";

describe("first-run onboarding", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("shows the network notice before any provider key is asked for", async () => {
    mockCommands({ get_settings: () => settingsFixture() });

    renderWithQuery(<App />);

    expect(
      await screen.findByText(/데이터가 어디에 저장되는지 먼저 알려드립니다/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("API 키")).not.toBeInTheDocument();
  });

  it("asks for a provider key only after the notice is accepted", async () => {
    let settings = settingsFixture();
    mockCommands({
      get_settings: () => settings,
      update_settings: ({ patch }) => {
        if (patch.networkNoticeAccepted) {
          settings = { ...settings, networkNoticeAcceptedAt: "2026-07-14T00:00:00Z" };
        }
        return settings;
      },
    });

    renderWithQuery(<App />);
    await userEvent.click(await screen.findByRole("button", { name: /이해했습니다/ }));

    expect(await screen.findByText("AI 공급자를 연결하세요")).toBeInTheDocument();
    expect(screen.getAllByLabelText("API 키")).toHaveLength(2);
  });

  it("lets the user skip AI setup and still enter the app", async () => {
    let settings = settingsFixture({ networkNoticeAcceptedAt: "2026-07-14T00:00:00Z" });
    mockCommands({
      get_settings: () => settings,
      update_settings: ({ patch }) => {
        if (patch.onboardingCompleted) {
          settings = { ...settings, onboardingCompletedAt: "2026-07-14T00:00:00Z" };
        }
        return settings;
      },
    });

    renderWithQuery(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "나중에 설정" }));

    expect(await screen.findByRole("navigation", { name: "주요 메뉴" })).toBeInTheDocument();
  });

  it("keeps the user on setup and explains an invalid key without echoing it", async () => {
    mockCommands({
      get_settings: () =>
        settingsFixture({ networkNoticeAcceptedAt: "2026-07-14T00:00:00Z" }),
      configure_provider: () => {
        throw {
          code: "provider_auth",
          message: "API 키가 올바르지 않거나 권한이 없습니다.",
          retryAfterSeconds: null,
        };
      },
    });

    renderWithQuery(<App />);

    const keyFields = await screen.findAllByLabelText("API 키");
    await userEvent.type(keyFields[0], "sk-wrong-key");
    await userEvent.click(screen.getAllByRole("button", { name: "연결" })[0]);

    const alert = await screen.findByRole("alert");
    await waitFor(() => expect(alert).toHaveTextContent(/API 키가 올바르지 않거나/));
    expect(document.body.textContent).not.toContain("sk-wrong-key");
  });
});
