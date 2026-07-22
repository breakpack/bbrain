import type { ReactNode } from "react";

import { cn } from "@/lib/cn";

type Tone = "neutral" | "primary" | "danger";

// Status is carried by icon + text, never color alone (DEVELOPMENT.md §15).
const TONES: Record<Tone, string> = {
  neutral: "bg-canvas-soft text-ink border-line",
  primary: "bg-primary-soft text-primary border-primary/30",
  danger: "bg-canvas-soft text-danger border-danger/30",
};

export function Badge({
  tone = "neutral",
  icon,
  children,
}: {
  tone?: Tone;
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-sm border px-2 py-1 text-caption",
        TONES[tone],
      )}
    >
      {icon}
      {children}
    </span>
  );
}
