import { formatBinding } from "../lib/keys";
import { basename } from "../lib/paths";
import { fileManagerName, primaryModifier } from "../lib/platform";
// The composer, after ContentView.swift's Composer: one glass box floating over
// the transcript, holding the attachments, the field (with a slash-command token
// in front of it; pasted links stay inline as text) and its foot: + on the left,
// the model pill and send/queue/stop on the right. What the thread works in and
// what it may do hang off that box's bottom edge as a strip of their own (the
// folder, the worktree, access, the meter), because they describe the thread and
// not the message. What waits for the thread is not here: queued messages are rows at the end of the transcript
// (ui/QueuedMessages.tsx), and one of them is edited in this field. Keys follow
// the SwiftUI app: ⏎ send (or queue while running), ⇧⏎ newline, ⌘⏎ steer, Esc
// closes suggestions, then leaves a queued edit, then clears the draft, then
// stops the turn.

import {
  ArrowUp,
  Check,
  ChevronDown,
  Clock,
  Command,
  Copy,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  ListEnd,
  Monitor,
  Paperclip,
  Pencil,
  Plus,
  Shield,
  ShieldCheck,
  ShieldOff,
  Square,
  SquarePen,
  SquareTerminal,
  X,
} from "lucide-react";
import { Fragment, memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { harnessPlanKey } from "../acp/rustcode";
import { type Harness, projectName, type SlashCommand } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { suggestBranch, validBranch } from "../acp/v2";
import { FIELD_FADE, type FieldFade, fieldFade } from "../lib/fade";
import { measuredFieldHeight } from "../lib/fieldHeight";
import { folderRows, matchSpans, middleEllipsis } from "../lib/folderList";
import { haptic } from "../lib/haptics";
import {
  canOpenTerminal,
  clipboardImage,
  type ImageFile,
  onFileDrop,
  openPath,
  openTerminalAt,
  pickFiles,
  pickFolder,
  projectFiles,
  readImage,
} from "../lib/native";
import { isLongPaste, pastedImages } from "../lib/paste";
import { observeSize } from "../lib/resize";
import { fadeClearedField } from "../lib/sendFlight";
import {
  type AgentChoice,
  type Attachment,
  type ComposeExtras,
  commandSuggestions,
  emptyExtras,
  hasExtras,
  hashToken,
  hasTokens,
  IMAGE_TYPES,
  imageMime,
  insertCommand,
  insertMention,
  MAX_IMAGE_BYTES,
  MAX_INLINE_FILE_BYTES,
  type Mention,
  type MentionState,
  pruneMentions,
  rankAgents,
  rankThreads,
  sentMessages,
  slashToken,
  splitTokens,
  type ThreadChoice,
  trackPastes,
} from "../state/compose";
import { agentStateOf, type DelegateNode, delegateTree } from "../state/delegates";
import { atToken, insertKeyword, insertRef, type MentionScope, rankFiles, SCOPES } from "../state/fileRefs";
import { controlDisabledReason, defaultEffort, orderHarnesses, unavailableReason } from "../state/harness";
import { usageOf } from "../state/harnessUsage";
import { childAccountOn, type SignedOutLogins } from "../state/logins";
import { type MachineState, selfName, useMachines } from "../state/machines";
import { PERMISSION_MODES, type PermissionMode } from "../state/permissions";
import { scheduledSummary } from "../state/schedule";
import type { CachedPlan } from "../state/store";
import {
  acceptsImages,
  answerPermission,
  cancel,
  cancelEdit,
  cancelQueuedEdit,
  commandsFor,
  dropLimitWait,
  endQueuedEdit,
  featureMissing,
  harnessesOn,
  limitTime,
  newThreadChoice,
  permissionModeOf,
  saveQueuedEdit,
  scheduleMessage,
  sendDecisionFor,
  setDraft,
  setExtras,
  setNewThreadHost,
  setNewWorktree,
  setPermissionMode,
  setProjectCwd,
  startNewThread,
  startQueuedEdit,
  steerOrSubmit,
  submit,
  threadHarness,
  toast,
  useApp,
  useMachineOfThread,
  useThreadOffline,
} from "../state/store";
import { localId, shortAge, type ThreadState } from "../state/thread";
import { LOCAL_HOST, splitThreadKey } from "../state/threadKeys";
import { UsageMeter } from "./ComposerUsage";
import { CardMenuItem, CardPopover, menuKeys } from "./cardPopover";
import { HarnessIcon } from "./HarnessIcon";
import { openLightbox } from "./Lightbox";
import { LinkChip } from "./LinkChip";
import { ModelPicker } from "./ModelPicker";
import { PasteChip } from "./PasteChip";
import { cx, MenuSearchField, MenuShortcut, noTextHelpers, Pill, Tooltip } from "./primitives";
import { QueuedAnnouncer } from "./QueuedMessages";
import { ScheduledMessages } from "./ScheduledMessages";
import { SendLaterDialog } from "./SendLaterDialog";

// The field's line box: 14px at 1.5 (`leading-normal`), a hair tighter than a reply's, which is what
// makes a draft read as something being typed rather than something read back.
const LINE = 21;
// Real padding inside the scroller, so it scrolls with the text: the first and last lines keep their
// room at either end, and the caret on the last line sits above the padding, never on the edge.
const PAD = 6;
// It opens at one line above the card's foot and scrolls past the ninth. The minimum is that one
// whole line plus its padding on each side (`MIN_HEIGHT`), so a single line sits centred between
// them instead of at the top of an empty second line, and the field cuts no line in half as it
// grows a line at a time; past the maximum the fade (`.rc-field-fade`) hides the cut, as it does in
// any scroller.
const MIN_HEIGHT = Math.round(LINE + PAD * 2);
const MAX_HEIGHT = 208;
// How much of a folder's path the picker shows: its ends, not the whole of a deep path.
const PATH_CHARS = 30;

function readFileAs(file: File, as: "base64" | "text"): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the file."));
    reader.onload = () => {
      const result = String(reader.result ?? "");
      resolve(as === "base64" ? result.slice(result.indexOf(",") + 1) : result);
    };
    if (as === "base64") reader.readAsDataURL(file);
    else reader.readAsText(file);
  });
}

/**
 * Whether a popover, menu or dialog is open over the app. Radix closes those
 * on Escape from a document listener, after the field's own handler has run,
 * so the composer checks first. Tooltips do not count.
 */
function overlayOpen(): boolean {
  return !!document.querySelector(
    '[data-radix-popper-content-wrapper]:not(:has([role="tooltip"])), [role="dialog"], [role="alertdialog"]',
  );
}

/** Browser files (paste, drop, the file input) as attachments. */
async function attachmentsFromFiles(
  files: File[],
  images: boolean,
  harnessName: string,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const file of files) {
    const mime = Object.values(IMAGE_TYPES).includes(file.type) ? file.type : imageMime(file.name);
    if (mime) {
      if (!images) {
        toast(`${harnessName} does not take images`, "info");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        toast(`${file.name || "The image"} is over 20 MB`, "info");
        continue;
      }
      out.push({
        id: localId("image"),
        kind: "image",
        name: file.name || "Pasted image",
        mimeType: mime,
        data: await readFileAs(file, "base64"),
      });
    } else if (file.size <= MAX_INLINE_FILE_BYTES) {
      out.push({ id: localId("file"), kind: "file", name: file.name, text: await readFileAs(file, "text") });
    } else {
      toast(`${file.name} is too big to send inline here; the app attaches it by path`, "info");
    }
  }
  return out;
}

/** Paths from the open panel or a drop in the app: images are read, other files go by path. */
async function attachmentsFromPaths(
  paths: string[],
  images: boolean,
  harnessName: string,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const path of paths) {
    const name = basename(path) || path;
    if (imageMime(name) && images) {
      try {
        const image = await readImage(path);
        out.push({ id: localId("image"), kind: "image", ...image });
      } catch (error) {
        toast(`Could not attach ${name}: ${error instanceof Error ? error.message : String(error)}`, "error");
      }
    } else {
      if (imageMime(name)) toast(`${harnessName} does not take images; ${name} goes as a path`);
      out.push({ id: localId("file"), kind: "file", name, path });
    }
  }
  return out;
}

/** Re-render for what the composer shows, not for every streamed chunk (`items` is read live). */
function sameComposer(
  a: { thread?: ThreadState; compact?: boolean; className?: string },
  b: { thread?: ThreadState; compact?: boolean; className?: string },
): boolean {
  if (a.compact !== b.compact || a.className !== b.className) return false;
  if (a.thread === b.thread) return true;
  if (!a.thread || !b.thread) return false;
  for (const key of Object.keys(b.thread) as (keyof ThreadState)[]) {
    if (key !== "items" && key !== "toolIndex" && key !== "usage" && a.thread[key] !== b.thread[key])
      return false;
  }
  return Object.keys(a.thread).length === Object.keys(b.thread).length;
}

const NO_FILES: string[] = [];
/** How long a project's file list is used before `@` lists it again. */
const FILES_FRESH_MS = 30_000;

/** A row of the composer's drawer: a command, a group's keyword, an agent, a thread, or a file. */
type DrawerEntry =
  | { kind: "command"; command: SlashCommand }
  | { kind: "keyword"; scope: MentionScope }
  | { kind: "agent"; agent: AgentChoice }
  | { kind: "thread"; thread: ThreadChoice }
  | { kind: "file"; path: string };

interface DrawerGroup {
  label: string;
  entries: DrawerEntry[];
}

/** A row of the drawer as it is drawn: a heading, or a bound row the arrow keys can land on. */
type DrawerRowItem = { kind: "heading"; label: string } | { kind: "row"; entry: DrawerEntry; index: number };

/** No plans to draw: a machine's harness rows, whose plans this computer has not read. */
const NO_PLANS: Record<string, CachedPlan> = {};

/** What one login's plan has left across its windows: `62% left`, or nothing when it was not read. */
function planLeft(plan: CachedPlan | undefined): string | undefined {
  const windows = usageOf(plan)?.windows;
  if (!windows?.length) return undefined;
  return `${Math.max(0, Math.round(100 - Math.max(...windows.map((w) => w.used_percent))))}% left`;
}

/**
 * Who a delegate could run on: every available harness's models, with the model a new thread there
 * starts on first, and the effort, login and plan that go with each. The rows are interleaved one
 * model per harness, so the drawer's first rows show what every harness offers rather than one
 * harness's whole list, and the whole catalog is searched by name (`rankAgents`), not just what is
 * on screen.
 */
function agentChoices(
  harnesses: readonly Harness[],
  plans: Record<string, CachedPlan>,
  signedOut: SignedOutLogins,
  /** The thread the delegate would be started from: a child on its harness inherits its login. */
  parent: { harness?: string; account?: string | null },
): AgentChoice[] {
  const perHarness = orderHarnesses([...harnesses])
    .filter((h) => !unavailableReason(h))
    .map((h) => {
      const accounts = h.accounts ?? [];
      const accountId = accounts.length ? childAccountOn(h, parent, signedOut) : null;
      // Only a harness with logins to choose between names one on its rows.
      const account = accounts.length > 1 ? accounts.find((a) => a.id === accountId)?.name : undefined;
      const plan = planLeft(plans[harnessPlanKey(h.id, accountId)]);
      const lead = h.last?.model;
      const models = [...h.models].sort((a, b) => Number(b.id === lead) - Number(a.id === lead));
      if (!models.length) {
        return [
          {
            harness: h.id,
            harnessName: h.name,
            name: h.name,
            effort: h.capabilities.effort ? (h.last?.effort ?? undefined) : undefined,
            account,
            plan,
            token: h.id,
          },
        ];
      }
      return models.map((m) => ({
        harness: h.id,
        harnessName: h.name,
        model: m.id,
        name: m.name,
        effort: h.capabilities.effort
          ? m.id === lead && h.last?.effort
            ? h.last.effort
            : (defaultEffort(m, m.efforts) ?? undefined)
          : undefined,
        account,
        plan,
        token: `${h.id}/${m.id}`,
      }));
    });
  const rows: AgentChoice[] = [];
  const deepest = Math.max(0, ...perHarness.map((choices) => choices.length));
  for (let i = 0; i < deepest; i++) {
    for (const choices of perHarness) {
      const row = choices[i];
      if (row) rows.push(row);
    }
  }
  return rows;
}

/** The group keywords the drawer offers first, while what is typed is empty or still a prefix. */
function keywordEntries(query: string): DrawerEntry[] {
  const asked = query.trim().toLowerCase();
  return SCOPES.filter((scope) => scope.word.startsWith(asked)).map((scope) => ({
    kind: "keyword",
    scope: scope.scope,
  }));
}

/** The folder a thread is grouped by: a worktree's repo, else the thread's own folder. */
const projectKeyOf = (session: { cwd?: string; repo?: string; worktree?: { repo?: string } }) =>
  session.worktree?.repo || session.repo || session.cwd || "";

/**
 * What a thread is doing, as the `#` list orders it and says it: the app's live turn first, then what
 * the engine listed (a thread working in another window is working here too), and settled only when
 * the engine says so.
 */
function mentionStateOf(session: { status?: ThreadStatus }, working: boolean, asking: boolean): MentionState {
  // The app's own reading of a thread (state/store.ts `statusOf`): what it asks of you, then that it
  // works, then what the engine last listed.
  if (asking) return "needs";
  if (working) return "working";
  if (session.status === "needs_input") return "needs";
  if (session.status === "running") return "working";
  return session.status === "done" ? "settled" : "open";
}

/** What a thread's state says last in its row, in the sidebar's own words and tones. */
const MENTION_WORD: Record<MentionState, string> = {
  needs: "Needs you",
  working: "Working",
  open: "",
  settled: "Done",
};
const MENTION_TINT: Record<MentionState, string> = {
  needs: "text-warning",
  working: "text-accent",
  open: "",
  settled: "text-fg-muted",
};

// The drawer's row, shared by all four kinds: one line, the mark first, what it is last.
const DRAWER_ROW = "flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left";
const DRAWER_NAME = "min-w-[96px] max-w-[45%] shrink-0 truncate font-medium text-fg text-sub";

/** `text` with the letters the query matched marked, as the folder picker marks a folder's name. */
function Marked({ text, query }: { text: string; query: string }) {
  return (
    <>
      {matchSpans(text, query).map((span, i) =>
        span.hit ? (
          <mark key={i} className="rounded-xs bg-fill-strong text-fg">
            {span.text}
          </mark>
        ) : (
          <Fragment key={i}>{span.text}</Fragment>
        ),
      )}
    </>
  );
}

export const Composer = memo(function Composer({
  thread,
  compact,
  className,
}: {
  thread?: ThreadState;
  compact?: boolean;
  /** A modifier for the box itself: `rc-composer-over` on a new thread's picture (ui/ChatView.tsx). */
  className?: string;
}) {
  const key = thread?.id ?? "new";
  const draft = useApp((s) => s.drafts[key] ?? "");
  const extras = useApp((s) => s.extras[key] ?? emptyExtras);
  const editing = useApp((s) => s.editing[key]);
  // This thread's own session list, or else the menu of its harness (the engine's catalog for a new thread).
  const commands = useApp(useShallow((s) => commandsFor(s, thread?.id ?? null)));
  const projectCwd = useApp((s) => s.projectCwd);
  // The computer this composer speaks for: the thread's own, or the one **Run on** names for a new
  // thread. Its harnesses are what the `@` agents and the model picker offer (docs/desktop.md ›
  // Several machines).
  const newThreadHost = useApp((s) => s.newThreadHost);
  const host = thread ? splitThreadKey(thread.id).host : newThreadHost;
  const harnesses = useApp((s) => harnessesOn(s, host)?.harnesses); // re-render when harness capabilities arrive
  const status = useApp((s) => s.status);
  const field = useRef<HTMLTextAreaElement>(null);
  // The field's own box (the mirror and the field): what fades when the message leaves it.
  const fieldBox = useRef<HTMLDivElement>(null);
  // The field's twin: the draft is measured on it, never on the field, whose own box may be halfway
  // through its height transition (lib/fieldHeight.ts).
  const gauge = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [caret, setCaret] = useState(0);
  const [files, setFiles] = useState<{ cwd: string; at: number; paths: string[] } | null>(null);
  const [dropping, setDropping] = useState(false);
  // The send-later dialog: opened from the + menu, with the draft it is scheduling.
  const [sendLater, setSendLater] = useState(false);

  const running = thread?.running ?? false;
  // A thread on a paired computer that is not connected now is read-only: its turns run there,
  // and the header and a note over the field say which computer it is (state/store.ts `threadOffline`).
  const offline = useThreadOffline(thread?.id);
  const offMachine = useMachineOfThread(thread?.id);
  // What ⌘⏎ would do with what is in the field now, by the same rule the queued row uses: the
  // placeholder says it, so a message that will wait does not read as one that steers.
  const decision = thread ? sendDecisionFor(thread, extras) : undefined;
  // The queued message the composer is editing (its pencil, or ↑): ⏎ saves it where it waits, and
  // never sends it. What the composer held before comes back when the edit ends.
  const queuedEdit = useApp((s) => (s.queuedEdit?.threadId === key ? s.queuedEdit : null));
  // `/`: the commands, at the start or anywhere after whitespace, filtered by the
  // token the caret is in. `/usr/bin` is a path, not a command (state/compose.ts).
  const slash = dismissed === draft || extras.command ? null : slashToken(draft, caret);
  const commandQuery = slash?.query ?? null;
  const list = useMemo(
    () => (commandQuery === null ? [] : commandSuggestions(commandQuery, commands)),
    [commandQuery, commands],
  );
  // `@`: one drawer for agents, threads and the project's files, narrowed by a keyword (`@agents`,
  // `@threads`, `@files`); `#` opens it straight on the threads. The project's file list is read when
  // the first `@` opens it, and again once it is stale.
  const at = dismissed === draft || slash ? null : atToken(draft, caret);
  const cwdForFiles = thread?.cwd ?? projectCwd;
  const wantsFiles = !!at && !!cwdForFiles;
  useEffect(() => {
    if (!wantsFiles || (files?.cwd === cwdForFiles && Date.now() - files.at < FILES_FRESH_MS)) return;
    let live = true;
    void projectFiles(cwdForFiles).then((paths) => {
      if (live) setFiles({ cwd: cwdForFiles, at: Date.now(), paths });
    });
    return () => {
      live = false;
    };
  }, [wantsFiles, cwdForFiles, files]);
  // `#`: the threads this message can point at. It opens like `/` and `@` do, and never inside a
  // word, a heading or code (state/compose.ts `hashToken`).
  const hash = dismissed === draft || slash || at ? null : hashToken(draft, caret);
  // What is typed after the trigger, and the group the trigger narrowed to (`#` narrows to threads).
  const mentionQuery = hash?.query ?? at?.query ?? "";
  const scope: MentionScope | null = hash ? "threads" : (at?.scope ?? null);
  const sessions = useApp((s) => s.sessions);
  const links = useApp((s) => s.links);
  const planCache = useApp((s) => s.planCache);
  // The harness and login a new thread would start on: what a delegate it names would inherit.
  const newHarness = useApp((s) => newThreadChoice(s)?.harness);
  const newAccount = useApp((s) => s.newThreadAccount);
  const signedOut = useApp((s) => s.signedOutLogins);
  const openId = thread?.id;
  // The open thread's project: its own, or the folder a new thread will start in.
  const home = useMemo(() => {
    const open = openId ? sessions.find((s) => s.id === openId) : undefined;
    return open ? projectKeyOf(open) : cwdForFiles;
  }, [sessions, openId, cwdForFiles]);
  // The threads that work or ask right now, as their rows say it: read as lists so a streamed chunk
  // somewhere else does not redraw the composer.
  const working = useApp(
    useShallow((s) => Object.values(s.threads).flatMap((t) => (t.running ? [t.id] : []))),
  );
  const asking = useApp(
    useShallow((s) => Object.values(s.threads).flatMap((t) => (t.permissions.length ? [t.id] : []))),
  );
  const threadChoices = useMemo(() => {
    const workingIds = new Set(working);
    const askingIds = new Set(asking);
    return sessions.flatMap((session): ThreadChoice[] => {
      const title = session.title?.trim();
      // A thread cannot point at itself, and one with no name has nothing to match or to write.
      if (!title || session.id === openId) return [];
      const project = projectKeyOf(session);
      return [
        {
          id: session.id,
          title,
          harness: session.harness,
          project: project ? projectName(project) : undefined,
          home: project === home || session.cwd === home,
          state: mentionStateOf(session, workingIds.has(session.id), askingIds.has(session.id)),
          updatedAt: session.updatedAt,
        },
      ];
    });
  }, [sessions, openId, home, working, asking]);
  // The threads this one started and still runs, at any depth: the tree the details panel draws.
  const delegates = useMemo(() => {
    if (!openId) return [];
    const workingIds = new Set(working);
    const askingIds = new Set(asking);
    const found: ThreadChoice[] = [];
    const walk = (nodes: readonly DelegateNode[]) => {
      for (const node of nodes) {
        const session = sessions.find((s) => s.id === node.id);
        const title = session?.title?.trim();
        // What the thread itself reports wins over the link's own record, as the details tree reads
        // it (state/delegates.ts `agentStateOf`); the app's live turn, when it has one, first of all.
        const activity = askingIds.has(node.id)
          ? "needs_input"
          : workingIds.has(node.id)
            ? "running"
            : session?.status;
        const state = agentStateOf(node.link, activity);
        if (session && title && (state === "working" || state === "needs")) {
          found.push({
            id: node.id,
            title,
            harness: session.harness,
            state,
            delegate: true,
            home: true,
            updatedAt: session.updatedAt,
          });
        }
        walk(node.children);
      }
    };
    walk(delegateTree(openId, links.links));
    return found;
  }, [openId, sessions, links, working, asking]);
  // `@`'s agents: who a delegate could run on, on which model, at which effort.
  // The thread a delegate would be started from: a child on this harness inherits its login.
  const parent = useMemo(
    () => ({
      harness: openId ? sessions.find((x) => x.id === openId)?.harness : newHarness,
      account: openId ? (sessions.find((x) => x.id === openId)?.account ?? null) : newAccount,
    }),
    [openId, sessions, newHarness, newAccount],
  );
  // A machine's harness rows carry no plan: `planCache` is this computer's `_rustcode/plans`,
  // and a login that runs there is not the one whose windows it read.
  const plans = host ? NO_PLANS : planCache;
  const agents = useMemo(
    () => agentChoices(harnesses ?? [], plans, signedOut, parent),
    [harnesses, plans, signedOut, parent],
  );
  const fileList = useMemo(
    () => (files?.cwd === cwdForFiles ? rankFiles(files.paths, mentionQuery) : NO_FILES),
    [files, cwdForFiles, mentionQuery],
  );
  const mentioning = !!hash || !!at;
  const drawerGroups = useMemo((): DrawerGroup[] => {
    if (slash) return [{ label: "", entries: list.map((command) => ({ kind: "command", command })) }];
    if (!mentioning) return [];
    const groups: DrawerGroup[] = [];
    if (!scope || scope === "agents") {
      // The harnesses and their models first, then the threads this one is already running.
      const rows: DrawerEntry[] = [
        ...rankAgents(mentionQuery, agents).map((agent): DrawerEntry => ({ kind: "agent", agent })),
        ...rankThreads(delegates, mentionQuery).map((thread): DrawerEntry => ({ kind: "thread", thread })),
      ];
      if (rows.length) groups.push({ label: "Agents", entries: rows });
    }
    if (!scope || scope === "threads") {
      const threads = rankThreads(threadChoices, mentionQuery);
      if (threads.length)
        groups.push({
          label: "Threads",
          entries: threads.map((thread): DrawerEntry => ({ kind: "thread", thread })),
        });
    }
    if (!scope || scope === "files") {
      if (fileList.length)
        groups.push({
          label: "Files",
          entries: fileList.map((path): DrawerEntry => ({ kind: "file", path })),
        });
    }
    // The keywords first, so the drawer says how to look in one group before it lists them.
    const keywords = !scope ? keywordEntries(mentionQuery) : [];
    if (keywords.length) groups.unshift({ label: "", entries: keywords });
    return groups;
  }, [slash, mentioning, scope, mentionQuery, list, agents, delegates, threadChoices, fileList]);
  // The drawer's rows, as drawn: a group's heading, then its rows, and in one bound list so the
  // arrow keys walk the rows across the groups and never land on a heading.
  const drawerRows = useMemo((): DrawerRowItem[] => {
    const labelled = drawerGroups.filter((group) => group.entries.length).length > 1;
    const rows: DrawerRowItem[] = [];
    let index = 0;
    for (const group of drawerGroups) {
      if (!group.entries.length) continue;
      if (labelled && group.label) rows.push({ kind: "heading", label: group.label });
      for (const entry of group.entries) rows.push({ kind: "row", entry, index: index++ });
    }
    return rows;
  }, [drawerGroups]);
  const entries = useMemo(
    () => drawerRows.flatMap((row) => (row.kind === "row" ? [row.entry] : [])),
    [drawerRows],
  );
  // What the drawer is: `/`'s commands, or `@` and `#`'s agents, threads and files.
  const drawer = slash ? "commands" : hash || at ? "mentions" : null;
  const drawerQuery = mentionQuery;
  const harness = threadHarness(thread);
  const harnessName = harness?.name ?? "This harness";
  // Who the message goes to: the thread's harness, or for a new thread the one it will start on.
  const speakingTo = useApp((s) => {
    if (harness) return harness.name;
    const id = newThreadChoice(s)?.harness;
    return harnessesOn(s, s.newThreadHost)?.harnesses.find((h) => h.id === id)?.name ?? "RustCode";
  });
  const images = acceptsImages(thread);
  const cancelReason = controlDisabledReason("cancel", thread?.harness?.capabilities, harnessName);
  const cwd = thread?.cwd ?? projectCwd;
  const canSend = !!draft.trim() || hasExtras(extras);
  const mirror = useRef<HTMLDivElement>(null);
  const shell = useRef<HTMLDivElement>(null);
  const drawerRef = useRef<HTMLDivElement>(null);
  // Where the field is in what ↑/↓ walks (`sentMessages`), and the draft the walk started from.
  const history = useRef<{ draft: string; index: number } | null>(null);
  // Anything the mirror has to draw: a URL, a `@file` reference, or a `#thread` mention
  // (state/compose.ts `splitTokens`).
  const mentions = extras.mentions;
  const tokenized = useMemo(() => hasTokens(draft, mentions), [draft, mentions]);
  // Which edges of the scrolled field fade (lib/fade.ts); nothing to fade while the draft fits.
  const [fade, setFade] = useState<FieldFade>("none");
  // Carried by the field and the mirror alike: the mask (`.rc-field-fade`) takes its two depths.
  const fadeStyle = {
    "--rc-fade-top": fade === "top" || fade === "both" ? `${FIELD_FADE}px` : "0px",
    "--rc-fade-bottom": fade === "bottom" || fade === "both" ? `${FIELD_FADE}px` : "0px",
  } as React.CSSProperties;
  // The field's scroll is read twice: the mirror keeps the highlighted text under the textarea, and
  // the fade follows which edge the draft is cut at.
  const syncField = () => {
    const el = field.current;
    if (!el) return;
    if (mirror.current && mirror.current.scrollTop !== el.scrollTop) mirror.current.scrollTop = el.scrollTop;
    setFade(fieldFade(el.scrollTop, el.scrollHeight, el.clientHeight));
  };

  // A draft taller than the box scrolls, and the browser parks it so the caret's line only just
  // clears the bottom edge, leaving a line's worth of slack under it. The fade then dims the very
  // line being typed on (a new line after ⇧⏎ lands in the shadow). Typing at the end of the draft
  // asks to see its end, so go all the way down: the scroll range is whole lines, so the end is a
  // line boundary too, the last line reads clear of the fade, and the fade itself turns off there.
  const followCaret = () => {
    const el = field.current;
    if (!el || el.scrollHeight <= el.clientHeight) return;
    if (el.selectionStart === el.value.length && el.selectionStart === el.selectionEnd) {
      el.scrollTop = el.scrollHeight;
    }
  };

  // The field's height follows its draft: one line box per line, between the two-line minimum and
  // the point it scrolls. Measured on the twin at zero height, so the number is the text's own and
  // not the box the field is still animating towards; the field is then given the final height once,
  // and its transition (index.css) runs from where it was to where it belongs, growing or shrinking.
  const fitHeight = () => {
    const el = field.current;
    if (!el) return;
    const twin = gauge.current;
    const height = twin
      ? measuredFieldHeight(twin, { value: el.value, width: el.clientWidth }, MIN_HEIGHT, MAX_HEIGHT)
      : Math.min(Math.max(el.scrollHeight, MIN_HEIGHT), MAX_HEIGHT);
    el.style.height = `${height}px`;
  };

  // Ready to type on entering a thread, and again on coming back to the chat from Settings or
  // another view (the draft is kept either way).
  const inChat = useApp((s) => !s.settings && s.view.kind === "chat" && !s.switcherOpen);
  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus on thread change
  useEffect(() => {
    history.current = null;
    if (inChat) field.current?.focus();
  }, [key, inChat]);

  // Grow from two lines to the field's height, then scroll.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measure on every draft change
  useLayoutEffect(() => {
    fitHeight();
    followCaret();
    syncField();
    // The browser parks the field at the caret a frame after a new value lands, so ask again then:
    // a pasted draft would otherwise show a hard top edge until the next scroll, and the caret it
    // scrolled to would sit in the fade.
    const frame = requestAnimationFrame(() => {
      followCaret();
      syncField();
    });
    return () => cancelAnimationFrame(frame);
  }, [draft]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new suggestion list starts at the top
  useEffect(() => setSelected(0), [entries]);

  // A narrower window rewraps the draft, so the field can start or stop scrolling: measure again.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the field and the mirror are refs
  useEffect(() => {
    const el = field.current;
    if (!el) return;
    return observeSize(el, () => {
      fitHeight();
      syncField();
    });
  }, []);

  const update = (text: string) => {
    // A paste is a chip above the field, not text in it: its offset moves with every edit before it,
    // so the message can put it back where it was pasted (`state/compose.ts` `trackPastes`).
    const state = useApp.getState();
    const before = state.drafts[key] ?? "";
    const attachments = state.extras[key]?.attachments ?? [];
    const tracked = trackPastes(attachments, before, text);
    const mentions = state.extras[key]?.mentions;
    // A mention whose `#Title` was deleted has nothing left to say in the message.
    const kept = pruneMentions(text, mentions);
    const stale = kept.length !== (mentions?.length ?? 0);
    if (tracked !== attachments || stale)
      change((e) => ({
        ...e,
        attachments: tracked === attachments ? e.attachments : [...tracked],
        mentions: kept,
      }));
    setDraft(key, text);
  };
  // A message walked back into the field puts the caret at its end, so ↓ comes forward again at once.
  const recall = (text: string) => {
    update(text);
    requestAnimationFrame(() => {
      const el = field.current;
      if (el) el.selectionStart = el.selectionEnd = el.value.length;
    });
  };
  const change = (fn: (e: ComposeExtras) => ComposeExtras) => setExtras(key, fn);

  // A queued message that leaves the queue while it is open here (it started, or it was removed)
  // ends its edit; the store keeps what the user typed only when the composer was empty before it.
  const queue = thread?.queue;
  useEffect(() => {
    if (!queuedEdit) return;
    if (queuedEdit.threadId !== key || !queue?.some((q) => q.id === queuedEdit.queuedId)) endQueuedEdit();
  }, [key, queue, queuedEdit]);
  const attach = (more: Attachment[]) => {
    if (more.length) change((e) => ({ ...e, attachments: [...e.attachments, ...more] }));
  };
  // A picture WebKitGTK keeps to the native clipboard (Linux): the shell reads it as the PNG an
  // attachment carries, and a harness that does not take images refuses it as it does a dropped one.
  const pasteFromClipboard = async () => {
    let image: ImageFile | null;
    try {
      image = await clipboardImage();
    } catch (error) {
      toast(`Could not paste the image: ${error instanceof Error ? error.message : String(error)}`, "error");
      return;
    }
    if (!image) return;
    if (!images) {
      toast(`${harnessName} does not take images`, "info");
      return;
    }
    attach([{ id: localId("image"), kind: "image", ...image }]);
  };

  // Files dropped on the app window arrive as paths from the shell.
  const dropped = useRef<(paths: string[]) => void>(() => undefined);
  dropped.current = (paths) => void attachmentsFromPaths(paths, images, harnessName).then(attach);
  useEffect(() => {
    let stop: (() => void) | undefined;
    let live = true;
    void onFileDrop((event) => {
      setDropping(event.type === "over");
      if (event.type === "drop" && event.paths.length) dropped.current(event.paths);
    }).then((unlisten) => {
      if (live) stop = unlisten;
      else unlisten();
    });
    return () => {
      live = false;
      stop?.();
    };
  }, []);

  const approval = thread?.permissions.find((p) => !p.question);
  const send = (steer: boolean) => {
    if (offline) return;
    haptic("generic");
    // ⏎ on a queued message being edited saves it in the queue: nothing is sent.
    if (queuedEdit) {
      saveQueuedEdit();
      field.current?.focus();
      return;
    }
    const text = draft;
    const sentExtras = extras;
    if (!text.trim() && !hasExtras(sentExtras) && !(steer && running)) return;
    // A message sent while an approval waits declines it: the message says what to do instead.
    if (approval && (text.trim() || hasExtras(sentExtras))) {
      const decline =
        approval.options.find((o) => o.kind === "reject_once") ??
        approval.options.find((o) => o.kind === "reject_always");
      if (decline) answerPermission(approval, decline);
    }
    update("");
    change(() => emptyExtras);
    // The field comes back empty in a breath: the message has left for the transcript, not blinked.
    fadeClearedField(fieldBox.current);
    const sending = steer ? steerOrSubmit(text, sentExtras) : submit(text, sentExtras);
    void sending.then((sent) => {
      // A thread that could not start keeps the message for another try.
      if (!sent && !useApp.getState().drafts[key]) {
        // Set the draft directly: the pastes were never moved, and putting the text back must not
        // move them now (the extras are restored with it).
        setDraft(key, text);
        change(() => sentExtras);
      }
    });
  };

  // Schedule what is in the field for a time the user picked: it leaves the field as a sent message
  // would, into the card over the composer. A thread that is busy when its time comes queues it.
  const schedule = (due: number) => {
    if (!thread) return;
    if (!scheduleMessage(thread.id, draft, extras, due)) {
      toast("Could not schedule that message.", "error");
      return;
    }
    setSendLater(false);
    update("");
    change(() => emptyExtras);
    fadeClearedField(fieldBox.current);
  };

  const reference = (path: string) => {
    if (!at) return;
    const next = insertRef(draft, at, path);
    update(next.text);
    setCaret(next.caret);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
  };

  const complete = (command: SlashCommand, run: boolean) => {
    if (!slash) return;
    // A leading command keeps its path. A command the app answers itself (a picker, a new
    // thread) runs at once, arguments or not; one that wants arguments becomes the token in
    // front of the field.
    if (slash.start === 0 && slash.start + 1 + slash.query.length === draft.length) {
      if (run && (!command.takesArguments || command.action !== "execute")) {
        update("");
        void submit(command.name);
      } else {
        change((e) => ({ ...e, command }));
        update("");
      }
      return;
    }
    // Anywhere else the token is text: `/name ` is written where the caret was, as `@` is
    // (the harness reads a slash command only at the start of a prompt).
    const next = insertCommand(draft, slash, command.name);
    update(next.text);
    setCaret(next.caret);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
  };

  const addMention = (mention: Mention) => {
    // The token the drawer was opened by: `#` for the threads, `@` for the whole drawer — and a
    // delegate can come from either, since `@` lists the threads this one runs.
    const found = mention.kind === "thread" && hash ? hash : at;
    if (!found) return;
    const next = insertMention(draft, found, mention);
    update(next.text);
    // The id travels with the message, kept beside the text: two threads may share a title, and
    // the draft's own words say only the title (state/compose.ts `Mention`).
    change((e) => ({ ...e, mentions: [...(e.mentions ?? []), mention] }));
    setCaret(next.caret);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
  };

  // A group's keyword is written at the token, so the drawer stays open on that group and the next
  // word is what is looked for in it.
  const narrow = (scope: MentionScope) => {
    if (!at) return;
    const next = insertKeyword(draft, at, scope);
    update(next.text);
    setCaret(next.caret);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
  };

  // Accepting a row: `Enter` runs a command the app answers itself, as it always has; the arrows'
  // own keys only ever write a token.
  const accept = (entry: DrawerEntry, run: boolean) => {
    switch (entry.kind) {
      case "command":
        complete(entry.command, run);
        return;
      case "file":
        reference(entry.path);
        return;
      case "keyword":
        // Narrowing keeps the drawer open: the next word is what is looked for in that group.
        narrow(entry.scope);
        return;
      case "agent":
        // The token is the harness and model, one word the agent can act on; the message spells out
        // the effort beside it (state/compose.ts `mentionText`).
        addMention({
          kind: "agent",
          harness: entry.agent.harness,
          model: entry.agent.model,
          effort: entry.agent.effort,
          token: entry.agent.token,
        });
        return;
      case "thread":
        addMention({ kind: "thread", id: entry.thread.id, title: entry.thread.title });
        return;
    }
  };

  const onChange = (text: string) => {
    // `/name ` typed out becomes the token too, as in the SwiftUI app.
    const typed = /^(\/\S+) (.*)$/s.exec(text);
    const command = typed && !extras.command ? commands.find((c) => c.name === typed[1]) : undefined;
    if (command?.takesArguments) {
      change((e) => ({ ...e, command }));
      update(typed![2] ?? "");
    } else {
      update(text);
    }
    setDismissed(null);
  };

  const onPaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    // A picture comes as a file, as an item, or not at all: macOS fills `files`, WebKitGTK on Linux
    // can list it only as an item, and sometimes keeps it to the native clipboard (lib/paste.ts,
    // lib/native.ts).
    const files = pastedImages(e.clipboardData);
    if (files.length) {
      e.preventDefault();
      void attachmentsFromFiles(files, images, harnessName).then(attach);
      return;
    }
    // A long block (a log, a file, a stack trace) would turn the draft into a wall of text and the
    // card into a tower: it becomes a chip above the field, and goes back into the message where the
    // caret was when it is sent. A selection is taken by the paste, as it would be in the field.
    const text = e.clipboardData.getData("text/plain");
    if (isLongPaste(text)) {
      e.preventDefault();
      const at = e.currentTarget.selectionStart ?? draft.length;
      const end = e.currentTarget.selectionEnd ?? at;
      if (end > at) update(`${draft.slice(0, at)}${draft.slice(end)}`);
      attach([{ id: localId("paste"), kind: "text", text, at }]);
      return;
    }
    // A pasted link stays where it was pasted, as text; the sent message shows it as a chip.
    if (text) return;
    // The web view saw nothing to paste, which is how a copied screenshot reaches Linux: the shell
    // reads the picture from the native clipboard instead. Nothing there either leaves the paste
    // empty, as it would have been.
    e.preventDefault();
    void pasteFromClipboard();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // An IME's Enter (keyCode 229) belongs to the field: it commits a candidate. Taking it would
    // complete a command or send a draft that was still being written.
    if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
    if (entries.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : entries.length - 1)) % entries.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !primaryModifier(e))) {
        e.preventDefault();
        const entry = entries[selected] ?? entries[0];
        if (entry) accept(entry, e.key === "Enter");
        return;
      }
    }
    // An open drawer takes the Escape even when nothing matches it: it is what the reader sees.
    if (drawer && e.key === "Escape") {
      e.preventDefault();
      setDismissed(draft);
      return;
    }
    const el = e.currentTarget;
    if (e.key === "Backspace" && el.selectionStart === 0 && el.selectionEnd === 0) {
      // ⌫ at the start takes back the last chip: a link, then the command (as text).
      if (extras.links.length) {
        e.preventDefault();
        change((x) => ({ ...x, links: x.links.slice(0, -1) }));
        return;
      }
      if (extras.command) {
        e.preventDefault();
        const name = extras.command.name;
        change((x) => ({ ...x, command: undefined }));
        update(`${name}${draft ? ` ${draft}` : ""}`);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
      // A held Enter sends one message, not one a frame.
      if (e.repeat) return;
      e.preventDefault();
      send(primaryModifier(e));
      return;
    }
    if (e.key === "Escape") {
      // The topmost thing goes first: an open popover, menu or dialog takes this Escape for
      // itself, and the draft and the running turn stay as they are (issue 2804).
      if (overlayOpen()) return;
      // A queued message open here goes back as it was, and the composer gets its own message back.
      if (queuedEdit) {
        e.preventDefault();
        cancelQueuedEdit();
        return;
      }
      if (draft || hasExtras(extras)) {
        e.preventDefault();
        update("");
        change(() => emptyExtras);
        return;
      }
      // Nothing here to take it: no preventDefault, so an open toast can dismiss (ui/Toaster.tsx).
      if (running) {
        e.preventDefault();
        void cancel(thread?.id);
      }
      return;
    }
    // ↑ and ↓ walk what this thread was sent, newest first, a line at a time: ↑ at the first line
    // goes back, ↓ at the last line comes forward, and past the newest the draft returns. What is
    // queued here opens before the history does (it is edited, never sent again).
    if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !hasExtras(extras) && thread && !queuedEdit) {
      if (el.selectionStart !== el.selectionEnd) return;
      // The first and last line, not the first and last character: a recalled message keeps ↑ and ↓
      // walking from its own ends, and a draft of several lines still moves its caret line by line.
      const atFirst = !el.value.slice(0, el.selectionStart).includes("\n");
      const atLast = !el.value.slice(el.selectionStart).includes("\n");
      const live = useApp.getState().threads[thread.id] ?? thread;
      const walking = history.current;
      if (e.key === "ArrowUp") {
        if (!atFirst) return;
        if (!walking) {
          if (draft) return;
          const queued = [...live.queue].reverse().find((q) => !q.link && !q.wake);
          if (queued) {
            e.preventDefault();
            startQueuedEdit(thread.id, queued.id);
            return;
          }
        }
        const sent = sentMessages(live.items ?? thread.items);
        if (!sent.length) return;
        e.preventDefault();
        const index = walking ? Math.min(walking.index + 1, sent.length - 1) : 0;
        history.current = { draft: walking?.draft ?? draft, index };
        recall(sent[sent.length - 1 - index]!);
        return;
      }
      if (!walking || !atLast) return;
      e.preventDefault();
      if (walking.index === 0) {
        history.current = null;
        recall(walking.draft);
        return;
      }
      const sent = sentMessages(live.items ?? thread.items);
      const index = walking.index - 1;
      history.current = { draft: walking.draft, index };
      recall(sent[sent.length - 1 - index] ?? walking.draft);
    }
  };

  const chooseFiles = () => {
    void pickFiles(cwd).then((paths) => {
      if (paths === null) fileInput.current?.click();
      else void attachmentsFromPaths(paths, images, harnessName).then(attach);
    });
  };

  const placeholder = offline
    ? `Not connected${offMachine ? ` to ${offMachine}` : ""}. This thread is read-only until it is back…`
    : queuedEdit
      ? "Edit the queued message: ⏎ saves it in the queue, esc leaves it…"
      : extras.command
        ? extras.command.description || "Arguments…"
        : approval
          ? "Send a message to decline and redirect the agent…"
          : running
            ? decision?.action === "steer"
              ? `⏎ queues it for the next turn, ${formatBinding("mod+enter")} steers this one…`
              : decision?.action === "interruptRestart"
                ? "⏎ stops this turn and starts a new one…"
                : "⏎ queues it for the next turn…"
            : `Message ${speakingTo}…`;

  // The drawer over the box: the commands `/` is looking for, the threads `#` is, or the agents and
  // files `@` is. A token that matches nothing still opens it and says so, where the field is about
  // to be typed in. It rises from the box's top edge, so it never covers the line being typed on.
  const drawerLabel = drawer === "commands" ? "Commands" : "Agents, threads and files";
  // What the drawer says when nothing answers what is typed, by what was asked for.
  const emptyText =
    drawer === "commands"
      ? "No matching command."
      : scope === "agents"
        ? "No agents match."
        : scope === "threads"
          ? "No threads match."
          : scope === "files"
            ? "No matching files or folders."
            : "No agents, threads or files match.";

  // The drawer reaches up over the transcript, under the box's top edge. It tells the chat column how
  // far it does (`--rc-composer-float`, from the foot of the pane), so the jump-to-latest pill sits
  // above it instead of over it (ui/Transcript.tsx). Nothing floats above the pane otherwise.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measured when the drawer opens and closes
  useLayoutEffect(() => {
    const main = shell.current?.closest<HTMLElement>('main[aria-label="Chat"]');
    if (!main) return;
    const publish = () => {
      const el = drawerRef.current;
      const parent = el?.offsetParent as HTMLElement | null | undefined;
      // `offsetTop`, not the drawer's own rect: the drawer rises in with `rc-in`, and a rect read
      // mid-animation leaves the pill a few px short of the drawer's settled top.
      const top =
        el && parent
          ? main.getBoundingClientRect().bottom - parent.getBoundingClientRect().top - el.offsetTop
          : 0;
      main.style.setProperty("--rc-composer-float", `${Math.max(0, Math.round(top))}px`);
    };
    publish();
    const stop = drawerRef.current ? observeSize(drawerRef.current, publish) : undefined;
    window.addEventListener("resize", publish);
    return () => {
      stop?.();
      window.removeEventListener("resize", publish);
      main.style.removeProperty("--rc-composer-float");
    };
  }, [drawer]);

  // The scroll follows the arrow keys: the list runs past the drawer's height, the row it is on
  // stays in view.
  const selectedRow = useRef<HTMLButtonElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the row under the keys is the one that moves
  useEffect(() => {
    selectedRow.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const drawerRow = (entry: DrawerEntry, i: number) => {
    const picked = i === selected;
    const row = {
      type: "button" as const,
      role: "option",
      "aria-selected": picked,
      ref: picked ? selectedRow : undefined,
      onMouseDown: (e: React.MouseEvent) => e.preventDefault(),
      onMouseEnter: () => setSelected(i),
      className: cx(DRAWER_ROW, picked && "bg-fill"),
    };
    switch (entry.kind) {
      case "command":
        return (
          <button key={`c:${entry.command.name}`} {...row} onClick={() => complete(entry.command, true)}>
            <Command size={16} className="shrink-0 text-fg-muted" />
            {/* A command that wants arguments says so before it is picked. */}
            <span className={DRAWER_NAME}>
              {entry.command.name}
              {entry.command.takesArguments && <span className="text-fg-faint"> …</span>}
            </span>
            <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">
              {entry.command.description}
            </span>
            {/* What answers it: the app itself, not the harness. */}
            {(entry.command.action !== "execute" || entry.command.report) && (
              <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 text-caption text-fg-muted">
                {entry.command.action === "skill" ? "Skill" : "App"}
              </span>
            )}
          </button>
        );
      case "file": {
        const cut = entry.path.lastIndexOf("/") + 1;
        return (
          <button key={`f:${entry.path}`} {...row} onClick={() => reference(entry.path)}>
            <FileText size={16} className="shrink-0 text-fg-muted" />
            <span className={DRAWER_NAME}>{entry.path.slice(cut)}</span>
            <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">{entry.path.slice(0, cut)}</span>
            <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 text-caption text-fg-muted">
              Project
            </span>
          </button>
        );
      }
      case "keyword": {
        const keyword = SCOPES.find((candidate) => candidate.scope === entry.scope)!;
        return (
          <button key={`k:${keyword.word}`} {...row} onClick={() => accept(entry, false)}>
            {/* No mark: this row is not something to point at, it says where to look. */}
            <span className="w-4 shrink-0" />
            <span className={DRAWER_NAME}>
              <Marked text={keyword.label} query={drawerQuery} />
            </span>
            <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">{keyword.hint}</span>
            <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 font-mono text-caption text-fg-muted">
              @{keyword.word}
            </span>
          </button>
        );
      }
      case "agent":
        return (
          <button key={`a:${entry.agent.token}`} {...row} onClick={() => accept(entry, false)}>
            <HarnessIcon
              id={entry.agent.harness}
              name={entry.agent.harnessName}
              size={16}
              className="shrink-0"
            />
            <span className={DRAWER_NAME}>
              <Marked text={entry.agent.name} query={drawerQuery} />
            </span>
            {/* Which harness, at which effort: what the row delegates to. */}
            <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">
              {entry.agent.model
                ? `${entry.agent.harnessName}${entry.agent.effort ? ` · ${entry.agent.effort}` : ""}`
                : entry.agent.effort}
            </span>
            <span className="ml-auto flex shrink-0 items-center gap-2">
              {entry.agent.account && (
                <span className="max-w-[120px] truncate text-caption text-fg-muted">
                  {entry.agent.account}
                </span>
              )}
              {entry.agent.plan && (
                <span className="text-caption text-fg-faint tabular">{entry.agent.plan}</span>
              )}
              {/* The token the row writes, so what it will say in the message is no surprise. */}
              <span className="max-w-[220px] truncate rounded-xs bg-fill px-1.5 font-mono text-caption text-fg-muted">
                @{entry.agent.token}
              </span>
            </span>
          </button>
        );
      case "thread": {
        const thread = entry.thread;
        return (
          <button key={`t:${thread.id}`} {...row} onClick={() => accept(entry, false)}>
            <HarnessIcon
              id={thread.harness ?? "rustcode"}
              name={thread.harness}
              size={16}
              className="shrink-0"
            />
            <span className={DRAWER_NAME}>
              <Marked text={thread.title} query={drawerQuery} />
            </span>
            {thread.project && !thread.home && (
              <span className="min-w-0 flex-1 truncate text-caption text-fg-faint">{thread.project}</span>
            )}
            <span className="ml-auto flex shrink-0 items-center gap-2">
              {thread.delegate && (
                <span className="rounded-xs bg-fill px-1.5 text-caption text-fg-muted">Delegate</span>
              )}
              {MENTION_WORD[thread.state] && (
                <span className={cx("font-medium text-caption", MENTION_TINT[thread.state])}>
                  {MENTION_WORD[thread.state]}
                </span>
              )}
              {thread.updatedAt !== undefined && (
                <span className="text-caption text-fg-faint tabular">{shortAge(thread.updatedAt)}</span>
              )}
            </span>
          </button>
        );
      }
    }
  };

  return (
    <div ref={shell} className={cx("rc-composer-shell relative @container/composer", className)}>
      {thread && <QueuedAnnouncer threadId={thread.id} />}
      {/* What waits for a time the user picked, over the box: it leaves the transcript's own lane
          alone, and the stack's bottom edge carries the box down as the cards come and go. */}
      {thread && <ScheduledMessages threadId={thread.id} />}
      {/* The box: the field and how it is sent. The drawer tucks under its top, the tray under its
          foot, so the three read as one floating surface over the transcript. */}
      <div className="relative">
        {drawer && (
          <div
            ref={drawerRef}
            className="rc-composer-drawer rc-in absolute inset-x-6 bottom-[calc(100%-1rem)] z-0 flex max-h-72 flex-col gap-0.5 overflow-y-auto px-1.5 pt-1.5 pb-[calc(1rem+6px)]"
            role="listbox"
            aria-label={drawerLabel}
          >
            {/* Nothing at all to offer: the drawer says so where the field is about to be typed in. */}
            {!entries.length && <p className="px-5 pt-4 pb-7 text-fg-muted text-sub">{emptyText}</p>}
            {drawerRows.map((item) =>
              item.kind === "heading" ? (
                // A group's name: the rows under it are the drawer's options, never this line.
                <div
                  key={`g:${item.label}`}
                  role="presentation"
                  className="px-2 pt-1.5 pb-0.5 font-medium text-caption text-fg-muted"
                >
                  {item.label}
                </div>
              ) : (
                drawerRow(item.entry, item.index)
              ),
            )}
          </div>
        )}
        <div
          className={cx(
            // The field's own card: glass, floating over the transcript. Focus steps the glass;
            // there is no ring or colour. Its radius is its own, larger step (index.css). A draft
            // that gains or loses a line grows the card over 150ms instead of jumping; the
            // transcript, pinned to its end, follows it up.
            "rc-composer relative z-10 flex flex-col gap-1",
            dropping && "outline-2 outline-accent outline-dashed",
          )}
          // Where a message sent from here comes from: its row in the transcript rises out of this
          // edge (lib/sendFlight.ts).
          data-rc-composer=""
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) {
              e.preventDefault();
              field.current?.focus();
            }
          }}
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes("Files")) {
              e.preventDefault();
              setDropping(true);
            }
          }}
          onDragLeave={() => setDropping(false)}
          onDrop={(e) => {
            if (!e.dataTransfer.files.length) return;
            e.preventDefault();
            setDropping(false);
            void attachmentsFromFiles([...e.dataTransfer.files], images, harnessName).then(attach);
          }}
        >
          {thread && <LimitWaitRow threadId={thread.id} />}
          {queuedEdit && (
            // A queued message is edited here, not in an input of its own: its text, command, links
            // and attachments are the composer's while it is open, and ⏎ saves them where it waits.
            <div className="flex items-center gap-1.5 px-5 pt-2 text-caption text-accent">
              <Pencil size={11} />
              <b>Editing a queued message</b>
              <span className="text-fg-muted">· ⏎ saves it in the queue, esc leaves it</span>
              <Pill quiet size="sm" className="ml-auto" onClick={() => cancelQueuedEdit()}>
                Cancel
              </Pill>
            </div>
          )}
          {editing && (
            <div className="flex items-center gap-1.5 px-5 pt-2 text-caption text-accent">
              <Pencil size={11} />
              <b>Editing a sent message</b>
              <span className="text-fg-muted">· sending takes the thread back to it</span>
              <Pill
                quiet
                size="sm"
                className="ml-auto"
                onClick={() => {
                  cancelEdit(key);
                  update("");
                }}
              >
                Cancel
              </Pill>
            </div>
          )}
          {extras.attachments.length > 0 && (
            <AttachmentStrip
              attachments={extras.attachments}
              onRemove={(id) =>
                change((e) => ({ ...e, attachments: e.attachments.filter((a) => a.id !== id) }))
              }
            />
          )}
          {/* The field and what reads in front of it. The chip and the links drop the field's own 6px
              of padding so they sit on its first line box, not above it. Every text row in the card
              starts at the same 20px (`px-5`), the bigger card's own inset; the tray is inset 12px
              (`px-3`) because its round controls carry a few px of their own, which puts their icons
              on the same edge. */}
          <div
            className={cx(
              "flex min-w-0 flex-wrap items-start gap-x-1.5 gap-y-1 px-5",
              // Pictures above already carry their own band and padding: the field then sits close
              // under them, while a field with nothing above it keeps the card's own air.
              extras.attachments.length > 0 ? "pt-1.5" : "pt-3",
            )}
          >
            {extras.command && (
              // One line of the field's own height (21px), so the command and its argument read
              // as one entry: the same leading keeps their baselines together.
              <Tooltip label={extras.command.description}>
                <span className="mt-1.5 flex h-[21px] shrink-0 items-center rounded-xs bg-accent/12 px-1.5 font-semibold text-accent text-body leading-[21px] rc-in">
                  {extras.command.name}
                </span>
              </Tooltip>
            )}
            {extras.links.map((url) => (
              // The field's own 6px of top padding: the chip takes the first line box's height
              // (`variant="token"`), so it sits on that line rather than above it.
              <LinkChip
                key={url}
                url={url}
                variant="token"
                className="mt-1.5 rc-in"
                onRemove={() => change((e) => ({ ...e, links: e.links.filter((l) => l !== url) }))}
              />
            ))}
            {/* Tokens in the draft read as tokens: a mirror under the field draws the same text with
                  each URL on an accent pill and each `@file` reference on a quiet one, and the field's
                  own text is transparent so the mirror shows through while the caret, selection and
                  editing stay the field's. The two take the same padding, the same scroll offset and
                  the same mask, so neither drifts from the other; a token's span adds no horizontal
                  padding, which is what keeps the caret on its character. */}
            <div ref={fieldBox} className="relative min-w-[160px] flex-1 basis-[160px]">
              {tokenized && (
                <div
                  ref={mirror}
                  aria-hidden
                  className="rc-field-fade pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words py-1.5 text-body text-fg leading-normal"
                  style={fadeStyle}
                >
                  {splitTokens(draft, mentions).map((part, i) =>
                    part.url ? (
                      <span
                        key={i}
                        className="rounded-xs bg-accent/15 text-accent [box-decoration-break:clone]"
                      >
                        {part.url}
                      </span>
                    ) : part.mention ? (
                      // A thread's name, exactly as the field holds it: the pill adds no character.
                      <span
                        key={i}
                        className="rounded-xs bg-fill-strong text-fg [box-decoration-break:clone]"
                      >
                        {part.mention}
                      </span>
                    ) : part.ref ? (
                      <span
                        key={i}
                        className="rounded-xs bg-fill-strong text-fg [box-decoration-break:clone]"
                      >
                        @{part.ref}
                      </span>
                    ) : (
                      part.text
                    ),
                  )}
                  {"\u200b"}
                </div>
              )}
              {/* The field's twin: same font, leading, padding and width, no transition, never
                  seen or reached. The draft is measured on it (lib/fieldHeight.ts). */}
              <textarea
                ref={gauge}
                aria-hidden
                tabIndex={-1}
                readOnly
                rows={1}
                className="pointer-events-none invisible absolute top-0 left-0 block resize-none overflow-hidden py-1.5 text-body leading-normal"
                style={{ height: 0, width: 0 }}
              />
              <textarea
                ref={field}
                value={draft}
                onChange={(e) => {
                  history.current = null;
                  setCaret(e.target.selectionStart);
                  onChange(e.target.value);
                }}
                onSelect={(e) => {
                  setCaret(e.currentTarget.selectionStart);
                  // A line wrapped or a ⇧⏎ pressed with the caret at the end: keep its line clear.
                  followCaret();
                }}
                onKeyDown={onKeyDown}
                onPaste={onPaste}
                onScroll={syncField}
                rows={1}
                placeholder={placeholder}
                aria-label="Message"
                readOnly={offline}
                {...noTextHelpers}
                style={fadeStyle}
                className={cx(
                  "rc-field-fade relative block w-full resize-none bg-transparent py-1.5 text-body leading-normal outline-none transition-[height] duration-150 ease-snappy placeholder:text-fg-faint",
                  tokenized && "text-transparent caret-fg selection:bg-accent/30",
                )}
              />
            </div>
          </div>
          {/* The card's own row: the + that adds to the message at one end, what sends it at the
              other. Where the thread works and what it may do are not here: they are the strip under
              the card (index.css `.rc-composer-footer`), so this row keeps the two things it is
              about — writing the message and sending it — apart and quiet. No divider: the field's
              own air, and the row's `pt-1`, separate them. */}
          <div className="rc-composer-tray flex items-center gap-2 px-3 pt-1 pb-3">
            <PlusMenu
              onAttach={chooseFiles}
              attachLabel={images ? "Attach Files or Images…" : "Attach Files…"}
              onSendLater={() => setSendLater(true)}
              sendLaterDisabled={
                !thread ||
                status !== "ready" ||
                !!queuedEdit ||
                !!editing ||
                (!draft.trim() && !hasExtras(extras))
              }
              sendLaterTip={
                !thread
                  ? "Open a thread first"
                  : status !== "ready"
                    ? "Not connected"
                    : queuedEdit
                      ? "Finish editing the queued message first"
                      : editing
                        ? "Finish editing the sent message first"
                        : "Write a message first"
              }
              cwd={cwd}
            />
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              onChange={(e) => {
                const files = [...(e.target.files ?? [])];
                e.target.value = "";
                void attachmentsFromFiles(files, images, harnessName).then(attach);
              }}
            />
            <span className="flex-1" />
            <div className="flex shrink-0 items-center gap-2">
              <ModelPicker thread={thread} compact={compact} />
              {queuedEdit ? (
                <Tooltip label="Save in the queue (⏎)">
                  <button
                    type="button"
                    aria-label="Save the queued message"
                    onClick={() => {
                      saveQueuedEdit();
                      field.current?.focus();
                    }}
                    className="flex size-9 items-center justify-center rounded-full bg-fg text-bg hover:bg-fg/85 sm:size-8"
                  >
                    <Check size={14} strokeWidth={2.75} />
                  </button>
                </Tooltip>
              ) : (
                running &&
                canSend && (
                  <Tooltip label="Queue for when this turn ends (⏎)">
                    <button
                      type="button"
                      aria-label="Queue message"
                      onClick={() => send(false)}
                      className="flex size-9 items-center justify-center rounded-full bg-fg text-bg hover:bg-fg/85 sm:size-8"
                    >
                      <ListEnd size={13} strokeWidth={2.5} />
                    </button>
                  </Tooltip>
                )
              )}
              {running ? (
                <Tooltip label={cancelReason ?? "Stop turn (Esc)"}>
                  <button
                    type="button"
                    aria-label="Stop turn"
                    disabled={!!cancelReason}
                    onClick={() => {
                      haptic("level");
                      void cancel(thread?.id);
                    }}
                    className="flex size-9 items-center justify-center rounded-full bg-danger text-accent-fg hover:bg-danger/85 disabled:opacity-40 sm:size-8"
                  >
                    <Square size={10} fill="currentColor" strokeWidth={0} />
                  </button>
                </Tooltip>
              ) : queuedEdit ? null : (
                <Tooltip label="Send (⏎)">
                  <button
                    type="button"
                    aria-label="Send"
                    disabled={!canSend || offline || status !== "ready" || (!thread && !projectCwd)}
                    onPointerEnter={(e) => !e.currentTarget.disabled && haptic("alignment")}
                    onClick={() => send(false)}
                    className="flex size-9 items-center justify-center rounded-full bg-fg text-bg transition-[background-color,opacity] duration-150 enabled:hover:bg-fg/85 disabled:bg-fill-strong disabled:text-fg-muted sm:size-8"
                  >
                    <ArrowUp size={14} strokeWidth={2.75} />
                  </button>
                </Tooltip>
              )}
            </div>
          </div>
        </div>
      </div>
      {/* The strip under the card: where this thread works (its folder, its worktree or branch),
          what it may do without asking, and what it has spent. It is the card's footer rather than
          a row in it: the card floats and keeps its own air, and the strip is attached under its
          bottom edge, a step lighter than the pane, inset 12px each side, its top edge tucked
          behind the card (index.css `.rc-composer-footer`). Its padding (`px-2 pt-[14px] pb-1.5`,
          the top 8px of it behind the card) leaves its pills the same 8px on every side. Labels give
          way with the width (the folder's first), so a narrow window keeps every control and drops
          what it says. */}
      <div className="rc-composer-footer relative z-0 mx-3 -mt-2 flex items-center gap-4 px-2 pt-[14px] pb-1.5 text-callout text-fg-muted">
        <div className="flex min-w-0 items-center gap-1">
          {!thread && <RunOnMenu />}
          <FolderMenu thread={thread} cwd={cwd} host={host} />
          {!thread && !newThreadHost && <WorktreeToggle draft={draft} compact={compact} />}
          {thread?.worktree && (
            <Tooltip side="bottom" label={thread.worktree.path}>
              <span className="flex h-7 max-w-[220px] shrink-0 items-center gap-1.5 rounded-full px-2 text-fg-muted @max-xl/composer:hidden">
                <GitBranch size={14} className="shrink-0" />
                <span className="truncate">{thread.worktree.branch}</span>
              </span>
            </Tooltip>
          )}
        </div>
        <span className="flex-1" />
        <div className="flex shrink-0 items-center gap-1.5">
          <PermissionMenu threadId={thread?.id} compact={compact} />
          <span className="flex h-7 shrink-0 items-center empty:hidden">
            <UsageMeter threadId={thread?.id} />
          </span>
        </div>
      </div>
      {thread && (
        <SendLaterDialog
          open={sendLater}
          onOpenChange={setSendLater}
          preview={scheduledSummary(draft, extras)}
          onSchedule={schedule}
        />
      )}
    </div>
  );
}, sameComposer);

/** The thread stopped on its plan's limit and carries on by itself when it resets. */
function LimitWaitRow({ threadId }: { threadId: string }) {
  const wait = useApp((s) => s.limitWaits[threadId]);
  // Once it is continuing, the turn says so; the wait is settled when it ends.
  const running = useApp((s) => s.threads[threadId]?.running);
  if (!wait || running) return null;
  return (
    <div className="flex items-center gap-1.5 px-5 pt-2 text-caption text-fg-muted">
      <Clock size={11} className="shrink-0" />
      <span className="min-w-0 truncate">Continues when the limit resets, at {limitTime(wait.until)}</span>
      <Pill quiet size="sm" className="ml-auto shrink-0" onClick={() => dropLimitWait(threadId)}>
        Don't continue
      </Pill>
    </div>
  );
}

/**
 * How many thumbnails a draft shows before the rest become one count.
 *
 * Past two or three pictures the band is a wall of thumbnails that pushes the field down for
 * nothing: the count says `+3` and opens the whole set, the way a full attachment row collapses in
 * T3 Code (`getRestingComposerImagePreviewCounts`). Files keep their own chips, after them.
 */
const IMAGE_PREVIEW_LIMIT = 3;

function AttachmentStrip({
  attachments,
  onRemove,
}: {
  attachments: Attachment[];
  onRemove: (id: string) => void;
}) {
  const pictures = attachments.flatMap((a) =>
    a.kind === "image" ? [{ src: `data:${a.mimeType};base64,${a.data}`, alt: a.name }] : [],
  );
  const images = attachments.filter((a) => a.kind === "image");
  const shown = images.slice(0, IMAGE_PREVIEW_LIMIT);
  const more = images.slice(IMAGE_PREVIEW_LIMIT);
  return (
    // Its own band above the field, inset from the box's rounder corner and never shrinking, so the
    // thumbnails clear the curve and only the field below scrolls.
    <div className="flex shrink-0 gap-2 overflow-x-auto px-5 pt-5 pb-1">
      {shown.map((a) =>
        a.kind === "image" ? (
          <div key={a.id} className="group relative shrink-0 rc-in">
            <button
              type="button"
              aria-label={`Open ${a.name}`}
              onClick={() =>
                openLightbox(
                  pictures,
                  pictures.findIndex((p) => p.alt === a.name),
                )
              }
              className="block rounded-lg"
            >
              <img
                src={`data:${a.mimeType};base64,${a.data}`}
                alt={a.name}
                title={a.name}
                className="size-16 rounded-lg object-cover shadow-1"
              />
            </button>
            {/* Inside the corner, so the strip's scroll never crops it. */}
            <button
              type="button"
              aria-label={`Remove ${a.name}`}
              onClick={() => onRemove(a.id)}
              className="rc-glass absolute top-1.5 right-1.5 flex size-[20px] items-center justify-center rounded-full text-fg opacity-0 shadow-1 hover:bg-surface-3 group-hover:opacity-100 focus-visible:opacity-100"
            >
              <X size={11} strokeWidth={2.5} />
            </button>
          </div>
        ) : null,
      )}
      {more.length > 0 && (
        // The count is a way in, not a dead label: it opens the whole set at the first one it hides.
        <button
          key="more"
          type="button"
          aria-label={`Show all ${images.length} images`}
          onClick={() => openLightbox(pictures, IMAGE_PREVIEW_LIMIT)}
          className="group relative size-16 shrink-0 rounded-lg bg-fill-strong text-callout text-fg-muted shadow-1 rc-in hover:text-fg"
        >
          +{more.length}
        </button>
      )}
      {attachments.map((a) =>
        // A long paste keeps its size in the chip, and its text in the panel it opens.
        a.kind === "text" ? (
          <PasteChip key={a.id} text={a.text} onRemove={() => onRemove(a.id)} />
        ) : a.kind === "file" ? (
          <Tooltip key={a.id} label={a.path ?? `${a.name} (sent inline)`}>
            <span className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-full bg-fill-subtle pr-1.5 pl-2 text-callout rc-in">
              <FileText size={12} className="text-fg-muted" />
              <span className="max-w-[200px] truncate">{a.name}</span>
              <button
                type="button"
                aria-label={`Remove ${a.name}`}
                onClick={() => onRemove(a.id)}
                className="text-fg-faint hover:text-fg"
              >
                <X size={12} />
              </button>
            </span>
          </Tooltip>
        ) : null,
      )}
    </div>
  );
}

/**
 * "New worktree" for a new thread: the thread runs on its own branch in its
 * own folder, so parallel threads never edit the same checkout. The branch
 * name follows the message until the user edits it.
 */
function WorktreeToggle({ draft, compact }: { draft: string; compact?: boolean }) {
  const wt = useApp((s) => s.newWorktree);
  const missing = useApp((s) => (s.features.worktrees ? null : featureMissing("worktrees")));
  const suggested = suggestBranch(draft || "new thread");
  // biome-ignore lint/correctness/useExhaustiveDependencies: follow the draft until edited
  useEffect(() => {
    if (wt.on && !wt.edited && wt.branch !== suggested) setNewWorktree({ branch: suggested });
  }, [suggested, wt.on, wt.edited]);
  const valid = !wt.branch.trim() || validBranch(wt.branch.trim());
  return (
    // Not `shrink`: the toggle keeps its button's width so it never runs under the mode beside it;
    // the folder name is the one label that gives way.
    <div className="flex min-w-0 shrink-0 items-center gap-1">
      <Tooltip
        side="bottom"
        label={
          missing ?? (wt.on ? "Runs on its own branch and folder" : "Run on a new branch, in its own folder")
        }
      >
        <button
          type="button"
          aria-pressed={wt.on}
          aria-label="Run in a new worktree"
          disabled={!!missing}
          onClick={() => setNewWorktree({ on: !wt.on, branch: suggested, edited: false })}
          className={cx(
            "flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2 text-callout disabled:opacity-40",
            wt.on
              ? "bg-fill-strong text-fg"
              : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg enabled:active:bg-fill-strong",
          )}
        >
          <GitBranch size={14} className="shrink-0" />
          {/* The label drops at the same step as the branch chip beside it, so a narrow card keeps
              the toggle, the folder and the mode without the row running out of room. */}
          {!compact && !wt.on && <span className="@max-xl/composer:hidden">Worktree</span>}
        </button>
      </Tooltip>
      {wt.on && (
        <input
          value={wt.branch}
          onChange={(e) => setNewWorktree({ branch: e.target.value, edited: true })}
          aria-label="Branch name"
          aria-invalid={!valid}
          {...noTextHelpers}
          placeholder={suggested}
          size={Math.min(32, Math.max(12, wt.branch.length + 1))}
          className={cx(
            "h-[24px] min-w-0 max-w-[220px] rounded-full bg-fill px-2 font-mono text-caption rc-in",
            valid ? "focus:bg-fill-strong" : "text-danger",
          )}
        />
      )}
    </div>
  );
}

/**
 * **Run on**: the computer the next new thread runs on (docs/desktop.md › Several machines).
 * This computer first, then each paired machine in the order Settings › Machines lists them,
 * with the state of its connection. A machine that is away cannot start a thread, so its row
 * says "Offline" and stays disabled. Hidden until a second computer is paired: a picker with one
 * choice is noise. A thread that exists already has its machine, so this is the new-thread strip's
 * own control.
 */
function RunOnMenu() {
  const machines = useMachines((s) => s.machines);
  const host = useApp((s) => s.newThreadHost);
  if (!machines.length) return null;
  const current = machines.find((machine) => machine.id === host);
  const label = current?.name ?? selfName();
  const row = (id: string, name: string, state: MachineState, selected: boolean) => {
    const online = state === "connected";
    return (
      <CardMenuItem
        key={id || "local"}
        role="menuitemradio"
        aria-checked={selected}
        disabled={!online}
        onSelect={() => void setNewThreadHost(id)}
        className={cx("gap-2", selected && "bg-fill-subtle", !online && "opacity-60")}
      >
        <span
          aria-hidden
          className={cx(
            "shrink-0 text-[9px] leading-none",
            online ? "text-success" : state === "needsPairing" ? "text-warn" : "text-fg-faint",
          )}
        >
          {online ? "●" : state === "needsPairing" ? "⚠" : "○"}
        </span>
        <span className="min-w-0 flex-1 truncate text-body text-fg">{name}</span>
        {!online && (
          <span className="shrink-0 text-caption text-fg-faint">
            {state === "needsPairing" ? "Revoked" : "Offline"}
          </span>
        )}
        {selected && <Check size={13} className="shrink-0 text-fg-muted" />}
      </CardMenuItem>
    );
  };
  return (
    <CardPopover
      role="menu"
      label="Run on"
      tooltip={`Run on ${label}`}
      onKeyDown={menuKeys}
      side="top"
      className="min-w-[220px]"
      trigger={
        <button
          type="button"
          aria-label={`Run on: ${label}`}
          className={cx(
            footerButton,
            "min-w-0 shrink data-[state=open]:bg-fill disabled:hover:bg-transparent",
          )}
        >
          <Monitor size={14} className="shrink-0" />
          <span className="min-w-0 max-w-[200px] truncate @max-lg/composer:max-w-[96px] @max-sm/composer:hidden">
            {label}
          </span>
          <ChevronDown size={12} className="shrink-0 text-fg-faint" />
        </button>
      }
    >
      <div role="menu" aria-label="Run on">
        {row(LOCAL_HOST, selfName(), "connected", !host)}
        {machines.map((machine) => row(machine.id, machine.name, machine.state, machine.id === host))}
      </div>
    </CardPopover>
  );
}

/**
 * The folder a thread works in: for a new thread a menu of projects to start
 * in; once the thread exists, what to do with its folder (Finder, Terminal,
 * copy the path, another thread there).
 *
 * The list is projects, not every folder a thread ever ran in: the root, a home folder and a
 * folder named like a container (`~/code`, `~/dev`) that holds another project are not offered, and
 * a worktree nests under the repo it was cut from (lib/folderList.ts). Projects are ranked by how
 * much they are used — how recently, and how many threads — so the ones worked in sit at the top.
 * One line per row — the name, then the end of its path, faded — so a deep checkout never turns the
 * list into a wall of wrapping text. "Choose Folder…" is the last row, and the last thing the
 * arrow keys reach.
 */
function FolderMenu({ thread, cwd, host }: { thread?: ThreadState; cwd: string; host: string }) {
  const sessions = useApp((s) => s.sessions);
  const machineName = useApp((s) => s.machineNames[host]);
  const threadMachine = useMachineOfThread(thread?.id);
  const [filter, setFilter] = useState("");
  // A new thread on another computer starts in that computer's folders: its threads' projects,
  // never this computer's paths, which are meaningless there.
  const sources = useMemo(
    () =>
      sessions
        .filter((s) => (s.host ?? LOCAL_HOST) === host)
        .map((s) => ({ cwd: s.cwd, worktree: s.worktree, updatedAt: s.updatedAt })),
    [sessions, host],
  );
  const all = useMemo(() => folderRows(sources), [sources]);
  const rows = useMemo(() => folderRows(sources, { filter }), [sources, filter]);
  // The search appears once there is a list worth searching, and stays while it holds something.
  const searchable = all.length > 4 || !!filter.trim();
  return (
    <CardPopover
      label={cwd ? `Project folder: ${projectName(cwd)}` : "Choose a folder"}
      onKeyDown={menuKeys}
      tooltip={cwd || "Choose a folder"}
      // Always above the button, pinned: the list is what it is about, and the card above it has
      // the room (docs/design.md › Composer strip).
      side="top"
      className="flex w-[300px] flex-col gap-1"
      onOpenChange={(open) => {
        // A search is about the panel it was typed in, not a filter the next one inherits.
        if (!open) setFilter("");
      }}
      trigger={
        <button
          type="button"
          // The name is the trigger's own text at full width and its icon's label when it drops
          // out under a container width, so the button keeps a name either way.
          aria-label={cwd ? `Project folder: ${projectName(cwd)}` : "Choose a folder"}
          disabled={!!thread && !cwd}
          className={cx(
            footerButton,
            "min-w-0 shrink data-[state=open]:bg-fill disabled:hover:bg-transparent",
          )}
        >
          <Folder size={14} className="shrink-0" />
          <span className="min-w-0 max-w-[220px] truncate @max-lg/composer:max-w-[96px] @max-sm/composer:hidden">
            {cwd ? projectName(cwd) : "Choose a folder"}
          </span>
          <ChevronDown size={12} className="shrink-0 text-fg-faint" />
        </button>
      }
    >
      {thread ? (
        <FolderActions cwd={cwd} host={host} remote={!!threadMachine} />
      ) : (
        <>
          {/* The folders scroll under a search that filters them; "Choose Folder…" stays pinned
              under them, last in the list and last to the arrow keys. */}
          {searchable && (
            <div data-menu-static className="px-0.5 pb-0.5">
              <MenuSearchField
                value={filter}
                onChange={setFilter}
                placeholder="Search folders"
                label="Search folders"
              />
            </div>
          )}
          <div className="scroll-stable min-h-0 flex-1">
            {rows.map((row) => (
              <CardMenuItem
                key={row.path}
                onSelect={() => setProjectCwd(row.path)}
                className={cx("gap-2", row.path === cwd && "bg-fill", row.repo && "pl-6")}
              >
                {/* A worktree is the repo's, not a project of its own: indented under it, marked by
                    the branch it is. */}
                {row.repo && <GitBranch size={12} className="shrink-0 text-fg-faint" />}
                <span className="min-w-[40%] flex-1 truncate text-body text-fg">
                  {matchSpans(row.name, filter).map((span, i) =>
                    span.hit ? (
                      <mark key={i} className="rounded-xs bg-fill-strong text-fg">
                        {span.text}
                      </mark>
                    ) : (
                      span.text
                    ),
                  )}
                </span>
                <span className="min-w-0 shrink truncate text-caption text-fg-faint">
                  {middleEllipsis(row.path, PATH_CHARS)}
                </span>
              </CardMenuItem>
            ))}
            {!rows.length && all.length > 0 && (
              <p className="px-3 py-2 text-callout text-fg-muted">No folder matches</p>
            )}
            {!all.length && host && (
              <p className="px-3 py-2 text-callout text-fg-muted">
                No folders on {machineName ?? "that computer"} yet
              </p>
            )}
          </div>
          {/* A folder of another computer is not this app's to browse: its projects are what its
              threads name, and nothing here can read its disk. */}
          {all.length > 0 && !host && <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />}
          {!host && (
            <CardMenuItem onSelect={() => void pickFolder(cwd).then((p) => p && setProjectCwd(p))}>
              <FolderPlus size={13} className="text-fg-muted" /> Choose Folder…
            </CardMenuItem>
          )}
        </>
      )}
    </CardPopover>
  );
}

/**
 * What the folder of an existing thread offers. A thread on another computer has its folder there:
 * this one can copy the path, but neither Finder nor a terminal on this Mac can open it.
 */
function FolderActions({ cwd, host, remote }: { cwd: string; host: string; remote?: boolean }) {
  return (
    <>
      <div className="truncate px-3 pt-1 pb-1.5 text-caption text-fg-muted">
        {middleEllipsis(cwd, PATH_CHARS + 10)}
      </div>
      {!remote && (
        <CardMenuItem onSelect={() => openPath(cwd)}>
          <FolderOpen size={13} className="text-fg-muted" /> Open in {fileManagerName()}
        </CardMenuItem>
      )}
      {!remote && canOpenTerminal() && (
        <CardMenuItem onSelect={() => void openTerminalAt(cwd).catch((e) => toast(String(e), "error"))}>
          <SquareTerminal size={13} className="text-fg-muted" /> Open in Terminal
        </CardMenuItem>
      )}
      <CardMenuItem
        onSelect={() => void navigator.clipboard.writeText(cwd).then(() => toast("Path copied", "info"))}
      >
        <Copy size={13} className="text-fg-muted" /> Copy Path
      </CardMenuItem>
      <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />
      <CardMenuItem onSelect={() => startNewThread(cwd, host || undefined)}>
        <SquarePen size={13} className="text-fg-muted" /> New Thread Here
      </CardMenuItem>
    </>
  );
}

// A button in the composer's footer: quiet until hovered, and its fill is its open state. It is
// not `shrink-0`: a long folder name gives way to the controls to its right (see FolderMenu).
// Its height is the footer's one height at every width, so the air around it stays even.
const footerButton =
  "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong";

const PERMISSION_ICON: Record<PermissionMode, typeof Shield> = {
  ask: Shield,
  edits: ShieldCheck,
  full: ShieldOff,
};

/** What the thread may do without asking. Full access is tinted so it is never on unnoticed. */
export function PermissionMenu({ threadId, compact }: { threadId?: string; compact?: boolean }) {
  const mode = useApp((s) => permissionModeOf(s, threadId));
  const current = PERMISSION_MODES.find((m) => m.id === mode)!;
  const Icon = PERMISSION_ICON[mode];
  return (
    <CardPopover
      role="menu"
      label="Permissions"
      tooltip={current.detail}
      onKeyDown={menuKeys}
      className="w-[280px]"
      trigger={
        <button
          type="button"
          aria-label={`Permissions: ${current.label}`}
          className={cx(
            footerButton,
            "shrink-0 data-[state=open]:bg-fill",
            mode === "full" && "text-warning hover:text-warning",
          )}
        >
          <Icon size={14} />
          {!compact && <span className="@max-md/composer:hidden">{current.label}</span>}
          <ChevronDown size={12} className="text-fg-faint" />
        </button>
      }
    >
      <div role="menu" aria-label="Permissions">
        {PERMISSION_MODES.map((m) => {
          const ItemIcon = PERMISSION_ICON[m.id];
          const checked = m.id === mode;
          return (
            <CardMenuItem
              key={m.id}
              role="menuitemradio"
              aria-checked={checked}
              onSelect={() => setPermissionMode(threadId, m.id)}
              className={cx("items-start gap-2 py-1.5", checked && "bg-fill-subtle")}
            >
              <ItemIcon
                size={13}
                className={cx("mt-[3px] shrink-0", m.id === "full" ? "text-warning" : "text-fg-muted")}
              />
              <span className="flex min-w-0 flex-col">
                <span className="text-body text-fg">{m.label}</span>
                <span className="text-caption text-fg-muted">{m.detail}</span>
              </span>
              {checked && <Check size={13} className="ms-auto mt-[3px] shrink-0 text-fg-muted" />}
            </CardMenuItem>
          );
        })}
      </div>
    </CardPopover>
  );
}

/** The + menu at the start of the composer's row, after the SwiftUI app's AddPanel. */
function PlusMenu({
  onAttach,
  attachLabel,
  onSendLater,
  sendLaterDisabled,
  sendLaterTip,
  cwd,
}: {
  onAttach: () => void;
  attachLabel: string;
  onSendLater: () => void;
  /** Nothing to schedule yet, or the app cannot send now: the row says so instead of opening. */
  sendLaterDisabled: boolean;
  /** Why the row is disabled, said on the row itself. */
  sendLaterTip: string;
  cwd: string;
}) {
  return (
    <CardPopover
      role="menu"
      label="Attach and more"
      tooltip="Attach and more"
      onKeyDown={menuKeys}
      className="min-w-[220px]"
      trigger={
        // The one button the message is added to: the send circle's own diameter with a 16px mark, so
        // the row's two ends read as the row's two ends and sit on one line. `absoluteStrokeWidth`
        // keeps that mark a true 2px instead of the 1.33px a 16px lucide would otherwise draw, and at
        // this size the arms land on whole pixels: the cross stays a cross at 2x instead of reading
        // as two soft, uneven strokes.
        <button
          type="button"
          aria-label="Attach and more"
          className="flex size-9 shrink-0 items-center justify-center rounded-full bg-fill-subtle text-fg hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill-strong sm:size-8"
        >
          <Plus size={16} strokeWidth={2} absoluteStrokeWidth />
        </button>
      }
    >
      <CardMenuItem onSelect={onAttach}>
        <Paperclip size={13} className="text-fg-muted" /> {attachLabel}
      </CardMenuItem>
      <CardMenuItem
        onSelect={onSendLater}
        disabled={sendLaterDisabled}
        data-disabled={sendLaterDisabled || undefined}
        title={sendLaterDisabled ? sendLaterTip : undefined}
      >
        <Clock size={13} className="text-fg-muted" /> Send later…
      </CardMenuItem>
      <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />
      <CardMenuItem onSelect={() => startNewThread()}>
        <SquarePen size={13} className="text-fg-muted" /> New Thread
        <MenuShortcut>{formatBinding("mod+n")}</MenuShortcut>
      </CardMenuItem>
      <CardMenuItem
        onSelect={() =>
          void pickFolder(cwd).then((p) => {
            if (!p) return;
            setProjectCwd(p);
            startNewThread(p);
          })
        }
      >
        <Folder size={13} className="text-fg-muted" /> New Thread in Folder…
      </CardMenuItem>
    </CardPopover>
  );
}
