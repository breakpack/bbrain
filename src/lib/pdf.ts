import {
  GlobalWorkerOptions,
  Util,
  getDocument,
  type PDFDocumentProxy,
  type PDFPageProxy,
} from "pdfjs-dist";
// `?url` goes through Vite's resolver and emits a same-origin asset. A
// cross-origin workerSrc would make pdf.js wrap it in a blob: URL, which the
// app's CSP blocks.
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

import { installStreamAsyncIterator } from "./polyfills";
import type { NormalizedRect } from "./types";

// pdf.js reads its text stream with `for await`, which WKWebView cannot do.
// Install the shim before any pdf.js call, not at app start, so every consumer
// of this module is covered.
installStreamAsyncIterator();

GlobalWorkerOptions.workerSrc = workerUrl;

/**
 * `getDocument({data})` transfers the buffer to the worker and detaches it, so
 * the caller's bytes would become unusable. Copy before handing them over.
 */
export function loadDocument(bytes: Uint8Array): Promise<PDFDocumentProxy> {
  // With no usable `data`, pdf.js falls back to its URL/stream path and fails
  // with a confusing internal error instead of saying the bytes were missing.
  if (!(bytes instanceof Uint8Array) || bytes.byteLength === 0) {
    throw new Error(
      `pdf bytes are unusable (type=${Object.prototype.toString.call(bytes)}, length=${
        (bytes as { byteLength?: number })?.byteLength ?? "none"
      })`,
    );
  }

  return getDocument({ data: bytes.slice() }).promise;
}

/** In v6 the document is torn down through its loading task, not the proxy. */
export function destroyDocument(pdf: PDFDocumentProxy): Promise<void> {
  return pdf.loadingTask.destroy();
}

/**
 * Maps PDF user space to the unrotated, unscaled page box with a top-left
 * origin. `rawDims` comes from the viewBox and does not change with scale or
 * rotation, so rectangles derived from it are zoom- and rotation-independent —
 * exactly what the DB expects (DEVELOPMENT.md §7.3).
 */
function pageSpaceMatrix(page: PDFPageProxy): {
  matrix: number[];
  pageWidth: number;
  pageHeight: number;
} {
  const { rawDims } = page.getViewport({ scale: 1 }) as unknown as {
    rawDims: { pageWidth: number; pageHeight: number; pageX: number; pageY: number };
  };
  const { pageWidth, pageHeight, pageX, pageY } = rawDims;

  return {
    matrix: [1, 0, 0, -1, -pageX, pageY + pageHeight],
    pageWidth,
    pageHeight,
  };
}

export type ExtractedItem = {
  text: string;
  rect: NormalizedRect;
  hasEOL: boolean;
  /** Baseline y in page points, used to group items into lines. */
  baseline: number;
  /** Font height in page points — the tolerance for "same line". */
  heightPt: number;
  left: number;
};

export async function extractItems(page: PDFPageProxy): Promise<ExtractedItem[]> {
  const { matrix, pageWidth, pageHeight } = pageSpaceMatrix(page);
  const content = await page.getTextContent();
  const styles = content.styles as Record<string, { vertical?: boolean }>;

  const items: ExtractedItem[] = [];

  for (const item of content.items) {
    if (!("str" in item)) continue; // TextMarkedContent
    if (item.str.length === 0) continue;

    const tx = Util.transform(matrix, item.transform);
    const fontHeight = Math.hypot(tx[2], tx[3]);
    const vertical = styles[item.fontName]?.vertical ?? false;

    const width = vertical ? fontHeight : item.width;
    const height = vertical ? item.height : fontHeight;
    if (width <= 0 || height <= 0) continue;

    const left = tx[4];
    const top = tx[5] - height;

    items.push({
      text: item.str,
      hasEOL: item.hasEOL,
      baseline: tx[5],
      heightPt: height,
      left,
      rect: {
        x: left / pageWidth,
        y: top / pageHeight,
        width: width / pageWidth,
        height: height / pageHeight,
      },
    });
  }

  return items;
}

/** Two items are on the same line when their baselines sit within half a line. */
const SAME_LINE_TOLERANCE = 0.6;

type Line = {
  items: ExtractedItem[];
  baseline: number;
  left: number;
  right: number;
};

function groupIntoLines(items: ExtractedItem[]): Line[] {
  const lines: Line[] = [];

  for (const item of items) {
    const tolerance = item.heightPt * SAME_LINE_TOLERANCE;
    const line = lines.find(
      (candidate) => Math.abs(candidate.baseline - item.baseline) <= tolerance,
    );

    if (line) {
      line.items.push(item);
      line.left = Math.min(line.left, item.rect.x);
      line.right = Math.max(line.right, item.rect.x + item.rect.width);
    } else {
      lines.push({
        items: [item],
        baseline: item.baseline,
        left: item.rect.x,
        right: item.rect.x + item.rect.width,
      });
    }
  }

  for (const line of lines) {
    line.items.sort((a, b) => a.left - b.left);
  }

  // Page space is y-down, so a smaller baseline means higher on the page.
  return lines.sort((a, b) => a.baseline - b.baseline);
}

/**
 * Finds the x of a two-column gutter, or null for a single column. Detection is
 * done at the *item* level, before lines are formed: a left-column line and a
 * right-column line share a baseline, so grouping into lines first would merge
 * them and interleave the columns (DEVELOPMENT.md §9.3).
 *
 * A gutter is an x in the middle of the page that almost no text item spans,
 * with substantial text on both sides.
 */
const isBlank = (item: ExtractedItem) => item.text.trim().length === 0;

/**
 * Finds the x of a two-column gutter, judging by item *left edges* only, which
 * come straight from the text transform and are the same in every environment.
 * Item widths are deliberately not used: font metrics differ between the browser
 * and a headless renderer, so a width-based test (does a line cross the middle?)
 * detects columns in one and misses them in the other. Left-aligned columns show
 * up as two clusters of left edges with an empty band — the gutter — between
 * them.
 */
function detectColumnGutter(items: ExtractedItem[]): number | null {
  const content = items.filter((item) => !isBlank(item));
  if (content.length < 12) return null;

  let best: { gutter: number; band: number } | null = null;

  for (let gutter = 0.42; gutter <= 0.58; gutter += 0.01) {
    let left = 0;
    let right = 0;
    let band = 0; // left edges sitting in the gutter itself

    for (const item of content) {
      const edge = item.rect.x;
      if (Math.abs(edge - gutter) < 0.04) band += 1;
      if (edge < gutter) left += 1;
      else right += 1;
    }

    // Both columns must be well populated by left edges, or this is one column
    // with a stray indented/centered element.
    if (left < content.length * 0.25 || right < content.length * 0.25) continue;
    if (best === null || band < best.band) best = { gutter, band };
  }

  // A real gutter has almost no line *starting* inside it — the whitespace band
  // between the two column margins.
  if (best && best.band < content.length * 0.03) return best.gutter;
  return null;
}

/**
 * Reading order for a page: one column list if single-column, or the left
 * column's lines followed by the right column's, each grouped independently so
 * same-baseline items from different columns never merge.
 */
function readingColumns(items: ExtractedItem[]): Line[][] {
  const gutter = detectColumnGutter(items);
  if (gutter === null) {
    return [groupIntoLines(items)];
  }

  const left: ExtractedItem[] = [];
  const right: ExtractedItem[] = [];
  for (const item of items) {
    // A blank filler item straddling the gutter belongs to neither column.
    if (isBlank(item)) continue;
    // Assign by left edge (width-independent): a left-column line starts before
    // the gutter, a right-column line at or after it.
    (item.rect.x < gutter ? left : right).push(item);
  }

  return [groupIntoLines(left), groupIntoLines(right)];
}

/**
 * Header and footer lines repeat across pages; they are excluded from analysis
 * and embedding but stay in the rendered page (§9.3).
 */
export function findRepeatedLines(pageLines: string[][]): Set<string> {
  const repeated = new Set<string>();
  if (pageLines.length < 4) return repeated;

  const counts = new Map<string, number>();
  for (const lines of pageLines) {
    const edges = [...lines.slice(0, 1), ...lines.slice(-1)];
    for (const line of new Set(edges)) {
      const key = normalizeRunning(line);
      if (key.length < 4) continue;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
  }

  const threshold = Math.max(3, Math.floor(pageLines.length * 0.6));
  for (const [line, count] of counts) {
    if (count >= threshold) repeated.add(line);
  }
  return repeated;
}

/** Page numbers differ per page, so digits are masked before comparing. */
function normalizeRunning(line: string): string {
  return line.trim().replace(/\d+/g, "#").toLowerCase();
}

export type PageSentence = {
  orderIndex: number;
  /** Sentences sharing a paragraphIndex belong to the same paragraph. */
  paragraphIndex: number;
  text: string;
  rects: NormalizedRect[];
};

export type PageExtraction = {
  pageNumber: number;
  width: number;
  height: number;
  rotation: number;
  text: string;
  lines: string[];
  sentences: PageSentence[];
};

/**
 * How far short of the column edge a line must stop to count as the end of a
 * block (a title, heading, or the last line of a paragraph) rather than a line
 * wrap. In fractions of the page width.
 */
const SHORT_LINE_MARGIN = 0.12;

/**
 * A vertical gap larger than this multiple of the line's font height marks a new
 * paragraph or a heading. Normal line spacing is ~1.2×; a paragraph break or the
 * space around a heading is larger. Kept conservative so ordinary spacing never
 * triggers it — over-merging paragraphs is harmless, but splitting a sentence
 * mid-wrap is not.
 */
const PARAGRAPH_GAP_RATIO = 1.6;

/** A line that ends a sentence: terminal punctuation, optionally closing quotes. */
function endsSentence(text: string): boolean {
  return /[.!?。！？…]["')\]”’]*$/.test(text.trim());
}

const lineText = (line: Line): string =>
  line.items.map((item) => item.text).join("").trim();

/**
 * Groups a column's (non-blank) lines into paragraphs. A line ends its paragraph
 * when the next line is a wide vertical gap away (a heading or real paragraph
 * break) or when the line stops short of the column AND ends a sentence. A short
 * line that does NOT end a sentence is a mid-sentence wrap, so the paragraph
 * continues and the sentence is never cut at the line break (§9.3).
 */
export function groupIntoParagraphs(lines: Line[]): Line[][] {
  const columnRight = lines.reduce((widest, line) => Math.max(widest, line.right), 0);

  const paragraphs: Line[][] = [];
  let current: Line[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    current.push(line);

    const next = lines[i + 1];
    const lineHeight = Math.max(...line.items.map((item) => item.heightPt));
    const short = line.right < columnRight - SHORT_LINE_MARGIN;
    const gap = next ? next.baseline - line.baseline : Infinity;

    if (!next || gap > lineHeight * PARAGRAPH_GAP_RATIO || (short && endsSentence(lineText(line)))) {
      paragraphs.push(current);
      current = [];
    }
  }

  if (current.length > 0) paragraphs.push(current);
  return paragraphs;
}

/** Maps a slice of a paragraph's text back to the item (and rect) it came from. */
type TextPiece = { start: number; end: number; item: ExtractedItem };

/**
 * Joins a paragraph's lines into one string, undoing the line-break hyphenation
 * that would otherwise cut a word ("edu-" + "cators" → "educators"), and records
 * where each item's text lands so a sentence can be traced back to its rectangles.
 */
export function buildParagraphText(lines: Line[]): { text: string; pieces: TextPiece[] } {
  let text = "";
  const pieces: TextPiece[] = [];

  for (let li = 0; li < lines.length; li++) {
    for (const item of lines[li].items) {
      const start = text.length;
      text += item.text;
      pieces.push({ start, end: text.length, item });
    }

    if (li === lines.length - 1) break;

    const nextFirst =
      lines[li + 1].items
        .map((item) => item.text)
        .join("")
        .trimStart()[0] ?? "";

    // A trailing hyphen before a lowercase continuation is a soft hyphen from
    // line wrapping: drop it and join the halves. Otherwise the line break is a
    // word space.
    const trimmed = text.replace(/\s+$/, "");
    const lastChar = trimmed[trimmed.length - 1];
    if ((lastChar === "-" || lastChar === "­") && /[a-z]/.test(nextFirst)) {
      text = trimmed.slice(0, -1);
      const last = pieces[pieces.length - 1];
      if (last) last.end = Math.min(last.end, text.length);
    } else {
      text += " ";
    }
  }

  return { text, pieces };
}

const ABBREVIATIONS = new Set([
  "e.g", "i.e", "cf", "vs", "etc", "al", "fig", "eq", "no", "pp", "p", "vol",
  "dr", "mr", "mrs", "ms", "prof", "st", "ch", "sec", "ref", "eds", "ed",
]);

/** The token just before a period looks like an abbreviation, not a sentence end. */
function endsWithAbbreviation(before: string): boolean {
  const match = before.match(/([A-Za-z.]+)\.?$/);
  if (!match) return false;
  const token = match[1].replace(/\./g, "").toLowerCase();
  // A single capital letter is usually an initial ("A. Theobold").
  if (/^[A-Z]$/.test(match[1])) return true;
  return ABBREVIATIONS.has(token);
}

/**
 * Splits paragraph text into sentence character ranges. Guards against the false
 * boundaries that made sentences look "cut off": decimals (28.4), abbreviations
 * (e.g., et al., p. 8), and initials.
 */
export function splitSentenceRanges(text: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  const re = /[.!?。！？]["')\]”’]?/g;
  let start = 0;
  let match: RegExpExecArray | null;

  while ((match = re.exec(text)) !== null) {
    const end = match.index + match[0].length;
    const before = text.slice(start, match.index + 1);
    const nextChar = text[end];
    const prevChar = text[match.index - 1];
    const followingChar = text[end + (/\s/.test(nextChar ?? "") ? 1 : 0)];

    // A decimal or numbered reference like "2.4" or "Fig. 1": digits on both
    // sides of the dot.
    if (/\d/.test(prevChar ?? "") && /\d/.test(nextChar ?? "")) continue;
    // A section number heading like "2." or "3.1." — the text so far is only a
    // number, so the dot is a section marker, not a sentence end.
    if (/^\d+(\.\d+)*\.?$/.test(before.trim())) continue;
    if (endsWithAbbreviation(before)) continue;
    // A sentence continues if the next visible character is lowercase.
    if (followingChar && /[a-z]/.test(followingChar)) continue;

    if (nextChar === undefined || /\s/.test(nextChar)) {
      ranges.push([start, end]);
      start = end;
    }
  }

  if (start < text.length) ranges.push([start, text.length]);
  return ranges;
}

/**
 * Extracts a page as paragraphs of sentences. Reading order comes from column
 * detection; within a paragraph, lines are joined (hyphenation undone) and split
 * into sentences on the full text, so a sentence is never cut at a line break or
 * a decimal point. Each sentence keeps the rectangles of the items it covers so
 * the viewer can highlight the source of a translation (§9.3).
 */
export async function extractPage(page: PDFPageProxy): Promise<PageExtraction> {
  const items = await extractItems(page);
  const columns = readingColumns(items);

  const sentences: PageSentence[] = [];
  const lineTexts: string[] = [];
  let paragraphIndex = 0;

  const flushParagraph = (paragraphLines: Line[]) => {
    if (paragraphLines.length === 0) return;

    const { text, pieces } = buildParagraphText(paragraphLines);
    let produced = false;

    for (const [start, end] of splitSentenceRanges(text)) {
      const sentenceText = text.slice(start, end).trim();
      if (sentenceText.length === 0) continue;

      const rects = pieces
        .filter((piece) => piece.end > start && piece.start < end)
        .map((piece) => piece.item.rect);
      if (rects.length === 0) continue;

      sentences.push({
        orderIndex: sentences.length,
        paragraphIndex,
        text: sentenceText,
        rects: mergeRects(rects),
      });
      produced = true;
    }

    if (produced) paragraphIndex += 1;
  };

  for (const column of columns) {
    const lines = column.filter((line) => lineText(line).length > 0);
    for (const line of lines) lineTexts.push(lineText(line));
    for (const paragraph of groupIntoParagraphs(lines)) flushParagraph(paragraph);
  }

  const viewport = page.getViewport({ scale: 1, rotation: 0 });
  const { rawDims } = viewport as unknown as {
    rawDims: { pageWidth: number; pageHeight: number };
  };

  return {
    pageNumber: page.pageNumber,
    width: rawDims.pageWidth,
    height: rawDims.pageHeight,
    rotation: page.rotate,
    text: lineTexts.join("\n"),
    lines: lineTexts,
    sentences,
  };
}

/**
 * One rectangle per visual line rather than one per glyph run: a sentence that
 * spans three lines should highlight as three bars, not forty slivers.
 */
export function mergeRects(rects: NormalizedRect[]): NormalizedRect[] {
  const merged: NormalizedRect[] = [];

  for (const rect of rects) {
    const line = merged.find(
      (candidate) =>
        Math.abs(candidate.y - rect.y) < rect.height * 0.5 &&
        Math.abs(candidate.height - rect.height) < rect.height * 0.5,
    );

    if (!line) {
      merged.push({ ...rect });
      continue;
    }

    const right = Math.max(line.x + line.width, rect.x + rect.width);
    const bottom = Math.max(line.y + line.height, rect.y + rect.height);
    line.x = Math.min(line.x, rect.x);
    line.y = Math.min(line.y, rect.y);
    line.width = right - line.x;
    line.height = bottom - line.y;
  }

  return merged;
}

/**
 * Converts a DOM selection over the text layer into page-space rectangles that
 * hug the text. The selection's own line boxes span the full text-layer width and
 * bleed into the margin, so instead we measure each selected text node's own
 * sub-range — those rectangles are tight to the glyphs — and merge them into one
 * continuous bar per line (§9.5).
 */
export function selectionToRects(range: Range, layer: HTMLElement): NormalizedRect[] {
  const box = layer.getBoundingClientRect();
  if (box.width === 0 || box.height === 0) return [];

  const collected: DOMRect[] = [];
  const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!range.intersectsNode(node)) continue;

    const text = node.textContent ?? "";
    let start = node === range.startContainer ? range.startOffset : 0;
    let end = node === range.endContainer ? range.endOffset : text.length;
    // Trim whitespace at the slice's edges — pdf.js pads spans with spaces and
    // renders them (white-space: pre), so an untrimmed rect reaches into the
    // margin. Inner spaces are kept; the per-line merge fills the small gaps.
    while (start < end && /\s/.test(text[start])) start++;
    while (end > start && /\s/.test(text[end - 1])) end--;
    if (end <= start) continue;

    const sub = document.createRange();
    sub.setStart(node, start);
    sub.setEnd(node, end);

    for (const rect of sub.getClientRects()) {
      if (rect.width > 0.5 && rect.height > 0.5) collected.push(rect);
    }
  }

  // Merge the tight per-node rectangles into one bar per visual line.
  type Line = { top: number; bottom: number; left: number; right: number };
  const lines: Line[] = [];
  for (const rect of collected) {
    const line = lines.find(
      (candidate) =>
        Math.min(candidate.bottom, rect.bottom) - Math.max(candidate.top, rect.top) >
        Math.min(candidate.bottom - candidate.top, rect.height) * 0.5,
    );
    if (line) {
      line.left = Math.min(line.left, rect.left);
      line.right = Math.max(line.right, rect.right);
      line.top = Math.min(line.top, rect.top);
      line.bottom = Math.max(line.bottom, rect.bottom);
    } else {
      lines.push({ top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right });
    }
  }

  const rects: NormalizedRect[] = [];
  for (const line of lines) {
    const x = (line.left - box.left) / box.width;
    const y = (line.top - box.top) / box.height;
    if (x < -0.01 || y < -0.01) continue;
    rects.push({
      x,
      y,
      width: (line.right - line.left) / box.width,
      height: (line.bottom - line.top) / box.height,
    });
  }

  return rects;
}

/**
 * Removes a `cut` rectangle (one word) from a set of highlight rectangles,
 * splitting the line it lands on into the left and right remainders. The cut is
 * padded slightly so the whitespace beside the word is removed too, and tiny
 * slivers are dropped. Used for word-level highlight deletion (§9.5).
 */
export function subtractRect(rects: NormalizedRect[], cut: NormalizedRect): NormalizedRect[] {
  const pad = cut.height * 0.25;
  const cutLeft = cut.x - pad;
  const cutRight = cut.x + cut.width + pad;
  const MIN_WIDTH = 0.004;

  const out: NormalizedRect[] = [];
  for (const rect of rects) {
    const sameLine =
      Math.min(rect.y + rect.height, cut.y + cut.height) - Math.max(rect.y, cut.y) >
      Math.min(rect.height, cut.height) * 0.4;
    const rectRight = rect.x + rect.width;

    if (!sameLine || cutRight <= rect.x || cutLeft >= rectRight) {
      out.push(rect);
      continue;
    }

    const leftEnd = Math.min(Math.max(cutLeft, rect.x), rectRight);
    if (leftEnd - rect.x > MIN_WIDTH) {
      out.push({ ...rect, x: rect.x, width: leftEnd - rect.x });
    }
    const rightStart = Math.max(Math.min(cutRight, rectRight), rect.x);
    if (rectRight - rightStart > MIN_WIDTH) {
      out.push({ ...rect, x: rightStart, width: rectRight - rightStart });
    }
  }

  return out;
}

/**
 * Splits a highlight's rectangles around a removed word into the part that comes
 * before it and the part after it, in reading order. When a word in the middle
 * is deleted this yields two non-empty groups, so the highlight can become two
 * separate entries (§9.5).
 */
export function splitAroundWord(
  rects: NormalizedRect[],
  cut: NormalizedRect,
): { before: NormalizedRect[]; after: NormalizedRect[] } {
  const pad = cut.height * 0.25;
  const cutLeft = cut.x - pad;
  const cutRight = cut.x + cut.width + pad;
  const MIN_WIDTH = 0.004;

  const before: NormalizedRect[] = [];
  const after: NormalizedRect[] = [];

  for (const rect of rects) {
    const sameLine =
      Math.min(rect.y + rect.height, cut.y + cut.height) - Math.max(rect.y, cut.y) >
      Math.min(rect.height, cut.height) * 0.4;
    const rectRight = rect.x + rect.width;

    if (!sameLine) {
      // A whole line above the word goes before; below goes after.
      if (rect.y + rect.height <= cut.y + cut.height * 0.5) before.push(rect);
      else after.push(rect);
      continue;
    }

    const leftEnd = Math.min(Math.max(cutLeft, rect.x), rectRight);
    if (leftEnd - rect.x > MIN_WIDTH) before.push({ ...rect, x: rect.x, width: leftEnd - rect.x });
    const rightStart = Math.max(Math.min(cutRight, rectRight), rect.x);
    if (rectRight - rightStart > MIN_WIDTH) {
      after.push({ ...rect, x: rightStart, width: rectRight - rightStart });
    }
  }

  return { before, after };
}

/**
 * The whitespace-delimited word under a point (in client coordinates), or null.
 * Used to delete a single word from a highlight (DEVELOPMENT.md §9.5).
 */
export function wordRangeAtPoint(x: number, y: number): Range | null {
  const caret = document.caretRangeFromPoint?.(x, y);
  if (!caret) return null;
  const node = caret.startContainer;
  if (node.nodeType !== Node.TEXT_NODE) return null;

  const text = node.textContent ?? "";
  const offset = caret.startOffset;

  // Intl.Segmenter gives proper word boundaries without needing a live selection
  // (native selection is disabled on the text layer). The pdf.js text node can
  // hold several words, so segmenting picks exactly the one under the caret.
  const Segmenter = (Intl as unknown as { Segmenter?: typeof Intl.Segmenter }).Segmenter;
  if (Segmenter) {
    const segmenter = new Segmenter(undefined, { granularity: "word" });
    for (const part of segmenter.segment(text)) {
      const partEnd = part.index + part.segment.length;
      if (part.isWordLike && offset >= part.index && offset <= partEnd) {
        const range = document.createRange();
        range.setStart(node, part.index);
        range.setEnd(node, partEnd);
        return range;
      }
    }
  }

  // Fallback: expand over the text node by whitespace.
  const isWord = (char: string | undefined) => Boolean(char) && !/\s/.test(char as string);
  let start = offset;
  let end = offset;
  while (start > 0 && isWord(text[start - 1])) start--;
  while (end < text.length && isWord(text[end])) end++;
  if (start === end) return null;
  const range = document.createRange();
  range.setStart(node, start);
  range.setEnd(node, end);
  return range;
}
