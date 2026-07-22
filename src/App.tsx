import { AlertCircle, Library, Network, Settings as SettingsIcon, Telescope } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle } from "@/components/ui/Card";
import { Chat } from "@/features/chat/Chat";
import { DiscoverPage } from "@/features/discover/DiscoverPage";
import { GraphPage } from "@/features/graph/GraphPage";
import { startExtractionWorker } from "@/features/jobs/extractionWorker";
import { LibraryPage } from "@/features/library/LibraryPage";
import { useLibraryEvents } from "@/features/library/queries";
import { Onboarding } from "@/features/onboarding/Onboarding";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { useSettings } from "@/features/settings/queries";
import { ViewerPage } from "@/features/viewer/ViewerPage";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/ipc";

type Route =
  | { name: "library" }
  | { name: "discover" }
  | { name: "graph" }
  | { name: "settings" }
  | { name: "viewer"; paperId: string };

export function App() {
  const settings = useSettings();
  const [route, setRoute] = useState<Route>({ name: "library" });

  useLibraryEvents();

  // PDF.js lives in the webview, so the Rust job runner delegates extraction and
  // thumbnails here. The worker must be listening before any import lands.
  useEffect(() => startExtractionWorker(), []);

  if (settings.isPending) {
    return <BootSkeleton />;
  }

  if (settings.isError) {
    return (
      <main className="flex min-h-full items-center justify-center bg-canvas-soft p-section">
        <Card className="flex max-w-[480px] flex-col gap-md">
          <AlertCircle aria-hidden className="h-8 w-8 text-danger" />
          <CardTitle>Bbrain을 시작하지 못했습니다</CardTitle>
          <CardDescription>{errorMessage(settings.error)}</CardDescription>
          <div>
            <Button onClick={() => void settings.refetch()}>다시 시도</Button>
          </div>
        </Card>
      </main>
    );
  }

  if (settings.data.onboardingCompletedAt === null) {
    return <Onboarding settings={settings.data} />;
  }

  // The viewer takes the whole window: the paper is the point (DESIGN.md §5).
  if (route.name === "viewer") {
    return (
      <>
        <ViewerPage
          paperId={route.paperId}
          onBack={() => setRoute({ name: "library" })}
        />
        <Chat
          scope={{ type: "paper", id: route.paperId }}
          onOpenCitation={(paperId) => setRoute({ name: "viewer", paperId })}
        />
      </>
    );
  }

  return (
    <div className="flex h-full">
      <nav
        aria-label="주요 메뉴"
        className="flex w-[220px] shrink-0 flex-col gap-xs border-r border-line bg-canvas-soft p-md"
      >
        <p className="px-2 py-md text-body font-bold text-ink-heading">Bbrain</p>
        <NavItem
          icon={<Library aria-hidden className="h-[18px] w-[18px]" />}
          label="라이브러리"
          active={route.name === "library"}
          onClick={() => setRoute({ name: "library" })}
        />
        <NavItem
          icon={<Telescope aria-hidden className="h-[18px] w-[18px]" />}
          label="논문 찾기"
          active={route.name === "discover"}
          onClick={() => setRoute({ name: "discover" })}
        />
        <NavItem
          icon={<Network aria-hidden className="h-[18px] w-[18px]" />}
          label="관계 그래프"
          active={route.name === "graph"}
          onClick={() => setRoute({ name: "graph" })}
        />
        <NavItem
          icon={<SettingsIcon aria-hidden className="h-[18px] w-[18px]" />}
          label="설정"
          active={route.name === "settings"}
          onClick={() => setRoute({ name: "settings" })}
        />
      </nav>

      <main className="min-w-0 flex-1 overflow-hidden">
        {route.name === "library" && (
          <LibraryPage
            onOpenPaper={(paperId) => setRoute({ name: "viewer", paperId })}
          />
        )}
        {route.name === "discover" && (
          <DiscoverPage
            onOpenPaper={(paperId) => setRoute({ name: "viewer", paperId })}
          />
        )}
        {route.name === "graph" && (
          <GraphPage onOpenPaper={(paperId) => setRoute({ name: "viewer", paperId })} />
        )}
        {route.name === "settings" && <SettingsPage settings={settings.data} />}
      </main>

      <Chat
        scope={{ type: "library" }}
        onOpenCitation={(paperId) => setRoute({ name: "viewer", paperId })}
      />
    </div>
  );
}

function NavItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex items-center gap-md rounded-control px-3 py-2 text-nav",
        "transition-colors duration-fast ease-standard",
        active ? "bg-canvas text-primary shadow-card" : "text-ink hover:bg-canvas",
      )}
    >
      {icon}
      {label}
    </button>
  );
}

function BootSkeleton() {
  return (
    <div className="flex h-full" aria-busy="true" aria-label="불러오는 중">
      <div className="w-[220px] shrink-0 border-r border-line bg-canvas-soft" />
      <div className="flex-1 p-xl">
        <div className="mx-auto flex max-w-[840px] flex-col gap-lg">
          <div className="h-6 w-40 animate-pulse rounded-sm bg-canvas-soft" />
          <div className="h-[220px] animate-pulse rounded-card bg-canvas-soft" />
          <div className="h-[160px] animate-pulse rounded-card bg-canvas-soft" />
        </div>
      </div>
    </div>
  );
}
