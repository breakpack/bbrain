import {
  AlertCircle,
  CheckCircle2,
  CircleDashed,
  KeyRound,
  Loader2,
  type LucideIcon,
} from "lucide-react";

import { Badge } from "@/components/ui/Badge";
import type { ImportStatus } from "@/lib/types";

/**
 * Status is carried by icon *and* text, never by color alone, and progress uses
 * neutral tones — green stays reserved for the brand accent (DEVELOPMENT.md §15).
 */
const STATUS: Record<
  ImportStatus,
  { label: string; icon: LucideIcon; tone: "neutral" | "primary" | "danger"; spin?: boolean }
> = {
  copying: { label: "복사 중", icon: Loader2, tone: "neutral", spin: true },
  extracting: { label: "본문 추출 중", icon: Loader2, tone: "neutral", spin: true },
  indexing: { label: "색인 중", icon: Loader2, tone: "neutral", spin: true },
  waiting_for_ai: { label: "API 키 대기", icon: KeyRound, tone: "neutral" },
  analyzing: { label: "분석 중", icon: Loader2, tone: "neutral", spin: true },
  ready: { label: "완료", icon: CheckCircle2, tone: "primary" },
  partial: { label: "일부 완료", icon: CircleDashed, tone: "neutral" },
  failed: { label: "실패", icon: AlertCircle, tone: "danger" },
};

export function StatusBadge({ status }: { status: ImportStatus }) {
  const { label, icon: Icon, tone, spin } = STATUS[status];

  return (
    <Badge tone={tone} icon={<Icon aria-hidden className={spin ? "h-4 w-4 animate-spin" : "h-4 w-4"} />}>
      {label}
    </Badge>
  );
}

export function statusLabel(status: ImportStatus): string {
  return STATUS[status].label;
}
