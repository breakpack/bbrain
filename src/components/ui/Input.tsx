import { forwardRef, useId, type InputHTMLAttributes, type ReactNode } from "react";

import { cn } from "@/lib/cn";

export type InputProps = InputHTMLAttributes<HTMLInputElement> & {
  label?: string;
  hint?: ReactNode;
  error?: string;
};

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ label, hint, error, className, id, ...props }, ref) => {
    const generatedId = useId();
    const inputId = id ?? generatedId;
    const hintId = `${inputId}-hint`;
    const errorId = `${inputId}-error`;

    return (
      <div className="flex flex-col gap-sm">
        {label && (
          <label htmlFor={inputId} className="text-caption font-medium text-ink">
            {label}
          </label>
        )}
        <input
          ref={ref}
          id={inputId}
          aria-invalid={error ? true : undefined}
          aria-describedby={cn(hint && hintId, error && errorId) || undefined}
          className={cn(
            "rounded-control border bg-canvas px-md py-[10px] text-[15px] text-ink",
            "placeholder:text-ink-body",
            "transition-colors duration-fast ease-standard",
            "focus:border-primary focus:outline-none focus-visible:outline-none",
            error ? "border-danger" : "border-line",
            className,
          )}
          {...props}
        />
        {hint && !error && (
          <p id={hintId} className="text-caption text-ink-body">
            {hint}
          </p>
        )}
        {error && (
          <p id={errorId} role="alert" className="text-caption text-danger">
            {error}
          </p>
        )}
      </div>
    );
  },
);

Input.displayName = "Input";
