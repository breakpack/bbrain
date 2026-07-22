import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// Radix Select relies on pointer-capture and scrollIntoView, which jsdom lacks.
if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
}
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = vi.fn();
}

// pdf.js touches these canvas APIs at import time; jsdom has none of them. The
// tests never render a PDF, so a stub is enough to let the module load.
const globals = globalThis as Record<string, unknown>;
globals.DOMMatrix ??= class DOMMatrix {
  constructor(readonly init?: unknown) {}
};
globals.Path2D ??= class Path2D {};
globals.ImageData ??= class ImageData {};

// pdf.js 6 uses the Uint8Array hex/base64 methods, which Node 23 does not ship
// yet (the webview does). Without them the fake worker throws on load.
const view = Uint8Array.prototype as Uint8Array & {
  toHex?: () => string;
  toBase64?: () => string;
};
view.toHex ??= function toHex(this: Uint8Array) {
  return Array.from(this, (byte) => byte.toString(16).padStart(2, "0")).join("");
};
view.toBase64 ??= function toBase64(this: Uint8Array) {
  return Buffer.from(this).toString("base64");
};

if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
