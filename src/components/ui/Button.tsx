import { forwardRef, type ButtonHTMLAttributes } from "react";
import { Loader2 } from "lucide-react";

import { cn } from "@/lib/cn";

type Variant = "primary" | "dark" | "ghost" | "outline";
type Size = "sm" | "md";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: Size;
  loading?: boolean;
};

// Controls stay at 6px radius; green never fills large areas (DESIGN.md §7).
const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-primary text-on-primary border border-primary hover:bg-primary-hover",
  dark: "bg-ink text-white border border-ink hover:opacity-90",
  outline:
    "bg-canvas text-ink border border-line hover:border-primary hover:text-primary",
  ghost: "bg-transparent text-ink border border-transparent hover:bg-canvas-soft",
};

const SIZES: Record<Size, string> = {
  sm: "px-md py-sm text-nav",
  md: "px-lg py-[10px] text-button",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  (
    { variant = "primary", size = "md", loading = false, className, children, disabled, ...props },
    ref,
  ) => (
    <button
      ref={ref}
      // Disabled surfaces fade rather than switching to gray (DESIGN.md §14).
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={cn(
        "inline-flex items-center justify-center gap-2 rounded-control font-medium",
        "transition-colors duration-fast ease-standard",
        "disabled:cursor-not-allowed disabled:opacity-50",
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...props}
    >
      {loading && <Loader2 aria-hidden className="h-[18px] w-[18px] animate-spin" />}
      {children}
    </button>
  ),
);

Button.displayName = "Button";
