import * as RadixSelect from "@radix-ui/react-select";
import { Check, ChevronDown } from "lucide-react";
import { useId } from "react";

import { cn } from "@/lib/cn";

export type SelectOption = {
  value: string;
  label: string;
};

export function Select({
  label,
  value,
  options,
  onChange,
  placeholder = "선택하세요",
  disabled = false,
  hint,
}: {
  label?: string;
  value: string | null;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  hint?: string;
}) {
  const id = useId();

  return (
    <div className="flex flex-col gap-sm">
      {label && (
        <label htmlFor={id} className="text-caption font-medium text-ink">
          {label}
        </label>
      )}
      <RadixSelect.Root
        value={value ?? undefined}
        onValueChange={onChange}
        disabled={disabled}
      >
        <RadixSelect.Trigger
          id={id}
          className={cn(
            "flex items-center justify-between gap-2 rounded-control border border-line",
            "bg-canvas px-md py-[10px] text-[15px] text-ink",
            "transition-colors duration-fast ease-standard",
            "hover:border-primary data-[placeholder]:text-ink-body",
            "disabled:cursor-not-allowed disabled:opacity-50",
          )}
        >
          <RadixSelect.Value placeholder={placeholder} />
          <RadixSelect.Icon>
            <ChevronDown aria-hidden className="h-[18px] w-[18px] text-ink-body" />
          </RadixSelect.Icon>
        </RadixSelect.Trigger>

        <RadixSelect.Portal>
          <RadixSelect.Content
            position="popper"
            sideOffset={4}
            className={cn(
              "z-50 max-h-[320px] overflow-hidden rounded-control border border-line",
              "bg-canvas shadow-card",
            )}
          >
            <RadixSelect.Viewport className="p-1">
              {options.map((option) => (
                <RadixSelect.Item
                  key={option.value}
                  value={option.value}
                  className={cn(
                    "flex cursor-pointer items-center justify-between gap-4 rounded-sm",
                    "px-3 py-2 text-caption text-ink outline-none",
                    "data-[highlighted]:bg-canvas-soft data-[state=checked]:text-primary",
                  )}
                >
                  <RadixSelect.ItemText>{option.label}</RadixSelect.ItemText>
                  <RadixSelect.ItemIndicator>
                    <Check aria-hidden className="h-4 w-4" />
                  </RadixSelect.ItemIndicator>
                </RadixSelect.Item>
              ))}
            </RadixSelect.Viewport>
          </RadixSelect.Content>
        </RadixSelect.Portal>
      </RadixSelect.Root>
      {hint && <p className="text-caption text-ink-body">{hint}</p>}
    </div>
  );
}
