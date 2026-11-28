import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { listCommandFor } from "../lib/keys";
import { onFileDrop } from "../lib/native";
import { appPlatform, primaryModifier } from "../lib/platform";
import { readStorage, writeStorage } from "../lib/storage";
import { useClock } from "../lib/useNow";
import type { StackAgent } from "../state/agentStack";
import { type BranchRow, branchRows } from "../state/branches";
import { type ChildInfo, childInfo } from "../state/children";
import { type DelegateOrigin, detachedDelegates, originFrom } from "../state/delegateOrigin";
import { sessionsListed } from "../state/engine";
import { addDroppedFolders } from "../state/folders";
import { stateOf } from "../state/host";
import { isJobRunThread, useLoopThreads } from "../state/loops";
import { useMachines } from "../state/machines";
import { projectLabel } from "../state/projectBook";
import {
  cachedPulls,
  loadPulls,
  projectFolders,
  pullForBranch,
  pullsVersion,
  subscribePulls,
} from "../state/pulls";
import { pendingSummary } from "../state/setup";
import { byProjectName, type ProjectGroup, sidebarProjects } from "../state/sidebarGroups";
import {
  applyOrder,
  moveBy,
  rememberOrder,
  reorder,
  type SidebarOrder,
  sectionOrderKey,
} from "../state/sidebarOrder";
import {
  PREF,
  SETTLE_AFTER_DEFAULT_DAYS,
  settleAfterDays,
  settleAfterMs,
  usePref,
  useSidebarDensity,
  useSidebarSort,
} from "../state/sidebarPrefs";
import {
  cleanFolds,
  contextRow,
  type HiddenKind,
  hiddenKind,
  isNestedKey,
  isShelf,
  keepListed,
  listedPicks,
  nextRow,
  type OrderKey,
  type OrderRow,
  type OrderSection,
  placeThread,
  rangeRows,
  rowIdOf,
  rowKeyAction,
  type SectionId,
  type ShelfId,
  type SidebarLayout,
  sectionLabelled,
  sectionsOf,
  shelfKey,
  shelfOpen,
  sortSection,
  visibleRows,
} from "../state/sidebarRows";
import {
  selectThread,
  setPinned,
  setSectionOrder,
  setSwitcher,
  settleThreads,
  startNewThread,
  statusOf,
  toast,
  toggleSidebar,
  unsettleThreads,
  useApp,
  useShortcutHint,
  wakeSnoozed,
} from "../state/store";
import { idleSettled, isRunning } from "../state/thread";
import { rememberSidebarOrder } from "../state/threadActions";
import { splitThreadKey } from "../state/threadKeys";
import {
  listedSinceOf,
  openChain,
  parentOf as parentOfThread,
  type SubRow,
  threadCreatedAt,
  topLevel,
} from "../state/topLevel";
import { showsPage } from "../state/viewActions";
import { useWaitingIds } from "../state/waiting";
import { CustomSnooze } from "./CustomSnooze";
import { Fold } from "./Fold";
import { FOLDER_DROP_ATTR, overFolderDrop } from "./folderDropTarget";
import { NeedsYouRow } from "./Inbox";
import { ChevronDown, ChevronRight, CornerDownRight, FolderPlus, PanelLeft, Search } from "./icons";
import { SettingsButton } from "./MainHeader";
import { HiddenDot, ProjectHeader } from "./ProjectHeader";
import { Cluster, ClusterButton, cx, Pill, Skeleton, Tooltip } from "./primitives";
import { RollingNumber } from "./RollingNumber";
import { SettingsNav } from "./Settings";
import { SidebarRowMotion } from "./SidebarRowMotion";
import { SidebarSelection } from "./SidebarSelection";
import { PendingRow, ROW_HEIGHT, ThreadRow } from "./SidebarThreadRow";
import { SidebarToolbar, ThreadSectionLabel } from "./SidebarToolbar";
import { useThreadDrag } from "./sidebarDrag";
import { LIGHTS_INSET, TitlebarRow } from "./Titlebar";
import { UpdateRow } from "./Updates";

// The title row starts past the traffic lights and ends in the sidebar's toggle, so the sidebar
// cannot be narrower than that row: the lights' inset, then room for the toggle.
const MIN = LIGHTS_INSET + 140;
const MAX = 460;
/** What the sidebar is wide before it is dragged, and what a double-click on its edge puts back. */
const DEFAULT_WIDTH = 288;

function useSidebarWidth() {
  const [width, setWidth] = useState(() =>
    Math.max(MIN, Number(readStorage("rustcode.sidebarWidth")) || DEFAULT_WIDTH),
  );
  useEffect(() => {
    // Not persisted in a private window.
    writeStorage("rustcode.sidebarWidth", String(width));
  }, [width]);
  return [width, setWidth] as const;
}

/** The list's sections by name: the one Threads list, and the shelves that close it. */
const LABELS: Record<SectionId, string> = {
  working: "Working",
  pinned: "Pinned",
  threads: "Threads",
  snoozed: "Snoozed",
  settled: "Settled",
};

/** The folds `PREF.folded` keeps (a project's folder), in one array that never changes identity. */
const NO_FOLDS: string[] = [];

interface Section {
  id: SectionId;
  label: string;
  items: SessionSummary[];
  /** What a folded section hides: a thread that needs you, else one that works, else one just
   * finished — and nothing for Snoozed (`state/sidebarRows.ts` `hiddenKind`). */
  hidden?: HiddenKind;
  /** Threads, split by project when there are several, and by machine when one is paired. */
  projects?: ProjectGroup[];
}

/**
 * The open delegated thread as an indented row directly under the row of the thread that started
 * it, and the chain of delegates under it. It unfolds in with the list's fold motion and folds
 * away when the user opens another thread or goes back, instead of vanishing: the last row and
 * what hung under it are held for the fold.
 */
function OpenChild({ row, deeper }: { row: React.ReactNode | null; deeper: React.ReactNode }) {
  const held = useRef({ row, deeper });
  if (row) held.current = { row, deeper };
  return (
    <Fold open={!!row}>
      {held.current.row && (
        // `rc-row-in` is the list's own arrival motion: the fold above it starts open, so the row
        // would otherwise appear in place when the thread is opened.
        <div className="rc-row-in relative pl-4">
          {/* A short, fine elbow points into the delegate instead of spanning its row. */}
          <CornerDownRight
            aria-hidden
            size={12}
            strokeWidth={0.75}
            className="pointer-events-none absolute top-2 left-0.5 text-fg-faint"
          />
          {held.current.row}
          {held.current.deeper}
        </div>
      )}
    </Fold>
  );
}

/** The most forks and side chats drawn under one thread's row. */
const MAX_BRANCH_ROWS = 6;

/** The forks and side chats under a row, each indented by how deep it is, then "+N more". */
function BranchRows({
  under,
  render,
  onMore,
}: {
  under: { rows: BranchRow[]; more: number };
  render: (session: SessionSummary) => React.ReactNode;
  onMore: () => void;
}) {
  return (
    <>
      {under.rows.map(({ session, depth }) => (
        <div key={session.id} className="relative" style={{ paddingLeft: depth * 16 }}>
          <span
            aria-hidden
            className="pointer-events-none absolute inset-y-1 w-px bg-border"
            style={{ left: (depth - 1) * 16 + 6 }}
          />
          {render(session)}
        </div>
      ))}
      {under.more > 0 && (
        <button
          type="button"
          onClick={onMore}
          className="ml-4 rounded-sm px-2 py-1 text-left text-caption text-fg-muted hover:text-fg"
        >
          +{under.more} more
        </button>
      )}
    </>
  );
}

function SectionHeader({
  section,
  open,
  onToggle,
}: {
  section: Section;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className={cx(
        "group/h flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left font-medium text-fg-muted text-body hover:bg-fill-subtle",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{section.label}</span>
      {!open && <HiddenDot kind={section.hidden} />}
      {/* One slot: an open section shows its count, and the chevron in its place on hover (macOS). */}
      <span className="grid shrink-0 items-center justify-items-end">
        <span
          className={cx(
            "font-normal text-fg-faint [grid-area:1/1]",
            open && "group-hover/h:opacity-0 group-focus-visible/h:opacity-0",
            !open && "hidden",
          )}
        >
          <RollingNumber value={section.items.length} />
        </span>
        {!open && (
          <span className="flex items-center gap-1.5 [grid-area:1/1]">
            <RollingNumber value={section.items.length} className="font-normal text-fg-faint" />
            <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
          </span>
        )}
        {open && (
          <ChevronRight
            size={12}
            strokeWidth={2.5}
            className="shrink-0 rotate-90 text-fg-faint opacity-0 [grid-area:1/1] group-hover/h:opacity-100 group-focus-visible/h:opacity-100"
          />
        )}
      </span>
    </button>
  );
}

const NO_MARKS: Record<string, ThreadStatus> = {};

/** Nothing picked: one set, so clearing an empty selection never re-renders the list. */
const NO_PICKS: ReadonlySet<string> = new Set();

export function Sidebar() {
  const mac = appPlatform() === "mac";
  const TitlebarControls = mac ? Cluster : "div";
  const listedSessions = useApp((s) => s.sessions);
  // A scheduled job's run lives in the Scheduled Jobs view, never in the thread list.
  const jobRuns = useLoopThreads((s) => s.runs);
  const sessions = useMemo(
    () => listedSessions.filter((s) => !isJobRunThread(s, jobRuns)),
    [listedSessions, jobRuns],
  );
  // The list stops at a page cap: a parent older than its oldest thread is past it, not gone.
  const sessionsCapped = useApp((s) => s.sessionsCapped);
  const listedSince = useMemo(
    () =>
      listedSinceOf(
        listedSessions.filter((s) => !s.host).map((s) => s.id),
        sessionsCapped,
      ),
    [listedSessions, sessionsCapped],
  );
  // Paired computers' threads are rows of the one list, each tagged with where it runs.
  const machines = useMachines((s) => s.machines);
  const pending = useApp((s) => s.pendingSetup);
  const settingsOpen = useApp((s) => s.settings !== null);
  const backgroundSetups = useApp((s) => s.backgroundSetups);
  const pendingById = useMemo(
    () => ({ ...backgroundSetups, ...(pending ? { [pending.id]: pending } : {}) }),
    [backgroundSetups, pending],
  );
  // Loading until this computer's list is in; a restart keeps the rows it already shows.
  const listing = useApp((s) => !sessionsListed(s.engine) && !s.sessions.length);
  const selectedId = useApp((s) => s.selectedId);
  const pageOpen = useApp((s) => showsPage(s.view));
  // Which threads need you or work, as a string so a streamed chunk does not change it.
  const flags = useApp((s) =>
    Object.values(s.threads)
      .map((t) => (t.permissions.length ? `${t.id}:n` : isRunning(t) ? `${t.id}:w` : ""))
      .join(" "),
  );
  // When each request reached the app and each running turn started, as one string for the same
  // reason: the Threads list and the Working shelf order by them (state/sidebarRows.ts `sortSection`).
  const clocked = useApp((s) =>
    Object.values(s.threads)
      .flatMap((t) => {
        const asked = t.permissions[0]?.at;
        const sent = t.turn?.startedAt;
        return asked === undefined && sent === undefined ? [] : [`${t.id}:${asked ?? ""}:${sent ?? ""}`];
      })
      .join(" "),
  );
  const timings = useMemo(() => {
    const askedAt = new Map<string, number>();
    const sentAt = new Map<string, number>();
    for (const row of clocked.split(" ")) {
      if (!row) continue;
      // A thread id never holds a colon: the two times follow the first one.
      const at = row.indexOf(":");
      const id = at < 0 ? row : row.slice(0, at);
      const [asked, sent] = row.slice(at + 1).split(":");
      if (asked) askedAt.set(id, Number(asked));
      if (sent) sentAt.set(id, Number(sent));
    }
    return { askedAt, sentAt };
  }, [clocked]);
  const marks = useApp((s) => s.statusMarks ?? NO_MARKS);
  const motionThreads = useMemo(() => {
    const threads = new Map<string, boolean>();
    for (const session of sessions) {
      threads.set(session.id, (marks[session.id] ?? session.status) === "done");
    }
    return threads;
  }, [marks, sessions]);
  const finished = useApp((s) => s.finished);
  const failed = useApp((s) => s.failed);
  const projectCwd = useApp((s) => s.projectCwd);
  const hidden = useApp((s) => s.sidebarHidden);
  const shortcuts = useApp((s) => s.shortcuts);
  const toggleKey = useShortcutHint("toggle-sidebar");
  // The row marks' keys, from the same table: they follow a remap and show on the row's tooltips.
  const pinKey = useShortcutHint("pin-thread");
  const settleKey = useShortcutHint("settle-thread");
  const pinned = useApp((s) => s.pinned);
  // The order each section draws its rows in, as the user arranged it (state/sidebarOrder.ts).
  const layout = useApp((s) => s.sidebarOrder);
  // Each thread's branch's pull request, from the projects' one read (state/pulls.ts).
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  // The cache's version: a landed read re-renders the rows that read it.
  useSyncExternalStore(subscribePulls, pullsVersion);
  // By state, with the project on each row; grouping by project is an option.
  // The view options are changed from ⌘K (ui/CommandPalette.tsx), so they are shared, not local.
  const [byProject] = usePref(PREF.groupByProject, false);
  // Running work leads the one Threads list, or folds into a shelf at its foot with the preference
  // off: the layout the sidebar had before it put running work first (state/sidebarRows.ts
  // `SidebarLayout`).
  const [showWorking] = usePref(PREF.workingSection, true);
  // One line per thread, or the two-line row (Settings › Appearance, ⌘K).
  const compact = useSidebarDensity()[0] === "compact";
  const layoutMode: SidebarLayout = showWorking ? "top" : "shelf";
  const snoozed = useApp((s) => s.snoozed);
  useEffect(() => {
    wakeSnoozed();
    const t = setInterval(() => wakeSnoozed(), 15_000);
    return () => clearInterval(t);
  }, []);
  const [onlyProject] = usePref(PREF.onlyProject, false);
  // The toolbar over the list (ui/SidebarToolbar.tsx), the order it chose, and when an idle
  // thread settles on its own (Settings › Appearance).
  const [toolbarOn] = usePref(PREF.toolbar, true);
  const [sort] = useSidebarSort();
  const [settleDays] = usePref<number>(PREF.settleAfterDays, SETTLE_AFTER_DEFAULT_DAYS);
  const settleMs = settleAfterMs(settleAfterDays(settleDays));
  // The projects folded under "group by project" (`project:<path>`); the shelves keep their own.
  const [folded, setFolded] = usePref<string[]>(PREF.folded, NO_FOLDS);
  // A section header never folds, so a `working` or `threads` entry an older release stored would
  // shut a list with no control to open it. Drop everything but a project's folder once, on mount.
  useEffect(() => {
    const kept = cleanFolds(folded);
    if (kept.length !== folded.length) setFolded(kept);
  }, [folded, setFolded]);
  // Each shelf's open state is its own, kept per section: `undefined` until the user chooses, so
  // it takes the shelf's default (Working open, Snoozed and Settled shut).
  const [workingStored, setWorkingStored] = usePref<boolean | undefined>(shelfKey("working"), undefined);
  const [snoozedStored, setSnoozedStored] = usePref<boolean | undefined>(shelfKey("snoozed"), undefined);
  const [settledStored, setSettledStored] = usePref<boolean | undefined>(shelfKey("settled"), undefined);
  // The groups of child rows are gone, and with them the folds older releases kept open: drop
  // the record once, so a stale preference does not sit in localStorage for ever.
  useEffect(() => {
    writeStorage("rustcode.sidebar.childrenOpen", null);
  }, []);
  const [width, setWidth] = useSidebarWidth();
  // The slot takes its new width at once, so the transcript reflows once per toggle, not every
  // frame (docs/design.md › Motion). The column beside it then slides from where it was with a
  // transform, in step with the sidebar's own slide, so the sidebar pushes the content along.
  const slot = useRef<HTMLElement>(null);
  // Folders dragged from the file manager onto the sidebar become projects (issue 3608). The drag
  // is the window's, not the list's: a thread row being reordered is a pointer drag inside the
  // page and never reaches here, and a drag elsewhere stays the composer's attachment.
  const [folderDrop, setFolderDrop] = useState(false);
  useEffect(() => {
    let stop: (() => void) | undefined;
    let live = true;
    void onFileDrop((event) => {
      const over = overFolderDrop(event.position);
      setFolderDrop(event.type === "over" && over);
      if (event.type === "drop" && over && event.paths.length) void addDroppedFolders(event.paths);
    }).then((unlisten) => {
      if (live) stop = unlisten;
      else unlisten();
    });
    return () => {
      live = false;
      stop?.();
    };
  }, []);
  const shown = useRef(!hidden);
  useLayoutEffect(() => {
    if (shown.current === !hidden) return;
    shown.current = !hidden;
    const column = slot.current?.nextElementSibling;
    if (!column || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const from = hidden ? width : -width;
    column.animate([{ transform: `translateX(${from}px)` }, { transform: "translateX(0)" }], {
      duration: 200,
      easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
    });
  }, [hidden, width]);
  const dragging = useRef(false);
  const now = useClock();
  // Settled lists 5 threads, then 25 more at a time.
  const [settledShown, setSettledShown] = useState(5);
  // design-lint: allow (mask alpha, not a colour)
  const fade = "linear-gradient(to bottom, #000 calc(100% - 12px), transparent)";

  const filtered = useMemo(
    () =>
      sessions.filter(
        (s) => !onlyProject || !projectCwd || s.cwd === projectCwd || s.worktree?.repo === projectCwd,
      ),
    [sessions, onlyProject, projectCwd],
  );
  // The threads waiting on work they handed out, though no turn of theirs runs: the Threads list
  // holds them beside the ones running (state/waiting.ts).
  const waiting = useWaitingIds(useMemo(() => filtered.map((s) => s.id), [filtered]));

  const links = useApp((s) => s.links.links);
  const messaged = useApp((s) => s.messaged);
  // Where a thread an agent started goes: under the thread that started it, and nowhere else
  // (state/topLevel.ts) — the parent survives link pruning in the thread's own metadata. Every
  // session is asked, not the filtered list: a project filter or a search changes what is drawn,
  // never who a thread belongs to.
  const ownerOf = useMemo(() => {
    const known = new Set(sessions.map((s) => s.id));
    const parentOf = new Map<string, string>();
    for (const s of sessions) {
      const placement = topLevel(
        {
          id: s.id,
          pinned: pinned.includes(s.id),
          messaged: messaged[s.id] !== undefined,
          parent: s.parent,
        },
        links,
        known,
        s.host ? undefined : listedSince,
      );
      const parent = parentOfThread(s, links);
      if (placement === "nested" && parent) parentOf.set(s.id, parent);
    }
    return parentOf;
  }, [sessions, links, pinned, messaged, listedSince]);
  // A delegate started from another project, with the list filtered to its own: its parent's row is
  // not drawn, so it takes a row of its own here, marked with where it came from, rather than
  // vanishing from the project it works in (state/delegateOrigin.ts). With no filter its parent
  // is drawn and it nests there, never twice in one list.
  const detached = useMemo(
    () =>
      detachedDelegates(
        new Set(filtered.map((s) => s.id)),
        ownerOf,
        new Set(listedSessions.map((s) => s.id)),
      ),
    [filtered, ownerOf, listedSessions],
  );
  // The parents the list draws its rows under: every nested thread's, but a detached one's.
  const parentOf = useMemo(() => {
    if (!detached.size) return ownerOf;
    const next = new Map(ownerOf);
    for (const id of detached) next.delete(id);
    return next;
  }, [ownerOf, detached]);
  const byId = useMemo(() => new Map(sessions.map((s) => [s.id, s] as const)), [sessions]);
  // A row with no parent row above it says which thread it came from, and that thread's project
  // when it is another one: a search result, a delegate started from another project, or the open
  // delegate whose parent the filter left out. One object per thread, so the memoized rows keep it.
  const origins = useMemo(() => {
    const out = new Map<string, DelegateOrigin>();
    for (const s of filtered) {
      const above = ownerOf.get(s.id);
      if (!above) continue;
      out.set(s.id, originFrom(s, above, byId.get(above), s.parent?.title?.trim()));
    }
    return out;
  }, [filtered, ownerOf, byId]);
  // The threads with a row of their own, before the open delegate's chain is drawn under its parent.
  const topRows = useMemo(
    () => new Set(filtered.filter((s) => !parentOf.has(s.id)).map((s) => s.id)),
    [filtered, parentOf],
  );
  // The open delegated thread and the threads between it and its row of its own: drawn as indented
  // rows under the parent, and folded away when the user opens another thread or goes back.
  const chain = useMemo(() => openChain(selectedId, parentOf, topRows), [selectedId, parentOf, topRows]);
  // A nested thread the filter left with no drawn row above it: it keeps a row of its own rather
  // than going missing, saying which thread it belongs under.
  const orphanOpen =
    selectedId && parentOf.has(selectedId) && !chain.length && filtered.some((s) => s.id === selectedId)
      ? selectedId
      : undefined;
  // Each open delegate by the row it hangs under. The chain is one path, so a parent has one child.
  const subOf = useMemo(() => {
    const map = new Map<string, SubRow>();
    for (const sub of chain) map.set(sub.parent, sub);
    return map;
  }, [chain]);
  // The last delegate drawn under each row, kept while it folds away: an unmount would skip the
  // animation, and the session is no longer in the chain by then.
  const lastSubs = useRef(new Map<string, SessionSummary>());
  // A parent still shows the delegates it started in another project, nested as usual.
  const kin = sessions;
  const childrenByParent = useMemo(() => {
    const map: Record<string, SessionSummary[]> = {};
    for (const s of kin) {
      const parent = parentOf.get(s.id);
      if (parent) map[parent] = [...(map[parent] ?? []), s];
    }
    // Ids, so a list that reorders on an update does not reorder the stack's icons.
    for (const list of Object.values(map)) list.sort((a, b) => a.id.localeCompare(b.id));
    return map;
  }, [kin, parentOf]);

  // The forks and side chats below a row, drawn as indented rows so they can be found again (a
  // delegate is not drawn: it is the agent's, and shows as its parent's stack). Branches of
  // branches and of delegates come too, and the open one is drawn by the chain. Capped to the
  // newest few, with a "+N more" row that opens the rest.
  const [allBranches, setAllBranches] = useState<ReadonlySet<string>>(new Set());
  // The open chain is drawn by itself, and a detached thread is a row of its own already.
  const chainIds = useMemo(() => new Set([...chain.map((c) => c.id), ...detached]), [chain, detached]);
  const branchesUnder = useCallback(
    (rootId: string) =>
      branchRows(
        rootId,
        (id) => childrenByParent[id] ?? [],
        chainIds,
        allBranches.has(rootId) ? Number.POSITIVE_INFINITY : MAX_BRANCH_ROWS,
      ),
    [childrenByParent, chainIds, allBranches],
  );

  // The projects whose branches a pull request could name: a thread in a worktree has one to look
  // up, and its pulls live in the repository the worktree came from.
  const branchProjects = useMemo(
    () =>
      projectFolders(
        sessions.filter((s) => !s.host && s.worktree?.branch).map((s) => s.worktree?.repo || s.cwd),
      ),
    [sessions],
  );
  // One read per project, a moment after the list settles: `gh` is slow, and a row is not worth it.
  useEffect(() => {
    if (!pullsOn || !branchProjects.length) return;
    const t = setTimeout(() => branchProjects.forEach(loadPulls), 400);
    return () => clearTimeout(t);
  }, [pullsOn, branchProjects]);
  // Each listed thread's own pull request. The cache keeps the objects, so a row whose branch has
  // no pull request gets nothing new to render, and one that has keeps the same object.
  const pullOf = (session: SessionSummary) =>
    pullForBranch(cachedPulls(session.worktree?.repo || session.cwd), session.worktree?.branch);

  const { sections, seenOrder } = useMemo(() => {
    const seenOrder: SidebarOrder = {};
    const flagged = new Set(flags.split(" "));
    const isFresh = (id: string) => !!finished[id];
    // A thread that waits on work it handed out is drawn with the ones that run, so it is not lost
    // among the idle rows. The shelf layout keeps today's list, where it stays in Threads.
    const works = (id: string) => flagged.has(`${id}:w`) || (layoutMode === "top" && waiting.has(id));
    const placed: Record<SectionId, SessionSummary[]> = {
      working: [],
      pinned: [],
      threads: [],
      snoozed: [],
      settled: [],
    };
    // The thread setting up is a row of the list until the session exists: first, where its real
    // row will be, so it keeps its place when it becomes one.
    const pendingRows = Object.values(pendingById)
      .filter(
        (setup) =>
          !onlyProject || !projectCwd || setup.cwd === projectCwd || setup.worktree?.repo === projectCwd,
      )
      .map(pendingSummary);
    for (const s of filtered) {
      // A delegated thread has no row of its own, unless the filter left the open one with no row
      // to hang under (`orphanOpen`).
      if (parentOf.has(s.id) && s.id !== orphanOpen) continue;
      const listed = s.status === "done" || s.status === "review" ? s.status : "idle";
      // Settled: threads settled (by the user, or the engine for a merged branch), and
      // threads untouched for three days unless they wait for review.
      const status = marks[s.id] ?? listed;
      placed[
        placeThread({
          pinned: pinned.includes(s.id),
          settled: status === "done",
          aged: status !== "review" && idleSettled(s.updatedAt, now, settleMs),
          snoozed: !!snoozed[s.id],
          needs: flagged.has(`${s.id}:n`),
          working: flagged.has(`${s.id}:w`),
          waiting: works(s.id),
          fresh: isFresh(s.id),
          layout: layoutMode,
        })
      ].push(s);
    }
    const { threads: active } = placed;
    active.unshift(...pendingRows);
    // What a slot in the list reads of one thread when the user has not arranged it: the marks the
    // app already keeps, never a running clock, so no section reorders as a turn streams.
    const pinAt = new Map(pinned.map((id, at) => [id, at] as const));
    const keyOf = (s: SessionSummary): OrderKey => ({
      sentAt: timings.sentAt.get(s.id) ?? s.updatedAt,
      askedAt: timings.askedAt.get(s.id),
      backAt: Math.max(s.updatedAt ?? 0, finished[s.id] ?? 0, failed[s.id] ?? 0),
      unseen: !!(finished[s.id] || failed[s.id]),
      pinRank: pinAt.get(s.id),
      wakeAt: snoozed[s.id],
      running: works(s.id),
      needs: flagged.has(`${s.id}:n`),
      title: s.title || "New thread",
    });
    const created = (s: SessionSummary) => threadCreatedAt(splitThreadKey(s.id).id) ?? s.updatedAt ?? 0;
    const arranged = (id: SectionId, items: SessionSummary[], project?: string) => {
      const key = sectionOrderKey(id, project);
      const ordered =
        id === "threads"
          ? [...items].sort((a, b) => created(b) - created(a) || a.id.localeCompare(b.id))
          : sortSection(id, items, keyOf);
      seenOrder[key] = ordered.filter((s) => !pendingById[s.id]).map((s) => s.id);
      if (sort !== "manual" && id === "threads") return sortSection(id, items, keyOf, sort);
      return [
        ...ordered.filter((s) => pendingById[s.id]),
        ...applyOrder(
          ordered.filter((s) => !pendingById[s.id]),
          rememberOrder(layout[key], seenOrder[key], true),
        ),
      ];
    };
    // The threads by project, whenever the preference is on, including a single project.
    const grouped = sidebarProjects(active);
    let projects: ProjectGroup[] | undefined;
    if (byProject && grouped.length > 0) {
      projects = grouped;
      for (const project of projects) {
        if (project.project) project.label = projectLabel(project.project);
        project.items = arranged("threads", project.items, project.key);
      }
      const named = byProjectName(projects).map((p) => ({ ...p, id: p.key }));
      seenOrder.projects = named.map((p) => p.id);
      projects = applyOrder(named, layout.projects);
    }
    // The sections this layout draws, in its order: Pinned and the one Threads list, and the
    // shelves close it (state/sidebarRows.ts `sectionsOf`).
    const all: Section[] = sectionsOf(layoutMode).map((id) => ({
      id,
      label: LABELS[id],
      items: arranged(id, placed[id]),
      projects: id === "threads" ? projects : undefined,
    }));
    for (const g of all) {
      // A Snoozed shelf advertises nothing: what its rows did while asleep is not news
      // (state/sidebarRows.ts `hiddenKind`).
      g.hidden = hiddenKind(
        g.id,
        g.items.map((x) => ({
          needs: flagged.has(`${x.id}:n`),
          working: works(x.id),
          unseen: isFresh(x.id),
        })),
      );
    }
    return { sections: all.filter((g) => g.id === "threads" || g.items.length), seenOrder };
  }, [
    filtered,
    flags,
    waiting,
    timings,
    marks,
    finished,
    failed,
    pinned,
    parentOf,
    byProject,
    now,
    snoozed,
    layoutMode,
    pendingById,
    onlyProject,
    projectCwd,
    orphanOpen,
    layout,
    sort,
    settleMs,
  ]);

  useEffect(() => rememberSidebarOrder(seenOrder), [seenOrder]);

  const settledSection = sections.find((g) => g.id === "settled");
  // The first shelf starts the foot of the list; Pinned and Threads name themselves (`sectionLabelled`).
  const firstShelf = sections.find((g) => isShelf(g.id, layoutMode))?.id;
  const shownIds = sections.map((g) => g.id);
  const labelled = (id: SectionId) => id === "threads" || sectionLabelled(id, shownIds, layoutMode);
  // The open delegate's parent may have been settled: the shelf opens for it and its slice of the
  // shelf grows to hold the row, so the row it hangs under is reachable rather than out of sight.
  const settledAt = settledSection
    ? Math.max(
        -1,
        ...settledSection.items.map((s, index) => (chain.some((c) => c.parent === s.id) ? index : -1)),
      )
    : -1;
  // The shelf opens for that row once; folding it while the delegate is open keeps it shut until
  // another settled parent's delegate is opened.
  const chainParent = settledAt >= 0 ? settledSection!.items[settledAt]!.id : null;
  const [shelfShutFor, setShelfShutFor] = useState<string | null>(null);
  const settledForChain = chainParent !== null && chainParent !== shelfShutFor;
  // Every shelf folds on its own header, and the choice is kept per shelf. The sections above them
  // are the list itself and never fold; a project's folder folds with `folded`.
  const shelfIsOpen: Record<ShelfId, boolean> = {
    working: shelfOpen("working", workingStored),
    snoozed: shelfOpen("snoozed", snoozedStored),
    settled: shelfOpen("settled", settledStored, { forced: settledForChain }),
  };
  // Only a shelf and a project's folder fold. A section header never does,
  // whatever `folded` holds, so an id an older release stored can never shut a list with no control
  // to open it.
  const isOpen = (id: string) => {
    if (isShelf(id, layoutMode)) return shelfIsOpen[id];
    if (id.startsWith("project:")) return !folded.includes(id);
    return true;
  };
  const toggle = (id: string) => {
    if (id === "working") setWorkingStored(!isOpen(id));
    else if (id === "snoozed") setSnoozedStored(!isOpen(id));
    else if (id === "settled") {
      const open = isOpen(id);
      setShelfShutFor(open ? chainParent : null);
      setSettledStored(!open);
    } else setFolded(folded.includes(id) ? folded.filter((x) => x !== id) : [...folded, id]);
  };
  const settledLimit = Math.max(settledShown, settledAt + 1);

  // Which list each drawn row arranges, and the rows drawn in it: a drag and ⌥↑/↓ both move a row
  // inside the one list it was drawn in — a section, or one project's run of rows under its heading
  // — and nowhere else (state/sidebarOrder.ts `reorder`).
  const homes = useMemo(() => {
    const homes = new Map<string, { key: string; ids: string[] }>();
    // Only threads with a session: the row a message shows before the engine has it is not one to
    // arrange, and it leaves the list as soon as the session exists.
    const add = (key: string, items: SessionSummary[]) => {
      const ids = items.map((s) => s.id).filter((id) => byId.has(id));
      for (const id of ids) homes.set(id, { key, ids });
    };
    for (const section of sections) {
      // Settled draws a slice of its rows, and a folded section draws none: the rows on screen are
      // the ones a drag can move.
      if (section.id === "threads" && sort !== "manual") continue;
      const items = section.id === "settled" ? section.items.slice(0, settledLimit) : section.items;
      if (section.projects) {
        for (const p of section.projects) add(sectionOrderKey(section.id, p.key), p.items);
      } else add(section.id, items);
    }
    return homes;
  }, [byId, sections, settledLimit, sort]);

  // ⌥↑/↓ on a focused row takes it one place up or down the list it is drawn in, and keeps the
  // keyboard on it: the row is the same element, moved (ui/Sidebar.tsx `onListKey`).
  const moveRow = (id: string, delta: number) => {
    const home = homes.get(id);
    if (!home) return;
    const next = moveBy(layout[home.key] ?? [], home.ids, id, delta);
    if (next) setSectionOrder(home.key, next);
  };

  // ---------------------------------------------------------------- the list's rows, selection and keys
  // What is picked, and which row the keyboard is on, belong to the sidebar while it is drawn:
  // nothing outside it reads them, and a thread that leaves the list leaves the selection.
  const [picked, setPicked] = useState<ReadonlySet<string>>(NO_PICKS);
  const [activeId, setActiveId] = useState<string | null>(null);
  const anchor = useRef<string | null>(null);
  const list = useRef<HTMLElement>(null);
  // Dragging a row rearranges the list it was drawn in and nothing else; a search narrows the rows,
  // so a drag is off while one is on and ⌥↑/↓ still arranges (ui/sidebarDrag.tsx).
  const drag = useThreadDrag({
    list,
    homeOf: (id) => homes.get(id),
    enabled: true,
    drop: (home, id, index) =>
      setSectionOrder(home.key, reorder(layout[home.key] ?? [], home.ids, id, index)),
  });
  const projectIds = sections.flatMap((section) => section.projects?.map((p) => p.key) ?? []);
  const projectDrag = useThreadDrag({
    list,
    rowAttribute: "data-project-id",
    handleSelector: "[data-project-handle]",
    homeOf: () => ({ key: "projects", ids: projectIds }),
    enabled: true,
    drop: (home, id, index) =>
      setSectionOrder(home.key, reorder(layout[home.key] ?? [], home.ids, id, index)),
  });
  // Every delegated thread's state, as its parent's stack reads it: one string, so a streamed
  // chunk that changes nothing about them does not redraw the list.
  const childStates = useApp((s) => {
    // A child reads by the same live state its own row and the delegate tree use: a link a restart
    // marked interrupted says nothing when its thread kept working (`state/host.ts` `stateOf`).
    const linked = new Map(s.links.links.filter((l) => l.started).map((l) => [l.target, l] as const));
    return kin
      .map((c) => {
        if (!parentOf.has(c.id)) return "";
        const link = linked.get(c.id);
        const info = childInfo(c.id, {
          status: statusOf(s, c.id),
          link: link ? stateOf(s, link) : undefined,
          failed: !!s.failed[c.id],
        });
        return `${info.id}:${info.state}`;
      })
      .join(" ");
  });
  const infos = useMemo(() => {
    const by = new Map<string, ChildInfo>();
    for (const part of childStates.split(" ")) {
      const [id, state] = part.split(":");
      if (id) by.set(id, { id, state: state as ChildInfo["state"] });
    }
    return by;
  }, [childStates]);

  // A row's stack: every descendant that works or waits on the user, grandchildren flattened,
  // in a stable order so an update that changes no state does not move the icons. A finished or
  // failed delegate adds nothing: the stack is work under way, not the tree's history.
  const stacks = useMemo(() => {
    const out = new Map<string, StackAgent[]>();
    // A nested thread's start time is its link's: the row says how long it has run, and
    // the mark's kind says it is a thread to open, not a subagent inside this one.
    const linkOf = new Map(links.filter((l) => l.started).map((l) => [l.target, l] as const));
    const gather = (parentId: string): StackAgent[] => {
      const found: StackAgent[] = [];
      const walk = (id: string) => {
        for (const child of childrenByParent[id] ?? []) {
          const info = infos.get(child.id);
          if (info && (info.state === "needs" || info.state === "working"))
            found.push({
              id: child.id,
              state: info.state,
              kind: "thread",
              harness: child.harness,
              label: child.title || "New thread",
              // A delegate on another machine says where it runs on this row's stack: the
              // mark a parent shows and the tip it opens both name the machine.
              ...(child.host
                ? { machine: machines.find((m) => m.id === child.host)?.name ?? child.host }
                : {}),
              since: linkOf.get(child.id)?.createdAt,
            });
          walk(child.id);
        }
      };
      walk(parentId);
      return found;
    };
    for (const s of filtered) {
      if (parentOf.has(s.id)) continue;
      const agents = gather(s.id);
      if (agents.length) out.set(s.id, agents);
    }
    return out;
  }, [childrenByParent, filtered, infos, links, machines, parentOf]);

  // The order the list draws its rows in, so ↑ ↓ move through what is on screen: a shut section
  // or project contributes nothing. A delegated thread has no row of its own, so it is not here
  // (state/sidebarRows.ts).
  const { working: workingOpen, snoozed: snoozedOpen, settled: settledShelfOpen } = shelfIsOpen;
  const order = useMemo<OrderSection[]>(() => {
    const shelfShut: Record<ShelfId, boolean> = {
      working: !workingOpen,
      snoozed: !snoozedOpen,
      settled: !settledShelfOpen,
    };
    // A row with the open delegate under it draws its chain next: ↑ ↓ and the tab stop reach the
    // sub-rows, because the list draws them.
    const toOrder = (items: SessionSummary[]): OrderRow[] => {
      const rows: OrderRow[] = [];
      const add = (s: SessionSummary, depth: number) => {
        rows.push({ id: s.id });
        const sub = subOf.get(s.id);
        const child = sub && depth < 8 ? byId.get(sub.id) : undefined;
        if (child) add(child, depth + 1);
        if (depth === 0) for (const kid of branchesUnder(s.id).rows) rows.push({ id: kid.session.id });
      };
      for (const s of items) add(s, 0);
      return rows;
    };
    return sections.map((section) =>
      section.projects
        ? {
            id: section.id,
            groups: section.projects.map((p) => ({
              key: p.key,
              shut: folded.includes(`project:${p.key}`),
              rows: toOrder(p.items),
            })),
          }
        : {
            id: section.id,
            shut: isShelf(section.id, layoutMode) && shelfShut[section.id],
            rows: toOrder(section.items),
            limit: section.id === "settled" ? settledLimit : undefined,
          },
    );
  }, [
    branchesUnder,
    byId,
    folded,
    sections,
    settledLimit,
    workingOpen,
    snoozedOpen,
    settledShelfOpen,
    subOf,
    layoutMode,
  ]);
  const orderIds = useMemo(() => visibleRows(order), [order]);

  // The row the keyboard is on: where it was left, else the open thread, else the first row. A
  // delegated thread has no row, so the open one lands on its parent, which is the row kept lit
  // as its context (state/sidebarRows.ts).
  const contextSelected = contextRow(selectedId, orderIds, parentOf);
  const currentId =
    activeId && orderIds.includes(activeId) ? activeId : (contextSelected ?? orderIds[0] ?? null);

  useEffect(() => {
    setPicked((prev) => (prev.size ? keepListed(prev, orderIds.map(rowIdOf)) : prev));
  }, [orderIds]);

  const pickedSessions = useMemo(() => listedPicks(sessions, picked), [sessions, picked]);
  const allPickedPinned = pickedSessions.length > 0 && pickedSessions.every((s) => pinned.includes(s.id));
  // Whether any or every picked thread is settled, read from the same list the bar acts on.
  // Unsettle moves only the settled ones, so a Review mark on the rest of the selection stays.
  const pickedSettled = useApp((s) =>
    listedPicks(s.sessions, picked).some((x) => statusOf(s, x.id) === "done"),
  );
  const allPickedSettled = useApp((s) => {
    const picks = listedPicks(s.sessions, picked);
    return picks.length > 0 && picks.every((x) => statusOf(s, x.id) === "done");
  });
  const clearSelection = useCallback(() => setPicked(NO_PICKS), []);
  const focusRow = (id: string) => {
    const row = list.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`);
    row?.focus();
    return !!row;
  };

  // P pins and D settles the rows the keyboard acts on: a multi-selection when there is one, else
  // the row the keyboard is on. Both are the calls the hover actions and the selection bar make,
  // so a mark means the same thing wherever it is asked for, and both offer the same toast.
  const markRows = (command: string, focused: string) => {
    // The pending row has no thread yet: there is nothing to pin or settle, as a click and the
    // row's own menu already know.
    if (pendingById[focused]) return;
    const ids = pickedSessions.length > 1 ? pickedSessions.map((s) => s.id) : [rowIdOf(focused)];
    if (command === "pin-thread") {
      // The same rule as the selection bar's button: unpin when every row is pinned, else pin.
      const wasPinned = ids.filter((id) => pinned.includes(id));
      const on = wasPinned.length !== ids.length;
      setPinned(ids, on);
      const title = byId.get(ids[0]!)?.title || "New thread";
      toast(
        ids.length > 1
          ? `${on ? "Pinned" : "Unpinned"} ${ids.length} threads`
          : `${on ? "Pinned" : "Unpinned"} "${title}"`,
        "info",
        {
          // Each row goes back the way it was, however mixed the selection was.
          action: {
            label: "Undo",
            run: () => {
              const rest = ids.filter((id) => !wasPinned.includes(id));
              if (wasPinned.length) setPinned(wasPinned, true);
              if (rest.length) setPinned(rest, false);
            },
          },
        },
      );
    } else if (command === "settle-thread") {
      const state = useApp.getState();
      // D settles; on threads already settled it puts them back, like the row's own button.
      if (ids.every((id) => statusOf(state, id) === "done")) unsettleThreads(ids);
      else settleThreads(ids);
    } else return;
    // A mark can move the row to another section, where it is drawn anew: keep the keyboard on it.
    // D also takes a row out of the list itself, folding it into the shut Settled shelf, and the
    // row then unmounts with the keyboard on it, which leaves focus on <body>. So read the rows
    // beside it first, while the list still draws them, and let the row that took its place take
    // the keyboard when the mark's row is gone: the next one, else the one before.
    const at = orderIds.indexOf(focused);
    const beside = at < 0 ? [] : [...orderIds.slice(at + 1), ...orderIds.slice(0, at).reverse()];
    setActiveId(focused);
    setTimeout(() => {
      const on = [focused, ...beside].find((id) => focusRow(id));
      if (on && on !== focused) setActiveId(on);
    }, 0);
  };

  // ↑ ↓ ⏎ Esc over the rows, and P/D to mark the row the keyboard is on; a section or group
  // control keeps its own keys, and a field inside the list (the rename box) keeps every key.
  const onListKey = (e: React.KeyboardEvent) => {
    const target = e.target as HTMLElement | null;
    const editable = !!target?.closest("input, textarea, [contenteditable]");
    // ⌥↑/↓ arranges the row the keyboard is on in the list it is drawn in — the settings list's own
    // gesture (ui/HeaderButtonsSetting.tsx) — so the order is reachable without a pointer. A field
    // inside the list keeps every key.
    if (!editable && e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      if (!target?.closest("[data-row-id]") || !currentId) return;
      e.preventDefault();
      // The copy under a parent has no place of its own in a list to arrange.
      if (!isNestedKey(currentId)) moveRow(currentId, e.key === "ArrowUp" ? -1 : 1);
      return;
    }
    const action = rowKeyAction({
      key: e.key,
      metaKey: e.metaKey,
      ctrlKey: e.ctrlKey,
      altKey: e.altKey,
      editable,
    });
    // The list's marking keys never fire from the window (lib/keys.ts): they are read here, where
    // a row has the keyboard, so typing one into any field stays typing. A key held down repeats,
    // and a repeated P or D would mark and unmark the row in turns: only the first press counts.
    const mark = editable || e.repeat ? null : listCommandFor(shortcuts, e);
    if (!action && !mark) return;
    if (action?.kind === "clear") {
      e.preventDefault();
      clearSelection();
      return;
    }
    if (!target?.closest("[data-row-id]")) return;
    e.preventDefault();
    if (mark) {
      if (currentId) markRows(mark, currentId);
      return;
    }
    if (action?.kind === "open") {
      if (currentId) selectThread(rowIdOf(currentId));
      clearSelection();
      return;
    }
    if (action?.kind !== "move") return;
    const to = nextRow(orderIds, currentId, action.delta);
    if (!to) return;
    setActiveId(to);
    focusRow(to);
  };

  const onListFocus = (e: React.FocusEvent) => {
    const id = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-row-id]")?.dataset.rowId;
    if (id) setActiveId(id);
  };

  // ⌘-click toggles a row, ⇧-click takes the range from the last row clicked, and a plain click
  // opens the thread and lets the selection go (the row's own onClick does the opening).
  const onListClick = (e: React.MouseEvent) => {
    const key = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-row-id]")?.dataset.rowId;
    if (!key) return;
    // What is picked is the thread, whichever of its rows was clicked; the keyboard and the range
    // anchor stay on the row.
    const id = rowIdOf(key);
    // The pending row has no session to pin, settle or rename: it opens, and never joins a bulk pick.
    if (pendingById[id]) {
      if (primaryModifier(e) || e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
      }
      setActiveId(key);
      anchor.current = key;
      clearSelection();
      return;
    }
    const mod = primaryModifier(e);
    if (!mod && !e.shiftKey) {
      setActiveId(key);
      anchor.current = key;
      clearSelection();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    setActiveId(key);
    if (e.shiftKey) {
      setPicked(new Set(rangeRows(orderIds, anchor.current ?? key, key).map(rowIdOf)));
      return;
    }
    anchor.current = key;
    setPicked((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  };

  // The width is dragged from the sidebar's edge and kept in app storage as it goes
  // (`useSidebarWidth`); a double-click on that edge puts the default back.
  const onResize = (e: React.PointerEvent) => {
    dragging.current = true;
    const startX = e.clientX;
    const start = width;
    const move = (ev: PointerEvent) => setWidth(Math.min(MAX, Math.max(MIN, start + ev.clientX - startX)));
    const up = () => {
      dragging.current = false;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const rowFor = (s: SessionSummary, hint = true, nested = false) => (
    <ThreadRow
      session={s}
      selected={!pageOpen && s.id === selectedId}
      context={!pageOpen && s.id === contextSelected && s.id !== selectedId}
      // A row being dragged shows no hover card and takes no pointer.
      dragging={drag.draggedId === s.id}
      pull={pullOf(s)}
      now={now}
      from={hint ? origins.get(s.id) : undefined}
      agents={stacks.get(s.id)}
      navActive={s.id === currentId}
      picked={picked.has(s.id)}
      bulk={picked.size > 1 && picked.has(s.id) ? pickedSessions : undefined}
      pinKey={pinKey}
      settleKey={settleKey}
      onClearSelection={clearSelection}
      compact={compact || nested}
    />
  );
  // The open delegate and its descendants, each an indented row under the one above it. The last
  // one drawn under a row is kept while it folds away (`OpenChild` holds it for the animation).
  const openBelow = (parentId: string, depth = 0): React.ReactNode => {
    const sub = subOf.get(parentId);
    const current = sub ? byId.get(sub.id) : undefined;
    if (current && sub) lastSubs.current.set(parentId, current);
    const child = current ?? lastSubs.current.get(parentId);
    if (!child || depth > 8) return null;
    return (
      <OpenChild
        row={current ? rowFor(current, false, true) : null}
        deeper={openBelow(child.id, depth + 1)}
      />
    );
  };
  const rows = (items: SessionSummary[]) => (
    <div className={cx("flex flex-col", compact ? "gap-px" : "gap-0.5")}>
      {items.map((s) => (
        <Fragment key={s.id}>
          {pendingById[s.id] ? (
            <PendingRow
              pending={pendingById[s.id]!}
              selected={!pageOpen && !selectedId && pending?.id === s.id}
              navActive={s.id === currentId}
              compact={compact}
            />
          ) : (
            rowFor(s)
          )}
          {openBelow(s.id)}
          <BranchRows
            under={branchesUnder(s.id)}
            render={(kid) => rowFor(kid, false)}
            onMore={() => setAllBranches((prev) => new Set(prev).add(s.id))}
          />
        </Fragment>
      ))}
    </div>
  );

  return (
    <aside
      ref={slot}
      aria-label="Threads"
      data-focus-area="threads"
      // Not clipped: while it slides out of a slot already 0 wide, the column beside it covers it.
      className={cx("relative flex shrink-0 flex-col", hidden && "rc-edge-peek rc-edge-peek-left")}
      style={{ width: hidden ? 0 : width }}
    >
      {hidden && <span aria-hidden className="rc-edge-peek-handle" />}
      <div
        {...{ [FOLDER_DROP_ATTR]: "" }}
        className={cx(
          "relative flex h-full flex-col bg-sidebar-glass transition-[translate] duration-200 ease-snappy",
          hidden ? "rc-edge-peek-surface" : "translate-x-0",
        )}
        style={{ width, minWidth: width }}
      >
        {folderDrop && (
          // The drop target: the whole sidebar, outlined the way the composer outlines a file drop.
          <div
            aria-live="polite"
            className="pointer-events-none absolute inset-2 z-30 flex flex-col items-center justify-center gap-2 rounded-lg bg-accent/10 px-4 text-center text-callout text-fg outline-2 outline-accent outline-dashed"
          >
            <FolderPlus size={18} className="text-accent" />
            Drop folders to add them as projects
          </div>
        )}
        {/* macOS groups the controls after its lights; other platforms leave a drag gap. */}
        <TitlebarRow className="pr-3">
          <TitlebarControls className={mac ? "ml-auto shrink-0" : "contents"}>
            <ClusterButton
              label={hidden ? "Pin sidebar open" : "Hide sidebar"}
              tooltip={`${hidden ? "Pin sidebar open" : "Hide sidebar"} (${toggleKey})`}
              onClick={toggleSidebar}
            >
              <PanelLeft size={14} />
            </ClusterButton>
            {!mac && <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />}
            <ClusterButton
              label="Search threads"
              tooltip="Search threads (⌘K)"
              onClick={() => setSwitcher(true)}
            >
              <Search size={14} />
            </ClusterButton>
            <SettingsButton />
          </TitlebarControls>
        </TitlebarRow>
        <div className={cx("relative flex min-h-0 flex-1 flex-col", mac && "pt-2")}>
          <div
            className={cx("relative flex min-h-0 flex-1 flex-col", settingsOpen && "invisible")}
            inert={settingsOpen}
          >
            <SidebarSelection />
            <NeedsYouRow threadIds={new Set(filtered.map((s) => s.id))} />
            {toolbarOn && <SidebarToolbar />}
            <nav
              ref={list}
              data-focus-default
              // One tab stop for the whole list (the row the keyboard is on), ↑ ↓ through its rows,
              // ⏎ to open and Esc to let the selection go.
              onKeyDown={onListKey}
              onFocus={onListFocus}
              onClickCapture={onListClick}
              // A press on a row is a drag if the pointer moves (ui/sidebarDrag.tsx), and a click if not.
              onPointerDown={(e) => {
                drag.start(e);
                projectDrag.start(e);
              }}
              aria-label="Thread list"
              data-sidebar-scroll
              className="relative min-h-0 flex-1 overflow-y-auto px-3 pb-2"
              style={{ maskImage: fade, WebkitMaskImage: fade }}
            >
              {listing && <ThreadRowsSkeleton />}
              <div className="relative flex min-h-full flex-col">
                {sections.map((section) => {
                  const open = isOpen(section.id);
                  const shelf = isShelf(section.id, layoutMode);
                  return (
                    <Fragment key={section.id}>
                      {/* The shelves sit at the foot, under one hairline, apart from Pinned and Threads. */}
                      {section.id === firstShelf && (
                        <div aria-hidden className="mt-2 min-h-2 flex-1 border-border border-b" />
                      )}
                      <section
                        className={cx("mt-2 first:mt-0", section.id === firstShelf && "mt-0")}
                        aria-label={section.label}
                        data-shelf={shelf ? section.id : undefined}
                      >
                        {/* Shelves fold. The Threads label also holds the list controls. */}
                        {shelf ? (
                          <SectionHeader section={section} open={open} onToggle={() => toggle(section.id)} />
                        ) : (
                          labelled(section.id) && (
                            <ThreadSectionLabel
                              label={section.label}
                              threads={section.id === "threads"}
                              filtering={onlyProject && !!projectCwd}
                            />
                          )
                        )}
                        {section.id === "threads" && !listing && !filtered.length && (
                          <div className="px-2 py-3 text-callout text-fg-muted">
                            {onlyProject ? "No threads in this project yet." : "No threads yet."}
                          </div>
                        )}
                        <Fold open={open}>
                          {section.projects
                            ? section.projects.map((p) => {
                                const projectOpen = isOpen(`project:${p.key}`);
                                const onNew = p.project ? () => startNewThread(p.project) : undefined;
                                return (
                                  <div
                                    key={p.key}
                                    data-project-id={p.key}
                                    className="relative mb-0.5 last:mb-0"
                                  >
                                    <ProjectHeader
                                      label={p.label}
                                      count={p.items.length}
                                      hidden={
                                        p.items.some((x) => flags.includes(`${x.id}:n`))
                                          ? "needs"
                                          : p.items.some((x) => flags.includes(`${x.id}:w`))
                                            ? "working"
                                            : p.items.some((x) => !!finished[x.id])
                                              ? "finished"
                                              : undefined
                                      }
                                      open={projectOpen}
                                      onToggle={() => toggle(`project:${p.key}`)}
                                      onNew={onNew}
                                    />
                                    {/* Only delegates are indented; project threads keep the full row width. */}
                                    <Fold open={projectOpen}>
                                      <div className="pt-1">{rows(p.items)}</div>
                                    </Fold>
                                  </div>
                                );
                              })
                            : rows(
                                section.id === "settled"
                                  ? section.items.slice(0, settledLimit)
                                  : section.items,
                              )}
                          {section.id === "settled" && section.items.length > settledLimit && (
                            <button
                              type="button"
                              onClick={() => setSettledShown((n) => n + 25)}
                              className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
                            >
                              <ChevronDown size={14} className="shrink-0" />
                              Show {Math.min(25, section.items.length - settledShown)} more
                            </button>
                          )}
                        </Fold>
                      </section>
                    </Fragment>
                  );
                })}
              </div>
            </nav>
            <SidebarRowMotion
              list={list}
              threads={motionThreads}
              ready={!listing && !hidden && !settingsOpen}
            />
            {pickedSessions.length > 1 && (
              <div className="flex shrink-0 items-center gap-1 px-2 pb-1">
                <span
                  className="min-w-0 flex-1 truncate px-1 font-medium text-caption text-fg-muted tabular"
                  aria-live="polite"
                >
                  {pickedSessions.length} selected
                </span>
                <Pill
                  size="sm"
                  quiet
                  onClick={() => {
                    setPinned(
                      pickedSessions.map((s) => s.id),
                      !allPickedPinned,
                    );
                    clearSelection();
                  }}
                >
                  {allPickedPinned ? "Unpin" : "Pin"}
                </Pill>
                {!allPickedSettled && (
                  <Pill
                    size="sm"
                    quiet
                    onClick={() => {
                      settleThreads(pickedSessions.map((s) => s.id));
                      clearSelection();
                    }}
                  >
                    Settle
                  </Pill>
                )}
                {pickedSettled && (
                  <Pill
                    size="sm"
                    quiet
                    onClick={() => {
                      unsettleThreads(pickedSessions.map((s) => s.id));
                      clearSelection();
                    }}
                  >
                    Unsettle
                  </Pill>
                )}
                <Pill size="sm" quiet onClick={clearSelection}>
                  Clear
                </Pill>
              </div>
            )}
            <UpdateRow />
          </div>
          {settingsOpen && (
            <div className="absolute inset-0">
              <SettingsNav />
            </div>
          )}
        </div>
      </div>
      <CustomSnooze />
      {!hidden && (
        <Tooltip label="Drag to resize · Double-click to reset" side="right">
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize sidebar"
            onPointerDown={onResize}
            // Back to the width a fresh window has, kept like any other width.
            onDoubleClick={() => setWidth(DEFAULT_WIDTH)}
            className="group absolute top-0 right-[-3px] bottom-0 z-10 w-[7px] cursor-col-resize"
          >
            <div className="mx-auto h-full w-px bg-transparent group-hover:bg-fill" />
          </div>
        </Tooltip>
      )}
    </aside>
  );
}

/** Where the thread list will be: a section's label, then rows at a thread row's height. */
const SKELETON_ROWS = [72, 56, 84, 64, 48, 76, 60];
function ThreadRowsSkeleton() {
  return (
    <div className="flex flex-col pt-1" role="status" aria-label="Loading threads">
      <Skeleton className="mx-2 mt-2 mb-3 h-2.5 w-16" />
      {SKELETON_ROWS.map((w, i) => (
        <div key={i} className="flex flex-col justify-center gap-2 px-2" style={{ height: ROW_HEIGHT }}>
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
          <Skeleton className="h-2" style={{ width: `${Math.round(w * 0.45)}%` }} />
          <Skeleton className="h-2" style={{ width: `${Math.round(w * 0.6)}%` }} />
        </div>
      ))}
    </div>
  );
}
