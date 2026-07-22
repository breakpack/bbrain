import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { GlobalWorkerOptions } from "pdfjs-dist";
import { describe, expect, it } from "vitest";

import {
  buildParagraphText,
  extractPage,
  findRepeatedLines,
  groupIntoParagraphs,
  loadDocument,
  mergeRects,
  splitAroundWord,
  splitSentenceRanges,
  subtractRect,
  type ExtractedItem,
} from "./pdf";

// In the app the worker is fetched over HTTP from the same origin. Under Node
// there is no Worker, so pdf.js imports the worker module from disk instead —
// which needs a real path rather than the bundler's asset URL.
GlobalWorkerOptions.workerSrc = createRequire(import.meta.url).resolve(
  "pdfjs-dist/build/pdf.worker.min.mjs",
);

/**
 * Runs the real extraction against a real PDF with a real text layer, so the
 * coordinate math is exercised rather than mocked (DEVELOPMENT.md §18.4).
 */
const FIXTURE = resolve(__dirname, "../test/fixtures/attention.pdf");
const TWO_COLUMN = resolve(__dirname, "../test/fixtures/two-column.pdf");

async function firstPage() {
  const bytes = new Uint8Array(readFileSync(FIXTURE));
  const pdf = await loadDocument(bytes);
  return { pdf, page: await pdf.getPage(1) };
}

describe("pdf extraction", () => {
  it("reads the text of a two-page paper", async () => {
    const bytes = new Uint8Array(readFileSync(FIXTURE));
    const pdf = await loadDocument(bytes);

    expect(pdf.numPages).toBe(2);

    const extraction = await extractPage(await pdf.getPage(1));
    expect(extraction.text).toContain("Attention Is All You Need");
    expect(extraction.text).toContain("Transformer");
  });

  it("does not detach the caller's bytes", async () => {
    const bytes = new Uint8Array(readFileSync(FIXTURE));
    await loadDocument(bytes);

    // getDocument transfers its buffer to the worker; loadDocument copies first.
    expect(bytes.byteLength).toBeGreaterThan(0);
  });

  it("produces sentences with normalized rectangles inside the page", async () => {
    const { page } = await firstPage();

    const extraction = await extractPage(page);

    expect(extraction.sentences.length).toBeGreaterThan(0);
    for (const sentence of extraction.sentences) {
      expect(sentence.rects.length).toBeGreaterThan(0);
      for (const rect of sentence.rects) {
        expect(rect.x).toBeGreaterThanOrEqual(0);
        expect(rect.y).toBeGreaterThanOrEqual(0);
        expect(rect.x + rect.width).toBeLessThanOrEqual(1.001);
        expect(rect.y + rect.height).toBeLessThanOrEqual(1.001);
      }
    }
  });

  it("reports the unrotated page box, not the rendered size", async () => {
    const { page } = await firstPage();

    const extraction = await extractPage(page);

    expect(extraction.width).toBe(612);
    expect(extraction.height).toBe(792);
    expect(extraction.rotation).toBe(0);
  });

  it("keeps sentences in reading order", async () => {
    const { page } = await firstPage();

    const extraction = await extractPage(page);
    const joined = extraction.sentences.map((sentence) => sentence.text).join(" ");

    expect(joined.indexOf("Attention Is All You Need")).toBeLessThan(
      joined.indexOf("Recurrent neural networks"),
    );
  });

  it("keeps a title separate from the paragraph below it", async () => {
    const { page } = await firstPage();

    const extraction = await extractPage(page);
    const title = extraction.sentences.find((s) => s.text.includes("Attention Is All You Need"));

    // A heading carries no full stop; without a block boundary it would be
    // glued to the abstract and mistranslated as one sentence.
    expect(title).toBeDefined();
    expect(title!.text).not.toContain("dominant sequence");
  });

  it("places the title above the body on the page", async () => {
    const { page } = await firstPage();

    const extraction = await extractPage(page);
    const title = extraction.sentences.find((s) => s.text.includes("Attention Is All You Need"));
    const body = extraction.sentences.find((s) => s.text.includes("Recurrent neural networks"));

    expect(title).toBeDefined();
    expect(body).toBeDefined();
    // y grows downward in the stored coordinate space.
    expect(title!.rects[0].y).toBeLessThan(body!.rects[0].y);
  });
});

describe("two-column reading order", () => {
  async function twoColumnPage() {
    const bytes = new Uint8Array(readFileSync(TWO_COLUMN));
    const pdf = await loadDocument(bytes);
    return pdf.getPage(1);
  }

  it("reads the left column fully before the right, without interleaving", async () => {
    const extraction = await extractPage(await twoColumnPage());
    const text = extraction.sentences.map((s) => s.text).join(" ");

    // Every left line must appear before every right line — the bug interleaved
    // them ("Left ... Right ... Left ...").
    const firstRight = text.indexOf("Right column first");
    const lastLeft = text.lastIndexOf("Left column eighth");
    expect(firstRight).toBeGreaterThan(-1);
    expect(lastLeft).toBeGreaterThan(-1);
    expect(lastLeft).toBeLessThan(firstRight);
  });

  it("does not glue a left-column line onto the right-column line beside it", async () => {
    const extraction = await extractPage(await twoColumnPage());

    // No sentence may contain text from both columns.
    for (const sentence of extraction.sentences) {
      const hasLeft = sentence.text.includes("Left column");
      const hasRight = sentence.text.includes("Right column");
      expect(hasLeft && hasRight).toBe(false);
    }
  });
});

describe("rectangle merging", () => {
  it("merges glyph runs on the same line into one bar", () => {
    const merged = mergeRects([
      { x: 0.1, y: 0.2, width: 0.1, height: 0.02 },
      { x: 0.2, y: 0.2, width: 0.1, height: 0.02 },
    ]);

    expect(merged).toHaveLength(1);
    expect(merged[0].width).toBeCloseTo(0.2, 5);
  });

  it("keeps separate lines separate", () => {
    const merged = mergeRects([
      { x: 0.1, y: 0.2, width: 0.3, height: 0.02 },
      { x: 0.1, y: 0.3, width: 0.3, height: 0.02 },
    ]);

    expect(merged).toHaveLength(2);
  });
});

describe("word-level highlight trimming", () => {
  // One line-bar highlight from x=0.1 to x=0.7 at y=0.2, height 0.02.
  const line = { x: 0.1, y: 0.2, width: 0.6, height: 0.02 };

  it("splits the line into two when a word in the middle is removed", () => {
    const word = { x: 0.3, y: 0.2, width: 0.1, height: 0.02 };
    const out = subtractRect([line], word);

    expect(out).toHaveLength(2);
    // left remainder ends before the word, right remainder starts after it.
    expect(out[0].x).toBeCloseTo(0.1);
    expect(out[0].x + out[0].width).toBeLessThan(word.x);
    expect(out[1].x).toBeGreaterThan(word.x + word.width);
    expect(out[1].x + out[1].width).toBeCloseTo(0.7);
  });

  it("leaves one remainder when the first word is removed", () => {
    const firstWord = { x: 0.1, y: 0.2, width: 0.12, height: 0.02 };
    const out = subtractRect([line], firstWord);

    expect(out).toHaveLength(1);
    expect(out[0].x).toBeGreaterThan(firstWord.x + firstWord.width - 0.01);
  });

  it("removes the line entirely when the word covers it", () => {
    const whole = { x: 0.1, y: 0.2, width: 0.6, height: 0.02 };
    expect(subtractRect([line], whole)).toHaveLength(0);
  });

  it("leaves rectangles on other lines untouched", () => {
    const other = { x: 0.1, y: 0.5, width: 0.6, height: 0.02 };
    const word = { x: 0.3, y: 0.2, width: 0.1, height: 0.02 };
    const out = subtractRect([other], word);
    expect(out).toEqual([other]);
  });

  it("splits into before and after when a middle word is removed", () => {
    const word = { x: 0.3, y: 0.2, width: 0.1, height: 0.02 };
    const { before, after } = splitAroundWord([line], word);

    expect(before).toHaveLength(1);
    expect(after).toHaveLength(1);
    expect(before[0].x).toBeCloseTo(0.1);
    expect(after[0].x + after[0].width).toBeCloseTo(0.7);
  });

  it("puts whole earlier lines before and later lines after the word", () => {
    const lineAbove = { x: 0.1, y: 0.16, width: 0.6, height: 0.02 };
    const lineBelow = { x: 0.1, y: 0.24, width: 0.6, height: 0.02 };
    const word = { x: 0.3, y: 0.2, width: 0.1, height: 0.02 };

    const { before, after } = splitAroundWord([lineAbove, line, lineBelow], word);

    expect(before).toContainEqual(lineAbove);
    expect(after).toContainEqual(lineBelow);
  });

  it("leaves only one side when the first word is removed", () => {
    const firstWord = { x: 0.1, y: 0.2, width: 0.12, height: 0.02 };
    const { before, after } = splitAroundWord([line], firstWord);

    expect(before).toHaveLength(0);
    expect(after).toHaveLength(1);
  });
});

describe("running headers", () => {
  it("finds a line that repeats across most pages", () => {
    const pages = Array.from({ length: 6 }, (_, index) => [
      "Preprint. Under review.",
      `body of page ${index}`,
      `${index + 1}`,
    ]);

    const repeated = findRepeatedLines(pages);

    expect(repeated.has("preprint. under review.")).toBe(true);
  });

  it("does not treat a one-off line as a header", () => {
    const pages = [
      ["Unique opening", "first body"],
      ["Another", "second body"],
      ["Third", "third body"],
      ["Fourth", "fourth body"],
    ];

    const repeated = findRepeatedLines(pages);

    expect(repeated.size).toBe(0);
  });
});

// Builds a Line from words, each word an item on the same baseline. The rects
// are placeholders — these tests exercise the text/boundary logic, not geometry.
function line(...words: string[]) {
  let x = 0;
  const items: ExtractedItem[] = words.map((text) => {
    const item: ExtractedItem = {
      text,
      rect: { x, y: 0, width: 0.05, height: 0.02 },
      hasEOL: false,
      baseline: 0,
      heightPt: 10,
      left: x,
    };
    x += 0.06;
    return item;
  });
  return { items, baseline: 0, left: 0, right: x };
}

describe("hyphenation across a line break", () => {
  it("rejoins a soft-hyphenated word", () => {
    // The exact reported bug: "Mathematics edu-" / "cators" was cut mid-word.
    const { text } = buildParagraphText([line("Mathematics", "edu-"), line("cators", "teach.")]);

    expect(text).toContain("educators");
    expect(text).not.toContain("edu- cators");
    expect(text).not.toContain("edu-cators");
  });

  it("keeps a real hyphen before a capitalized continuation", () => {
    // A line break before a proper noun is a word space, not a soft hyphen.
    const { text } = buildParagraphText([line("a", "non-"), line("Euclidean", "space.")]);

    expect(text).toContain("non- Euclidean");
  });
});

// A one-item line at a given vertical baseline and right edge, for exercising
// paragraph grouping (which reads baseline, right, and font height only).
function paraLine(text: string, opts: { baseline: number; right: number; heightPt?: number }) {
  const heightPt = opts.heightPt ?? 10;
  const item: ExtractedItem = {
    text,
    rect: { x: 0.1, y: 0.1, width: opts.right - 0.1, height: 0.02 },
    hasEOL: false,
    baseline: opts.baseline,
    heightPt,
    left: 0.1,
  };
  return { items: [item], baseline: opts.baseline, left: 0.1, right: opts.right };
}

describe("paragraph grouping", () => {
  const texts = (groups: ReturnType<typeof groupIntoParagraphs>) =>
    groups.map((group) => group.map((line) => line.items[0].text).join(" "));

  it("keeps a sentence wrapped across a short line in one paragraph", () => {
    // The reported bug: a mid-sentence line wrap ("안녕하세요 저" / "는 홍길동입니다.")
    // was split into two paragraphs — and so two separate translations.
    const groups = groupIntoParagraphs([
      paraLine("안녕하세요 저", { baseline: 100, right: 0.6 }),
      paraLine("는 홍길동입니다.", { baseline: 112, right: 0.5 }),
    ]);

    expect(texts(groups)).toEqual(["안녕하세요 저 는 홍길동입니다."]);
  });

  it("splits paragraphs at a completed sentence on a short line", () => {
    // A full-width body line sets the column width; the paragraph's last line is
    // short AND ends a sentence, so it closes the paragraph.
    const groups = groupIntoParagraphs([
      paraLine("첫 문단의 본문이 한 줄을 가득 채웁니다", { baseline: 100, right: 0.9 }),
      paraLine("그리고 여기서 끝납니다.", { baseline: 112, right: 0.5 }),
      paraLine("둘째 문단이 이어집니다.", { baseline: 124, right: 0.5 }),
    ]);

    expect(texts(groups)).toEqual([
      "첫 문단의 본문이 한 줄을 가득 채웁니다 그리고 여기서 끝납니다.",
      "둘째 문단이 이어집니다.",
    ]);
  });

  it("splits at a wide vertical gap even without terminal punctuation", () => {
    // A heading has no period but is set off by extra vertical space.
    const groups = groupIntoParagraphs([
      paraLine("제목 없는 소제목", { baseline: 100, right: 0.3, heightPt: 12 }),
      paraLine("본문 문장이 시작됩니다.", { baseline: 140, right: 0.9, heightPt: 10 }),
    ]);

    expect(texts(groups)).toEqual(["제목 없는 소제목", "본문 문장이 시작됩니다."]);
  });
});

describe("sentence boundaries", () => {
  const sentences = (text: string) =>
    splitSentenceRanges(text).map(([s, e]) => text.slice(s, e).trim());

  it("does not split a decimal number", () => {
    expect(sentences("The Transformer achieves 28.4 BLEU.")).toEqual([
      "The Transformer achieves 28.4 BLEU.",
    ]);
  });

  it("does not split a section-number heading from its title", () => {
    // The reported "2. Considerations" bug: the section number's dot is not an end.
    expect(sentences("2. Considerations for the model.")).toEqual([
      "2. Considerations for the model.",
    ]);
  });

  it("does not split after a common abbreviation", () => {
    expect(sentences("See e.g. the results. They hold.")).toEqual([
      "See e.g. the results.",
      "They hold.",
    ]);
  });

  it("splits two ordinary sentences", () => {
    expect(sentences("First sentence here. Second one follows.")).toEqual([
      "First sentence here.",
      "Second one follows.",
    ]);
  });
});
