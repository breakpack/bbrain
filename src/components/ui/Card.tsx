import type { HTMLAttributes, ReactNode } from "react";

import { cn } from "@/lib/cn";

export type CardProps = HTMLAttributes<HTMLDivElement> & {
  tinted?: boolean;
};

export function Card({ tinted = false, className, ...props }: CardProps) {
  return (
    <div
      className={cn(
        "rounded-card p-lg shadow-card",
        tinted ? "bg-canvas-tint" : "bg-canvas",
        className,
      )}
      {...props}
    />
  );
}

export function Eyebrow({ children }: { children: ReactNode }) {
  return <p className="text-eyebrow font-bold text-primary">{children}</p>;
}

export function CardTitle({ children }: { children: ReactNode }) {
  return <h2 className="text-subheading text-ink-heading">{children}</h2>;
}

export function CardDescription({ children }: { children: ReactNode }) {
  return <p className="text-caption text-ink-body">{children}</p>;
}
