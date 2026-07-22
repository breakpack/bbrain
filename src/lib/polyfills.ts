/**
 * WebKit (and therefore WKWebView, Bbrain's macOS webview) still does not
 * implement async iteration of `ReadableStream`. pdf.js 6 relies on it —
 * `page.getTextContent()` does `for await (const value of readableStream)` — so
 * without this shim text extraction throws `undefined is not a function` on
 * macOS while working on Windows.
 *
 * The shim follows the WHATWG `ReadableStream.values()` semantics: read until
 * done, and cancel the stream when the loop exits early.
 */
type AsyncIterableStream = ReadableStream & {
  values?: (options?: { preventCancel?: boolean }) => AsyncIterator<unknown>;
  [Symbol.asyncIterator]?: () => AsyncIterator<unknown>;
};

export function installStreamAsyncIterator(): void {
  if (typeof ReadableStream === "undefined") return;

  const prototype = ReadableStream.prototype as AsyncIterableStream;
  if (Symbol.asyncIterator in prototype) return;

  function values(
    this: ReadableStream,
    { preventCancel = false }: { preventCancel?: boolean } = {},
  ): AsyncIterableIterator<unknown> {
    const reader = this.getReader();

    return {
      async next() {
        try {
          const result = await reader.read();
          if (result.done) reader.releaseLock();
          return result as IteratorResult<unknown>;
        } catch (error) {
          reader.releaseLock();
          throw error;
        }
      },

      async return(value?: unknown) {
        if (!preventCancel) {
          const cancelled = reader.cancel(value);
          reader.releaseLock();
          await cancelled;
        } else {
          reader.releaseLock();
        }
        return { done: true, value } as IteratorResult<unknown>;
      },

      [Symbol.asyncIterator]() {
        return this;
      },
    };
  }

  prototype.values = values;
  prototype[Symbol.asyncIterator] = values;
}
