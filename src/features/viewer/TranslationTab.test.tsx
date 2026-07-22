import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { invokeMock, mockCommands } from "@/test/tauri";
import { renderWithQuery } from "@/test/render";
import type { NormalizedRect, PageTranslation, Sentence } from "@/lib/types";
import { TranslationTab } from "./TranslationTab";

const SENTENCES: Sentence[] = [
  {
    id: "s1",
    pageNumber: 1,
    orderIndex: 0,
    paragraphIndex: 0,
    text: "The Transformer achieves 28.4 BLEU.",
    rects: [{ x: 0.1, y: 0.1, width: 0.5, height: 0.02 }],
  },
];

function savedTranslation(): PageTranslation {
  return {
    pageNumber: 1,
    targetLanguage: "ko",
    cached: true,
    units: [
      { id: "u0", text: "트랜스포머는 BLEU 28.4를 달성한다.", sentenceIds: ["s1"], paragraphIndex: 0 },
    ],
  };
}

const noop = (_page: number, _rects: NormalizedRect[], _jump: boolean) => {};

// Korean settings + a Korean first page, so the foreign-paper auto-translate does
// not fire in tests that are about restore/first-translate behavior.
const BASE = {
  get_settings: () => ({ translationLanguage: "ko" }),
  get_page_sentences: () => [
    { id: "s1", pageNumber: 1, orderIndex: 0, paragraphIndex: 0, text: "한국어 문장.", rects: [] },
  ],
};

describe("translation panel", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("restores a saved translation automatically, without a button press", async () => {
    // The exact behavior requested: a page translated earlier reappears when the
    // viewer is reopened, with no network call and no click.
    mockCommands({
      ...BASE,
      get_page_sentences: () => SENTENCES,
      get_cached_translation: () => savedTranslation(),
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={1} hasTextLayer onHoverSentence={noop} />,
    );

    expect(
      await screen.findByText("트랜스포머는 BLEU 28.4를 달성한다."),
    ).toBeInTheDocument();
    // The button offers a re-translate, not a first translate.
    expect(screen.getByRole("button", { name: /다시 번역/ })).toBeInTheDocument();
  });

  it("offers a first translation when the page has none saved", async () => {
    mockCommands({
      ...BASE,
      get_page_sentences: () => SENTENCES,
      get_cached_translation: () => null,
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={1} hasTextLayer onHoverSentence={noop} />,
    );

    expect(
      await screen.findByRole("button", { name: /현재 페이지 번역/ }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(/다음에 열 때 자동으로/),
    ).toBeInTheDocument();
  });

  it("does not call the provider just to restore a page", async () => {
    let translateCalls = 0;
    mockCommands({
      ...BASE,
      get_page_sentences: () => SENTENCES,
      get_cached_translation: () => savedTranslation(),
      translate_page: () => {
        translateCalls += 1;
        return savedTranslation();
      },
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={1} hasTextLayer onHoverSentence={noop} />,
    );

    await screen.findByText("트랜스포머는 BLEU 28.4를 달성한다.");
    expect(translateCalls).toBe(0);
  });

  it("prefetches the next page once the current one is shown", async () => {
    const translated: number[] = [];
    mockCommands({
      get_settings: () => ({ translationLanguage: "ko" }),
      get_page_sentences: () => SENTENCES,
      // Page 1 is already saved (so it restores); page 2 is not.
      get_cached_translation: ({ pageNumber }: { pageNumber: number }) =>
        pageNumber === 1 ? savedTranslation() : null,
      translate_page: ({ pageNumber }: { pageNumber: number }) => {
        translated.push(pageNumber);
        return { ...savedTranslation(), pageNumber };
      },
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={5} hasTextLayer onHoverSentence={noop} />,
    );

    await screen.findByText("트랜스포머는 BLEU 28.4를 달성한다.");
    // The next page (2) is translated in the background; page 1 came from cache.
    await waitFor(() => expect(translated).toContain(2));
    expect(translated).not.toContain(1);
  });

  it("auto-translates the first page of a foreign paper on open", async () => {
    const translated: number[] = [];
    mockCommands({
      get_settings: () => ({ translationLanguage: "ko" }),
      // An English (foreign) first page.
      get_page_sentences: () => [
        { id: "s1", pageNumber: 1, orderIndex: 0, paragraphIndex: 0, text: "This is an English paper about transformers and attention.", rects: [] },
      ],
      get_cached_translation: () => null,
      translate_page: ({ pageNumber }: { pageNumber: number }) => {
        translated.push(pageNumber);
        return { ...savedTranslation(), pageNumber };
      },
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={3} hasTextLayer onHoverSentence={noop} />,
    );

    await waitFor(() => expect(translated).toContain(1));
  });

  it("does not auto-translate a Korean paper", async () => {
    const translated: number[] = [];
    mockCommands({
      get_settings: () => ({ translationLanguage: "ko" }),
      get_page_sentences: () => [
        { id: "s1", pageNumber: 1, orderIndex: 0, paragraphIndex: 0, text: "이것은 한국어로 작성된 논문입니다. 번역이 필요 없습니다.", rects: [] },
      ],
      get_cached_translation: () => null,
      translate_page: ({ pageNumber }: { pageNumber: number }) => {
        translated.push(pageNumber);
        return { ...savedTranslation(), pageNumber };
      },
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={3} hasTextLayer onHoverSentence={noop} />,
    );

    await screen.findByRole("button", { name: /현재 페이지 번역/ });
    // Give any stray effect a chance to fire, then confirm none did.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(translated).toHaveLength(0);
  });

  it("groups the translation into paragraphs and can switch to a whole view", async () => {
    mockCommands({
      ...BASE,
      get_page_sentences: () => SENTENCES,
      get_cached_translation: () => ({
        pageNumber: 1,
        targetLanguage: "ko",
        cached: true,
        units: [
          { id: "u0", text: "첫 문단 문장.", sentenceIds: ["s1"], paragraphIndex: 0 },
          { id: "u1", text: "둘째 문단 문장.", sentenceIds: ["s1"], paragraphIndex: 1 },
        ],
      }),
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={1} hasTextLayer onHoverSentence={noop} />,
    );

    expect(await screen.findByText("첫 문단 문장.")).toBeInTheDocument();
    expect(screen.getByText("둘째 문단 문장.")).toBeInTheDocument();

    // Both views render the same units; switching does not re-fetch.
    await userEvent.click(screen.getByRole("button", { name: "전체" }));
    expect(screen.getByText("첫 문단 문장.")).toBeInTheDocument();
    expect(screen.getByText("둘째 문단 문장.")).toBeInTheDocument();
  });

  it("re-translating overrides the restored copy", async () => {
    mockCommands({
      ...BASE,
      get_page_sentences: () => SENTENCES,
      get_cached_translation: () => savedTranslation(),
      translate_page: () => ({
        pageNumber: 1,
        targetLanguage: "ko",
        cached: false,
        units: [{ id: "u0", text: "새로 번역한 문장.", sentenceIds: ["s1"], paragraphIndex: 0 }],
      }),
    });

    renderWithQuery(
      <TranslationTab paperId="p1" pageNumber={1} pageCount={1} hasTextLayer onHoverSentence={noop} />,
    );

    await userEvent.click(await screen.findByRole("button", { name: /다시 번역/ }));

    await waitFor(() => expect(screen.getByText("새로 번역한 문장.")).toBeInTheDocument());
  });
});
