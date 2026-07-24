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
export function detectColumnGutter(items: ExtractedItem[]): number | null {
  const content = items.filter((item) => !isBlank(item));
  if (content.length < 12) return null;

  let best: { gutter: number; band: number } | null = null;

  // A candidate is read as "the right column starts at x = gutter". The
  // emptiness test therefore looks only at the whitespace band JUST LEFT of
  // the candidate: a symmetric window also swept up the right column's own
  // left edge, so layouts whose right column starts near 0.50 (IEEE
  // two-column) rejected every candidate and collapsed into one column.
  for (let gutter = 0.42; gutter <= 0.62; gutter += 0.01) {
    let left = 0;
    let right = 0;
    let band = 0; // left edges starting inside the gap left of the candidate

    for (const item of content) {
      const edge = item.rect.x;
      if (edge >= gutter - 0.045 && edge < gutter - 0.005) band += 1;
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
/** Bottom band that may hold footnote/imprint text (normalized page y). */
const FOOTNOTE_BAND_TOP = 0.88;
/** Footnote text is set visibly smaller than the body. */
const FOOTNOTE_FONT_RATIO = 0.85;

/**
 * Splits off the small-print block at the very bottom of the page — contact
 * lines, license/imprint notices, footnotes. Geometrically those sit at the end
 * of a column, so without this they interrupt the reading order between the
 * columns ("...body, CONTACT, ©2021, keywords, body...") and the translation
 * reads as if the columns were shuffled. Both conditions must hold: inside the
 * bottom band AND set smaller than the body median, so a body paragraph that
 * merely reaches the page bottom stays in place (§9.3).
 */
export function splitFootnoteBand(items: ExtractedItem[]): {
  body: ExtractedItem[];
  footnotes: ExtractedItem[];
} {
  const heights = items
    .filter((item) => !isBlank(item))
    .map((item) => item.rect.height)
    .sort((a, b) => a - b);
  if (heights.length === 0) return { body: items, footnotes: [] };
  const median = heights[Math.floor(heights.length / 2)];

  const body: ExtractedItem[] = [];
  const footnotes: ExtractedItem[] = [];
  for (const item of items) {
    const small = item.rect.height < median * FOOTNOTE_FONT_RATIO;
    (item.rect.y >= FOOTNOTE_BAND_TOP && small ? footnotes : body).push(item);
  }
  return { body, footnotes };
}

function readingColumns(items: ExtractedItem[]): Line[][] {
  // Footnotes leave the flow before column detection, and rejoin at the very
  // end of the page so the body columns read continuously.
  const { body, footnotes } = splitFootnoteBand(items);

  const gutter = detectColumnGutter(body);
  const columns: Line[][] = [];
  if (gutter === null) {
    columns.push(groupIntoLines(body));
  } else {
    const left: ExtractedItem[] = [];
    const right: ExtractedItem[] = [];
    for (const item of body) {
      // A blank filler item straddling the gutter belongs to neither column.
      if (isBlank(item)) continue;
      // Assign by left edge (width-independent): a left-column line starts
      // before the gutter, a right-column line at or after it.
      (item.rect.x < gutter ? left : right).push(item);
    }
    columns.push(groupIntoLines(left), groupIntoLines(right));
  }

  if (footnotes.length > 0) columns.push(groupIntoLines(footnotes));
  return columns;
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

/** A rectangle in client (viewport) coordinates, as `getClientRects` yields. */
export type ClientBox = { top: number; bottom: number; left: number; right: number };

/**
 * How far apart, as a multiple of the line box height, two rects on the same
 * visual line may sit and still be joined into one bar. Inter-word and
 * inter-span spacing (even the stretched spaces of justified text) stays well
 * under one line height; a column gutter is several line heights wide. So this
 * merges the words of a line while a gutter — or a wide indent — starts a new
 * bar instead of one bar spanning the whitespace between columns.
 */
const BAR_MERGE_GAP_RATIO = 1.2;

/**
 * Groups tight per-glyph client rectangles into one bar per *contiguous run* of
 * text on a visual line. Rects that overlap vertically are on the same line;
 * within a line they join only across a small horizontal gap. A large gap — the
 * whitespace of a column gutter, reached because the DOM range spans spans on
 * both sides of it — starts a NEW bar rather than a single bar bridging the
 * gutter into the margin. Splitting is always safe; over-merging is what bleeds
 * past the text (§9.5).
 */
export function mergeClientBoxesIntoBars(boxes: ClientBox[]): ClientBox[] {
  // Left-to-right within each line so a run grows rightward and a gutter gap is
  // seen as the jump from a settled bar's right edge to the next rect's left.
  const sorted = [...boxes].sort((a, b) => a.top - b.top || a.left - b.left);
  const bars: ClientBox[] = [];

  for (const box of sorted) {
    const height = box.bottom - box.top;
    const bar = bars.find((candidate) => {
      const vOverlap =
        Math.min(candidate.bottom, box.bottom) - Math.max(candidate.top, box.top);
      const sameLine = vOverlap > Math.min(candidate.bottom - candidate.top, height) * 0.5;
      if (!sameLine) return false;
      // Gap between the two boxes on the axis; negative when they overlap.
      const gap = Math.max(box.left - candidate.right, candidate.left - box.right);
      return gap <= height * BAR_MERGE_GAP_RATIO;
    });

    if (bar) {
      bar.left = Math.min(bar.left, box.left);
      bar.right = Math.max(bar.right, box.right);
      bar.top = Math.min(bar.top, box.top);
      bar.bottom = Math.max(bar.bottom, box.bottom);
    } else {
      // Copy the four fields explicitly: `box` may be a DOMRect (from
      // getClientRects), whose left/top/right/bottom live on the prototype as
      // getters, so `{ ...box }` would spread to an empty object and every
      // coordinate would read back undefined → NaN downstream.
      bars.push({ top: box.top, bottom: box.bottom, left: box.left, right: box.right });
    }
  }

  return bars;
}

export type LayerGeometry = {
  pageWidth: number;
  pageHeight: number;
  /** Text node → the PDF item it renders: the item's full page-space rect and
   * character count, for interpolating selection offsets into the rect. */
  nodes: Map<Node, { rect: NormalizedRect; length: number }>;
};

const layerGeometry = new WeakMap<HTMLElement, LayerGeometry>();

/** Attach (or clear) item geometry for a rendered layer. Test seam. */
export function setTextLayerGeometry(layer: HTMLElement, geometry: LayerGeometry | null): void {
  if (geometry) layerGeometry.set(layer, geometry);
  else layerGeometry.delete(layer);
}

/**
 * Maps each text node of a rendered text layer to the PDF text item it came
 * from, so selection geometry is computed from item transforms instead of DOM
 * measurement. The transparent DOM text is laid out with fallback fonts whose
 * metrics differ from the embedded fonts painted on the canvas — wider in some
 * faces, narrower in others — so DOM-measured rectangles drift off the ink.
 * Item transforms are identical in every environment; this is the same source
 * the extraction pipeline (and the translation hover overlay) uses (§9.5).
 *
 * Returns whitespace-trimmed glyph rectangles for `clipRectsToGlyphs`.
 * pdf.js's TextLayer renders exactly one text node per non-empty item, in item
 * order; if the DOM ever disagrees, the mapping is dropped and selection falls
 * back to DOM measurement rather than mis-attributing geometry.
 */
export async function registerTextLayerGeometry(
  page: PDFPageProxy,
  layer: HTMLElement,
): Promise<NormalizedRect[]> {
  const { matrix, pageWidth, pageHeight } = pageSpaceMatrix(page);
  const content = await page.getTextContent();
  const styles = content.styles as Record<string, { vertical?: boolean }>;

  const textNodes: Node[] = [];
  const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) textNodes.push(node);

  const nodes = new Map<Node, { rect: NormalizedRect; length: number }>();
  const glyphs: NormalizedRect[] = [];
  let cursor = 0;

  for (const item of content.items) {
    if (!("str" in item) || item.str.length === 0) continue;
    const node = textNodes[cursor];
    if (!node || node.textContent !== item.str) {
      setTextLayerGeometry(layer, null);
      return [];
    }
    cursor++;

    const tx = Util.transform(matrix, item.transform);
    const fontHeight = Math.hypot(tx[2], tx[3]);
    const vertical = styles[item.fontName]?.vertical ?? false;
    const width = vertical ? fontHeight : item.width;
    const height = vertical ? item.height : fontHeight;
    if (width <= 0 || height <= 0) continue;

    const rect: NormalizedRect = {
      x: tx[4] / pageWidth,
      y: (tx[5] - height) / pageHeight,
      width: width / pageWidth,
      height: height / pageHeight,
    };
    nodes.set(node, { rect, length: item.str.length });

    // Tight glyph rect for clipping: shave the whitespace padding off both
    // ends at the item's average character width, so a clip never keeps a
    // trailing space reaching into the gutter.
    const lead = item.str.length - item.str.trimStart().length;
    const trail = item.str.length - item.str.trimEnd().length;
    const inkLen = item.str.length - lead - trail;
    if (inkLen <= 0) continue;
    const per = rect.width / item.str.length;
    glyphs.push({
      x: rect.x + per * lead,
      y: rect.y,
      width: per * inkLen,
      height: rect.height,
    });
  }

  setTextLayerGeometry(layer, { pageWidth, pageHeight, nodes });
  return glyphs;
}

/**
 * Selection rectangles from registered item geometry: each selected text
 * node's character offsets are interpolated into its item's page-space rect at
 * the average character width (only the two drag endpoints are ever partial —
 * interior items are covered whole). Merging happens in page-point units so
 * the bar heuristics see the same proportions as the client-space path.
 */
function selectionToItemRects(
  range: Range,
  layer: HTMLElement,
  geometry: LayerGeometry,
): NormalizedRect[] {
  const { pageWidth, pageHeight, nodes } = geometry;
  const collected: ClientBox[] = [];
  const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!range.intersectsNode(node)) continue;
    const item = nodes.get(node);
    if (!item) continue;

    const text = node.textContent ?? "";
    let start = node === range.startContainer ? range.startOffset : 0;
    let end = node === range.endContainer ? range.endOffset : text.length;
    while (start < end && /\s/.test(text[start])) start++;
    while (end > start && /\s/.test(text[end - 1])) end--;
    if (end <= start) continue;

    const per = (item.rect.width * pageWidth) / item.length;
    collected.push({
      left: item.rect.x * pageWidth + per * start,
      right: item.rect.x * pageWidth + per * end,
      top: item.rect.y * pageHeight,
      bottom: (item.rect.y + item.rect.height) * pageHeight,
    });
  }

  const rects: NormalizedRect[] = [];
  for (const bar of mergeClientBoxesIntoBars(collected)) {
    rects.push({
      x: bar.left / pageWidth,
      y: bar.top / pageHeight,
      width: (bar.right - bar.left) / pageWidth,
      height: (bar.bottom - bar.top) / pageHeight,
    });
  }
  return rects;
}

/**
 * Converts a DOM selection over the text layer into page-space rectangles that
 * hug the text. The selection's own line boxes span the full text-layer width and
 * bleed into the margin, so instead we measure each selected text node's own
 * sub-range — those rectangles are tight to the glyphs — and merge them into one
 * continuous bar per contiguous run of text (§9.5).
 */
export function selectionToRects(range: Range, layer: HTMLElement): NormalizedRect[] {
  // Prefer PDF item geometry when the layer has it registered — exact against
  // the canvas ink in every engine. DOM measurement below is the fallback.
  const geometry = layerGeometry.get(layer);
  if (geometry) return selectionToItemRects(range, layer, geometry);

  const box = layer.getBoundingClientRect();
  if (box.width === 0 || box.height === 0) return [];

  const collected: ClientBox[] = [];
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

  const rects: NormalizedRect[] = [];
  for (const bar of mergeClientBoxesIntoBars(collected)) {
    const x = (bar.left - box.left) / box.width;
    const y = (bar.top - box.top) / box.height;
    if (x < -0.01 || y < -0.01) continue;
    rects.push({
      x,
      y,
      width: (bar.right - bar.left) / box.width,
      height: (bar.bottom - bar.top) / box.height,
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
 * The whitespace-delimited word under a page-normalized point, resolved from
 * the layer's registered item geometry — never from DOM caret positioning
 * (`caretRangeFromPoint` maps the click through the fallback-font layout,
 * which drifts off the canvas ink, so it picks the wrong character or even the
 * wrong word). The character under the point is found by interpolating at the
 * item's average character width, then expanded to the surrounding whitespace
 * boundaries. Used to delete a single word from a highlight (§9.5).
 */
export function wordAtPoint(
  layer: HTMLElement,
  nx: number,
  ny: number,
): { rect: NormalizedRect; text: string } | null {
  const geometry = layerGeometry.get(layer);
  if (!geometry) return null;

  for (const [node, item] of geometry.nodes) {
    const { rect, length } = item;
    if (ny < rect.y || ny > rect.y + rect.height) continue;
    if (nx < rect.x || nx > rect.x + rect.width) continue;

    const text = node.textContent ?? "";
    const per = rect.width / length;
    const offset = Math.min(length - 1, Math.max(0, Math.floor((nx - rect.x) / per)));
    if (/\s/.test(text[offset] ?? "")) return null; // clicked the gap between words

    let start = offset;
    let end = offset + 1;
    while (start > 0 && !/\s/.test(text[start - 1])) start--;
    while (end < length && !/\s/.test(text[end])) end++;

    return {
      text: text.slice(start, end),
      rect: {
        x: rect.x + per * start,
        y: rect.y,
        width: per * (end - start),
        height: rect.height,
      },
    };
  }
  return null;
}
