import { FileText } from "lucide-react";
import { useEffect, useState } from "react";

import { api } from "@/lib/ipc";

/**
 * Thumbnails are stored as PNG in app data (WKWebView cannot encode WebP), and
 * read through a command rather than an asset URL so the webview never gets
 * filesystem scope.
 */
export function Thumbnail({ paperId, title }: { paperId: string; title: string }) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let objectUrl: string | null = null;
    let cancelled = false;

    void api
      .readThumbnail(paperId)
      .then((bytes) => {
        if (cancelled || bytes.length === 0) return;
        const copy = new Uint8Array(bytes);
        objectUrl = URL.createObjectURL(new Blob([copy.buffer as ArrayBuffer], { type: "image/png" }));
        setUrl(objectUrl);
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [paperId]);

  if (!url) {
    return (
      <div
        className="flex aspect-[3/4] items-center justify-center rounded-sm bg-canvas-soft"
        aria-hidden
      >
        <FileText className="h-8 w-8 text-ink-subhead" />
      </div>
    );
  }

  return (
    <img
      src={url}
      alt={`${title} 첫 페이지 미리보기`}
      className="aspect-[3/4] w-full rounded-sm border border-line object-cover object-top"
    />
  );
}
