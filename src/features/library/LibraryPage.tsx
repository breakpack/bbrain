import { open } from "@tauri-apps/plugin-dialog";
import {
  Check,
  FileUp,
  FolderPlus,
  Grid2x2,
  List,
  Plus,
  Search,
  Star,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Card, CardDescription, CardTitle, Eyebrow } from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { Select } from "@/components/ui/Select";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/ipc";
import type {
  ImportOutcome,
  LibraryQuery,
  LibrarySort,
  LibraryView,
  Paper,
} from "@/lib/types";
import {
  useCreateGroup,
  useDeleteGroup,
  useDeletePaper,
  useGroups,
  useImportPapers,
  usePapers,
  useTags,
  useUpdatePaper,
} from "./queries";
import { StatusBadge } from "./status";
import { Thumbnail } from "./Thumbnail";

const VIEWS: Array<{ id: LibraryView; label: string }> = [
  { id: "all", label: "모든 논문" },
  { id: "inbox", label: "Inbox" },
  { id: "favorites", label: "즐겨찾기" },
  { id: "processing", label: "처리 중" },
  { id: "failed", label: "실패" },
];

const SORTS: Array<{ value: LibrarySort; label: string }> = [
  { value: "recent", label: "최근 가져온 순" },
  { value: "opened", label: "최근 열람 순" },
  { value: "title", label: "제목" },
  { value: "year", label: "출판 연도" },
];

export function LibraryPage({ onOpenPaper }: { onOpenPaper: (paperId: string) => void }) {
  const [view, setView] = useState<LibraryView>("all");
  const [groupId, setGroupId] = useState<string | undefined>();
  const [tagIds, setTagIds] = useState<string[]>([]);
  const [sort, setSort] = useState<LibrarySort>("recent");
  const [search, setSearch] = useState("");
  const [layout, setLayout] = useState<"list" | "icon">(
    () => (localStorage.getItem("bbrain.library.layout") as "list" | "icon") ?? "list",
  );
  const [report, setReport] = useState<ImportOutcome[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    localStorage.setItem("bbrain.library.layout", layout);
  }, [layout]);

  const query: LibraryQuery = useMemo(
    () => ({
      view,
      groupId,
      tagIds: tagIds.length > 0 ? tagIds : undefined,
      sort,
      search: search.trim() || undefined,
    }),
    [view, groupId, tagIds, sort, search],
  );

  const papers = usePapers(query);
  const groups = useGroups();
  const tags = useTags();
  const importPapers = useImportPapers();
  const updatePaper = useUpdatePaper();

  /** Files an existing paper into a group; already a member is a quiet no-op. */
  const addToGroup = useCallback(
    (paper: Paper, targetGroupId: string) => {
      if (paper.groupIds.includes(targetGroupId)) return;
      setError(null);
      updatePaper.mutate(
        { paperId: paper.id, patch: { groupIds: [...paper.groupIds, targetGroupId] } },
        { onError: (cause) => setError(errorMessage(cause)) },
      );
    },
    [updatePaper],
  );

  const paperDrag = useDragToGroup(addToGroup);

  const runImport = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setError(null);
      try {
        setReport(await importPapers.mutateAsync({ paths, groupId }));
      } catch (cause) {
        setError(errorMessage(cause));
      }
    },
    [importPapers, groupId],
  );

  const pickFiles = async () => {
    const selected = await open({
      multiple: true,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!selected) return;
    await runImport(Array.isArray(selected) ? selected : [selected]);
  };

  const dropZone = useFileDrop(runImport);

  return (
    <div className="flex h-full">
      <aside
        aria-label="라이브러리 필터"
        className="flex w-[240px] shrink-0 flex-col gap-lg overflow-y-auto border-r border-line bg-canvas-soft p-md"
      >
        <nav className="flex flex-col gap-xs" aria-label="시스템 뷰">
          {VIEWS.map((item) => (
            <FilterButton
              key={item.id}
              label={item.label}
              active={view === item.id && !groupId}
              onClick={() => {
                setView(item.id);
                setGroupId(undefined);
              }}
            />
          ))}
        </nav>

        <GroupList
          activeGroupId={groupId}
          dropTargetId={paperDrag.drag?.overGroupId ?? null}
          onSelect={(id) => {
            setGroupId(id);
            setView("all");
          }}
        />

        {(tags.data?.length ?? 0) > 0 && (
          <section className="flex flex-col gap-sm">
            <h2 className="px-2 text-caption font-medium text-ink-body">태그</h2>
            <div className="flex flex-wrap gap-1.5 px-2">
              {tags.data?.map((tag) => {
                const active = tagIds.includes(tag.id);
                return (
                  <button
                    key={tag.id}
                    aria-pressed={active}
                    onClick={() =>
                      setTagIds((current) =>
                        active
                          ? current.filter((id) => id !== tag.id)
                          : [...current, tag.id],
                      )
                    }
                    className={cn(
                      "rounded-sm border px-2 py-1 text-caption transition-colors duration-fast",
                      active
                        ? "border-primary bg-primary-soft text-primary"
                        : "border-line bg-canvas text-ink hover:border-primary",
                    )}
                  >
                    {tag.displayName}
                  </button>
                );
              })}
            </div>
          </section>
        )}
      </aside>

      <main
        {...dropZone.handlers}
        className={cn(
          "relative flex-1 overflow-y-auto p-xl",
          dropZone.over && "bg-primary-soft/40",
        )}
      >
        <header className="mb-lg flex flex-col gap-md">
          <div className="flex items-center justify-between gap-md">
            <div className="flex flex-col gap-xs">
              <Eyebrow>라이브러리</Eyebrow>
              <h1 className="text-subheading text-ink-heading">
                {groupId
                  ? groups.data?.find((group) => group.id === groupId)?.name
                  : VIEWS.find((item) => item.id === view)?.label}
              </h1>
            </div>

            <div className="flex items-center gap-sm">
              <div className="flex rounded-control border border-line">
                <IconToggle
                  label="목록 보기"
                  active={layout === "list"}
                  onClick={() => setLayout("list")}
                >
                  <List aria-hidden className="h-[18px] w-[18px]" />
                </IconToggle>
                <IconToggle
                  label="아이콘 보기"
                  active={layout === "icon"}
                  onClick={() => setLayout("icon")}
                >
                  <Grid2x2 aria-hidden className="h-[18px] w-[18px]" />
                </IconToggle>
              </div>

              <Button onClick={pickFiles} loading={importPapers.isPending}>
                <FileUp aria-hidden className="h-[18px] w-[18px]" />
                PDF 가져오기
              </Button>
            </div>
          </div>

          <div className="flex items-end gap-md">
            <div className="relative flex-1">
              <Search
                aria-hidden
                className="pointer-events-none absolute left-3 top-[13px] h-[18px] w-[18px] text-ink-body"
              />
              <Input
                className="pl-10"
                type="search"
                aria-label="논문 검색"
                placeholder="제목, 저자, 초록 검색"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
              />
            </div>
            <div className="w-[200px]">
              <Select
                label="정렬"
                value={sort}
                options={SORTS}
                onChange={(value) => setSort(value as LibrarySort)}
              />
            </div>
          </div>
        </header>

        {error && (
          <p role="alert" className="mb-md text-caption text-danger">
            {error}
          </p>
        )}

        {report && <ImportReport report={report} onDismiss={() => setReport(null)} />}

        {papers.isPending && <ListSkeleton />}

        {papers.isSuccess && papers.data.length === 0 && (
          <EmptyState view={view} onImport={pickFiles} />
        )}

        {papers.isSuccess && papers.data.length > 0 && (
          layout === "list" ? (
            <PaperList papers={papers.data} onOpen={onOpenPaper} onDragStart={paperDrag.start} />
          ) : (
            <PaperGrid papers={papers.data} onOpen={onOpenPaper} onDragStart={paperDrag.start} />
          )
        )}

        {dropZone.over && (
          <div className="pointer-events-none absolute inset-4 rounded-card border-2 border-dashed border-primary" />
        )}
      </main>

      {paperDrag.drag && (
        <div
          aria-hidden
          className="pointer-events-none fixed z-30 max-w-[280px] truncate rounded-control border border-line bg-canvas px-3 py-1.5 text-caption text-ink shadow-card"
          style={{ left: paperDrag.drag.x + 12, top: paperDrag.drag.y + 12 }}
        >
          {paperDrag.drag.paper.title}
        </div>
      )}
    </div>
  );
}

/**
 * Mouse-driven drag of a paper onto a sidebar group. HTML5 drag-and-drop can't
 * be used here: Tauri keeps the webview's native drag interception enabled for
 * OS file drops, which swallows `draggable` events on macOS. A drag starts
 * once the pointer moves past a small threshold, so plain clicks on the row
 * (open, favourite, delete) keep working; the click after a real drag is
 * suppressed. Drop targets are the sidebar entries carrying `data-group-id`.
 */
function useDragToGroup(onDrop: (paper: Paper, groupId: string) => void) {
  const [drag, setDrag] = useState<{
    paper: Paper;
    x: number;
    y: number;
    overGroupId: string | null;
  } | null>(null);

  const start = useCallback(
    (paper: Paper) => (event: React.MouseEvent) => {
      if (event.button !== 0) return;
      const originX = event.clientX;
      const originY = event.clientY;
      let active = false;

      const groupUnder = (x: number, y: number) =>
        document
          .elementFromPoint(x, y)
          ?.closest("[data-group-id]")
          ?.getAttribute("data-group-id") ?? null;

      const onMove = (move: MouseEvent) => {
        if (!active) {
          if (Math.hypot(move.clientX - originX, move.clientY - originY) < 5) return;
          active = true;
          document.body.style.userSelect = "none";
          window.getSelection()?.removeAllRanges();
        }
        setDrag({
          paper,
          x: move.clientX,
          y: move.clientY,
          overGroupId: groupUnder(move.clientX, move.clientY),
        });
      };

      const onUp = (up: MouseEvent) => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        if (active) {
          document.body.style.userSelect = "";
          // The mouseup completes a click on whatever the pointer is over —
          // swallow it so a drop doesn't also open a paper or select a group.
          window.addEventListener(
            "click",
            (click) => {
              click.stopPropagation();
              click.preventDefault();
            },
            { capture: true, once: true },
          );
          const groupId = groupUnder(up.clientX, up.clientY);
          if (groupId) onDrop(paper, groupId);
        }
        setDrag(null);
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [onDrop],
  );

  return { drag, start };
}

/**
 * Tauri delivers OS file drops as a window event, not a DOM drag event, so the
 * paths arrive from the backend rather than from `DataTransfer`.
 */
function useFileDrop(onDrop: (paths: string[]) => void) {
  const [over, setOver] = useState(false);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    void import("@tauri-apps/api/webviewWindow").then(({ getCurrentWebviewWindow }) => {
      void getCurrentWebviewWindow()
        .onDragDropEvent((event) => {
          if (event.payload.type === "over") setOver(true);
          else if (event.payload.type === "leave") setOver(false);
          else if (event.payload.type === "drop") {
            setOver(false);
            const paths = event.payload.paths.filter((path) =>
              path.toLowerCase().endsWith(".pdf"),
            );
            onDropRef.current(paths);
          }
        })
        .then((off) => {
          unlisten = off;
        });
    });

    return () => unlisten?.();
  }, []);

  return { over, handlers: {} };
}

function ImportReport({
  report,
  onDismiss,
}: {
  report: ImportOutcome[];
  onDismiss: () => void;
}) {
  const imported = report.filter((item) => item.outcome === "imported").length;
  const duplicates = report.filter((item) => item.outcome === "duplicate").length;
  const rejected = report.filter((item) => item.outcome === "rejected");

  return (
    <Card className="mb-lg flex flex-col gap-md p-md">
      <div className="flex items-center justify-between gap-md">
        <p className="text-caption text-ink">
          {imported}개를 가져왔습니다
          {duplicates > 0 && `. 이미 있는 논문 ${duplicates}개는 다시 복사하지 않았습니다`}
          {rejected.length > 0 && `. ${rejected.length}개는 가져오지 못했습니다`}
        </p>
        <Button variant="ghost" size="sm" onClick={onDismiss}>
          닫기
        </Button>
      </div>

      {rejected.length > 0 && (
        <ul className="flex flex-col gap-xs">
          {rejected.map((item, index) => (
            <li key={index} className="text-caption text-danger">
              {item.outcome === "rejected" && `${item.fileName} — ${item.message}`}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

function PaperList({
  papers,
  onOpen,
  onDragStart,
}: {
  papers: Paper[];
  onOpen: (id: string) => void;
  onDragStart: (paper: Paper) => (event: React.MouseEvent) => void;
}) {
  return (
    <table className="w-full border-collapse">
      <thead>
        <tr className="border-b border-line text-left">
          {["제목", "저자", "연도", "태그", "상태", ""].map((heading, index) => (
            <th
              key={index}
              scope="col"
              className="pb-sm text-caption font-medium text-ink-body"
            >
              {heading}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {papers.map((paper) => (
          <PaperRow key={paper.id} paper={paper} onOpen={onOpen} onDragStart={onDragStart} />
        ))}
      </tbody>
    </table>
  );
}

function PaperRow({
  paper,
  onOpen,
  onDragStart,
}: {
  paper: Paper;
  onOpen: (id: string) => void;
  onDragStart: (paper: Paper) => (event: React.MouseEvent) => void;
}) {
  const updatePaper = useUpdatePaper();
  const deletePaper = useDeletePaper();
  const [confirming, setConfirming] = useState(false);

  if (confirming) {
    return (
      <tr className="border-b border-line bg-canvas-soft">
        <td colSpan={6} className="py-md">
          <div className="flex items-center justify-between gap-md">
            <p className="text-caption text-ink">
              <span className="font-medium">{paper.title}</span>을(를) 삭제하면 하이라이트와
              분석도 함께 사라집니다. 원본 PDF 파일도 앱 저장소에서 지웁니다.
            </p>
            <div className="flex shrink-0 gap-sm">
              <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
                취소
              </Button>
              <Button
                variant="dark"
                size="sm"
                loading={deletePaper.isPending}
                onClick={() =>
                  deletePaper.mutate({ paperId: paper.id, deleteFile: true })
                }
              >
                삭제
              </Button>
            </div>
          </div>
        </td>
      </tr>
    );
  }

  return (
    <tr
      onMouseDown={onDragStart(paper)}
      className="group/row cursor-grab border-b border-line transition-colors duration-fast hover:bg-canvas-soft"
    >
      <td className="py-md pr-md">
        <button
          onClick={() => onOpen(paper.id)}
          className="text-left text-caption font-medium text-ink-heading hover:text-primary"
        >
          {paper.title}
        </button>
      </td>
      <td className="py-md pr-md text-caption text-ink-body">
        {paper.authors.slice(0, 2).join(", ") || "—"}
        {paper.authors.length > 2 && ` 외 ${paper.authors.length - 2}명`}
      </td>
      <td className="py-md pr-md text-caption text-ink-body">{paper.year ?? "—"}</td>
      <td className="py-md pr-md">
        <div className="flex flex-wrap gap-1">
          {paper.tags.slice(0, 3).map((tag) => (
            <Badge key={tag.id}>{tag.displayName}</Badge>
          ))}
        </div>
      </td>
      <td className="py-md pr-md">
        <StatusBadge status={paper.importStatus} />
      </td>
      <td className="py-md">
        <div className="flex items-center gap-1">
          <GroupMenu paper={paper} />
          <button
            aria-label={paper.isFavorite ? "즐겨찾기 해제" : "즐겨찾기에 추가"}
            aria-pressed={paper.isFavorite}
            onClick={() =>
              updatePaper.mutate({
                paperId: paper.id,
                patch: { isFavorite: !paper.isFavorite },
              })
            }
            className="rounded-sm p-1 text-ink-body transition-colors duration-fast hover:text-primary"
          >
            <Star
              aria-hidden
              className={cn("h-[18px] w-[18px]", paper.isFavorite && "fill-primary text-primary")}
            />
          </button>

          <button
            aria-label={`${paper.title} 삭제`}
            onClick={() => setConfirming(true)}
            className="rounded-sm p-1 text-ink-body opacity-0 transition-opacity duration-fast hover:text-danger focus-visible:opacity-100 group-hover/row:opacity-100"
          >
            <Trash2 aria-hidden className="h-[18px] w-[18px]" />
          </button>
        </div>
      </td>
    </tr>
  );
}

function PaperGrid({
  papers,
  onOpen,
  onDragStart,
}: {
  papers: Paper[];
  onOpen: (id: string) => void;
  onDragStart: (paper: Paper) => (event: React.MouseEvent) => void;
}) {
  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-lg">
      {papers.map((paper) => (
        <li key={paper.id} onMouseDown={onDragStart(paper)} className="cursor-grab">
          <button
            onClick={() => onOpen(paper.id)}
            className="flex w-full flex-col gap-sm rounded-card p-2 text-left transition-shadow duration-fast hover:shadow-card"
          >
            <Thumbnail paperId={paper.id} title={paper.title} />
            <p className="line-clamp-2 text-caption font-medium text-ink-heading">
              {paper.title}
            </p>
            <div className="flex items-center justify-between gap-2">
              <span className="text-caption text-ink-body">{paper.year ?? ""}</span>
              <StatusBadge status={paper.importStatus} />
            </div>
          </button>
        </li>
      ))}
    </ul>
  );
}

function GroupList({
  activeGroupId,
  dropTargetId,
  onSelect,
}: {
  activeGroupId?: string;
  /** Group currently under a dragged paper — shown as the drop target. */
  dropTargetId: string | null;
  onSelect: (groupId: string) => void;
}) {
  const groups = useGroups();
  const createGroup = useCreateGroup();
  const deleteGroup = useDeleteGroup();
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");

  return (
    <section className="flex flex-col gap-sm">
      <div className="flex items-center justify-between px-2">
        <h2 className="text-caption font-medium text-ink-body">그룹</h2>
        <button
          aria-label="그룹 추가"
          onClick={() => setAdding(true)}
          className="rounded-sm p-1 text-ink-body hover:text-primary"
        >
          <Plus aria-hidden className="h-4 w-4" />
        </button>
      </div>

      {adding && (
        <form
          className="px-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (!name.trim()) return;
            createGroup.mutate(name.trim());
            setName("");
            setAdding(false);
          }}
        >
          <Input
            autoFocus
            aria-label="새 그룹 이름"
            placeholder="그룹 이름"
            value={name}
            onChange={(event) => setName(event.target.value)}
            onBlur={() => setAdding(false)}
          />
        </form>
      )}

      <ul className="flex flex-col gap-xs">
        {groups.data?.map((group) => (
          <li
            key={group.id}
            data-group-id={group.id}
            className={cn(
              "group/item flex items-center rounded-control",
              dropTargetId === group.id && "outline outline-2 outline-primary",
            )}
          >
            <FilterButton
              className="flex-1"
              label={group.name}
              count={group.paperCount}
              active={activeGroupId === group.id}
              onClick={() => onSelect(group.id)}
            />
            <button
              aria-label={`${group.name} 그룹 삭제`}
              onClick={() => deleteGroup.mutate(group.id)}
              className="rounded-sm p-1 text-ink-body opacity-0 transition-opacity duration-fast hover:text-danger focus-visible:opacity-100 group-hover/item:opacity-100"
            >
              <Trash2 aria-hidden className="h-4 w-4" />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * Per-paper group membership menu: a folder button opening a checkable list of
 * every group. Selecting a group toggles the paper in or out of it — the
 * button-based counterpart to dragging the paper onto a sidebar group (§8.3).
 */
function GroupMenu({ paper }: { paper: Paper }) {
  const [open, setOpen] = useState(false);
  const groups = useGroups();
  const updatePaper = useUpdatePaper();
  const rootRef = useRef<HTMLDivElement>(null);

  // Light dismiss: a click anywhere else, or Escape, closes the menu.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const toggle = (targetGroupId: string) => {
    const member = paper.groupIds.includes(targetGroupId);
    updatePaper.mutate({
      paperId: paper.id,
      patch: {
        groupIds: member
          ? paper.groupIds.filter((id) => id !== targetGroupId)
          : [...paper.groupIds, targetGroupId],
      },
    });
  };

  return (
    <div ref={rootRef} className="relative" onMouseDown={(event) => event.stopPropagation()}>
      <button
        aria-label={`${paper.title} 그룹에 추가`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
        className={cn(
          "rounded-sm p-1 text-ink-body transition-opacity duration-fast hover:text-primary focus-visible:opacity-100 group-hover/row:opacity-100",
          open ? "opacity-100 text-primary" : "opacity-0",
        )}
      >
        <FolderPlus aria-hidden className="h-[18px] w-[18px]" />
      </button>

      {open && (
        <div
          role="menu"
          aria-label="그룹 선택"
          className="absolute right-0 top-full z-20 mt-1 w-[200px] rounded-control border border-line bg-canvas p-1 shadow-card"
        >
          {(groups.data?.length ?? 0) === 0 ? (
            <p className="px-2 py-1.5 text-caption text-ink-body">
              그룹이 없습니다. 사이드바의 +로 먼저 그룹을 만드세요.
            </p>
          ) : (
            groups.data?.map((group) => {
              const member = paper.groupIds.includes(group.id);
              return (
                <button
                  key={group.id}
                  role="menuitemcheckbox"
                  aria-checked={member}
                  disabled={updatePaper.isPending}
                  onClick={() => toggle(group.id)}
                  className="flex w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-left text-caption text-ink transition-colors duration-fast hover:bg-canvas-soft disabled:opacity-50"
                >
                  <span className="truncate">{group.name}</span>
                  {member && <Check aria-hidden className="h-4 w-4 shrink-0 text-primary" />}
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

function FilterButton({
  label,
  count,
  active,
  onClick,
  className,
}: {
  label: string;
  count?: number;
  active: boolean;
  onClick: () => void;
  className?: string;
}) {
  return (
    <button
      onClick={onClick}
      aria-current={active ? "true" : undefined}
      className={cn(
        "flex items-center justify-between rounded-control px-3 py-2 text-nav",
        "transition-colors duration-fast ease-standard",
        active ? "bg-canvas text-primary shadow-card" : "text-ink hover:bg-canvas",
        className,
      )}
    >
      <span>{label}</span>
      {count !== undefined && <span className="text-caption text-ink-body">{count}</span>}
    </button>
  );
}

function IconToggle({
  label,
  active,
  onClick,
  children,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "p-2 transition-colors duration-fast first:rounded-l-control last:rounded-r-control",
        active ? "bg-canvas-soft text-primary" : "text-ink-body hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function EmptyState({ view, onImport }: { view: LibraryView; onImport: () => void }) {
  const copy: Record<LibraryView, { title: string; description: string }> = {
    all: {
      title: "아직 가져온 논문이 없습니다",
      description: "PDF를 이 창에 끌어다 놓거나 가져오기 버튼을 누르세요.",
    },
    inbox: {
      title: "Inbox가 비어 있습니다",
      description: "그룹에 넣지 않은 논문이 여기에 모입니다.",
    },
    favorites: {
      title: "즐겨찾기한 논문이 없습니다",
      description: "목록에서 별 표시를 누르면 여기에 모입니다.",
    },
    processing: {
      title: "처리 중인 논문이 없습니다",
      description: "가져오기와 분석이 모두 끝났습니다.",
    },
    failed: {
      title: "실패한 논문이 없습니다",
      description: "모든 논문이 정상적으로 처리됐습니다.",
    },
  };

  return (
    <div className="flex flex-col items-center gap-md py-section text-center">
      <CardTitle>{copy[view].title}</CardTitle>
      <CardDescription>{copy[view].description}</CardDescription>
      {view === "all" && (
        <Button onClick={onImport}>
          <FileUp aria-hidden className="h-[18px] w-[18px]" />
          PDF 가져오기
        </Button>
      )}
    </div>
  );
}

function ListSkeleton() {
  return (
    <div className="flex flex-col gap-sm" aria-busy="true" aria-label="불러오는 중">
      {[0, 1, 2, 3].map((row) => (
        <div key={row} className="h-12 animate-pulse rounded-sm bg-canvas-soft" />
      ))}
    </div>
  );
}
