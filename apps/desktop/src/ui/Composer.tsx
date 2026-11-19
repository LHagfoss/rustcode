import { formatBinding } from "../lib/keys";
import { primaryModifier } from "../lib/platform";

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

import { memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { projectName, type SlashCommand } from "../acp/types";
import { fieldBeat } from "../lib/backdrop/events";
import { insertTranscript } from "../lib/dictation";
import { FIELD_FADE, type FieldFade, fieldFade } from "../lib/fade";
import { measuredFieldHeight } from "../lib/fieldHeight";
import { haptic } from "../lib/haptics";
import { clipboardImage, type ImageFile, onFileDrop, pickFiles, projectFiles } from "../lib/native";
import {
  dismissNextPrompt,
  nextPromptDismissed,
  nextPromptEnabled,
  nextPromptFor,
  suggestionKeyAction,
} from "../lib/nextPrompt";
import { isLongPaste, pastedImages } from "../lib/paste";
import { observeSize } from "../lib/resize";
import { fadeClearedField } from "../lib/sendFlight";
import {
  type Attachment,
  type ComposeExtras,
  commandSubmission,
  commandSuggestions,
  emptyExtras,
  hasExtras,
  hashToken,
  hasTokens,
  insertCommand,
  insertMention,
  type Mention,
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
import { atToken, insertKeyword, insertRef, type MentionScope, rankFiles } from "../state/fileRefs";
import { controlDisabledReason } from "../state/harness";
import { useMachineReconnecting } from "../state/machines";
import { type MentionNode, type MentionSection, mentionNodes, rankMachines } from "../state/mentionDrawer";
import { noteRecentFile, recentFiles, recentFirst } from "../state/recentFiles";
import { scheduledSummary, scheduleRefusalText } from "../state/schedule";
import {
  acceptsImages,
  answerPermission,
  cancel,
  cancelEdit,
  cancelQueuedEdit,
  commandsFor,
  endQueuedEdit,
  harnessesOn,
  newThreadChoice,
  saveQueuedEdit,
  scheduleMessage,
  sendDecisionFor,
  setDraft,
  setExtras,
  startQueuedEdit,
  steerOrSubmit,
  submit,
  threadHarness,
  toast,
  useApp,
  useMachineOfThread,
  useThreadOffline,
} from "../state/store";
import { isRunning, localId, type ThreadState } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { AttachmentStrip, attachmentsFromFiles, attachmentsFromPaths } from "./Attachments";
import { ComposerDrawer } from "./ComposerDrawer";
import {
  FolderMenu,
  LimitWaitRow,
  PermissionMenu,
  PlusMenu,
  ThreadMachineChip,
  WorktreeToggle,
} from "./ComposerFooter";
import { UsageMeter } from "./ComposerUsage";
import {
  ALL_MATCHES,
  agentChoices,
  type DrawerEntry,
  type DrawerPick,
  type DrawerRowItem,
  FILES_FRESH_MS,
  keywordEntries,
  mentionStateOf,
  NO_PLANS,
  NOT_EXPANDED,
  projectKeyOf,
} from "./composerMentions";
import { DictationButton, MicrophoneNotice, useDictation } from "./Dictation";
import { overFolderDrop, overQuestionDrop } from "./folderDropTarget";
import { ArrowUp, Check, GitBranch, ListEnd, Pencil, Square } from "./icons";
import { LinkChip } from "./LinkChip";
import { MachineSelector } from "./MachineSelector";
import { ModelPicker } from "./ModelPicker";
import { useMachineMentions } from "./machineMentions";
import { cx, noTextHelpers, Pill, Tooltip } from "./primitives";
import { QueuedAnnouncer } from "./QueuedMessages";
import { ScheduledMessages } from "./ScheduledMessages";
import { SendLaterDialog } from "./SendLaterDialog";

export { PermissionMenu } from "./ComposerFooter";

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
  const status = useApp((s) => s.engine.phase);
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

  const running = isRunning(thread);
  // A thread on a paired computer that is not connected now is read-only: its turns run there,
  // and the header and a note over the field say which computer it is (state/store.ts `threadOffline`).
  const offline = useThreadOffline(thread?.id);
  const offMachine = useMachineOfThread(thread?.id);
  // Away, but on the shell's own retry clock: the field says it is coming back, not that it is gone.
  const offReconnecting = useMachineReconnecting(thread ? splitThreadKey(thread.id).host : undefined);
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
    useShallow((s) => Object.values(s.threads).flatMap((t) => (isRunning(t) ? [t.id] : []))),
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
  const mentioning = !!hash || !!at;
  // `@`'s machines: where the agent should run the work, so it need not look for one.
  const machineMentions = useMachineMentions(host);
  // The sections the reader opened whole with a `Show all` row: a few rows of each is what the
  // drawer offers first, and this is the rest of the ones they asked for.
  const [expanded, setExpanded] = useState<ReadonlySet<MentionSection>>(NOT_EXPANDED);
  // The harnesses' names, for the sub-heading an agent's harness gets (`@` groups by harness).
  const harnessNames = useMemo(
    () => Object.fromEntries((harnesses ?? []).map((h) => [h.id, h.name])),
    [harnesses],
  );
  // What the `@` drawer lists: its keywords, then the agents under their own harness, the threads
  // and the files (state/mentionDrawer.ts). Every item is searched; a few of each are drawn first.
  const mention = useMemo((): MentionNode[] => {
    if (!mentioning) return [];
    // The project's files, ranked; while nothing is typed the ones this folder pointed at last
    // come first, since a folder keeps no record of its own of which were touched.
    const paths = files?.cwd === cwdForFiles ? files.paths : [];
    const ranked = rankFiles(paths, mentionQuery, ALL_MATCHES);
    return mentionNodes({
      query: mentionQuery,
      scope,
      keywords: scope ? [] : keywordEntries(mentionQuery, machineMentions.length > 0),
      // Ranked over everything, so a match past the few rows on screen still answers what is typed.
      agents: rankAgents(mentionQuery, agents, ALL_MATCHES),
      machines: rankMachines(machineMentions, mentionQuery),
      delegates: rankThreads(delegates, mentionQuery, ALL_MATCHES),
      threads: rankThreads(threadChoices, mentionQuery, ALL_MATCHES),
      files: mentionQuery.trim() ? ranked : recentFirst(ranked, recentFiles(cwdForFiles)),
      expanded,
      harnessNames,
    });
  }, [
    mentioning,
    mentionQuery,
    scope,
    agents,
    machineMentions,
    delegates,
    threadChoices,
    files,
    cwdForFiles,
    expanded,
    harnessNames,
  ]);
  // The drawer's rows, as drawn: a section's heading, a harness under it, then the bound rows, in
  // one list so the arrow keys walk the rows across the sections and never land on a heading.
  const drawerRows = useMemo((): DrawerRowItem[] => {
    if (slash) {
      return list.map((command, index) => ({ kind: "row", entry: { kind: "command", command }, index }));
    }
    const rows: DrawerRowItem[] = [];
    let index = 0;
    for (const node of mention) {
      if (node.kind === "entry") rows.push({ kind: "row", entry: node.entry, index: index++ });
      else if (node.kind === "more")
        rows.push({ kind: "more", section: node.section, hidden: node.hidden, index: index++ });
      else if (node.kind === "less") rows.push({ kind: "less", section: node.section, index: index++ });
      else rows.push(node);
    }
    return rows;
  }, [slash, list, mention]);
  // What the arrows walk: a row that points at something, or a section's own open/close row.
  const picks = useMemo(
    (): DrawerPick[] =>
      drawerRows.flatMap((row): DrawerPick[] => {
        if (row.kind === "row") return [{ kind: "entry", entry: row.entry }];
        if (row.kind === "more") return [{ kind: "more", section: row.section }];
        if (row.kind === "less") return [{ kind: "less", section: row.section }];
        return [];
      }),
    [drawerRows],
  );
  // What is left to offer at all: an empty drawer says so where the field is about to be typed.
  const anyRows = picks.length > 0;
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

  // What a section's `Show N more` row does: the rest of that section, in place, from here on.
  const expand = (section: MentionSection) => {
    setExpanded((held) => new Set(held).add(section));
  };

  // And what `Show fewer` does: back to the few rows the section opened with.
  const collapse = (section: MentionSection) => {
    setExpanded((held) => {
      const next = new Set(held);
      next.delete(section);
      return next;
    });
  };

  // A new list starts at the top, and what was opened whole closes again: another word is another
  // look, and a section left expanded would bury the rows below it before they were seen.
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new suggestion list starts at the top
  useEffect(() => {
    setSelected(0);
    setExpanded(NOT_EXPANDED);
  }, [drawer, drawerQuery]);

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
  // A transcript lands at the caret (replacing a selection) and never sends on its own.
  const dictation = useDictation(
    (text) => {
      const el = field.current;
      const current = useApp.getState().drafts[key] ?? "";
      const { value, caret } = insertTranscript(
        current,
        text,
        el?.selectionStart ?? current.length,
        el?.selectionEnd ?? current.length,
      );
      update(value);
      requestAnimationFrame(() => {
        const after = field.current;
        if (!after) return;
        after.focus();
        after.selectionStart = after.selectionEnd = caret;
      });
    },
    !offline,
    host ? null : home || null,
  );
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
      // A drag over the sidebar is a folder becoming a project there, and one over a question's
      // form goes with its answer: neither is the composer's attachment.
      const elsewhere = overFolderDrop(event.position) || overQuestionDrop(event.position);
      setDropping(event.type === "over" && !elsewhere);
      if (event.type === "drop" && event.paths.length && !elsewhere) dropped.current(event.paths);
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
  // A suggested next prompt (lib/nextPrompt.ts): ghost text in an empty field after a completed
  // turn, when Settings › Chat turns it on. Typing hides it, Tab puts it in the field to edit,
  // Escape dismisses it for that turn. It is never sent on its own.
  const [, setDismissedNext] = useState(0);
  // Read on each render, so turning the setting on shows it as soon as the chat is back.
  const nextPrompt = nextPromptEnabled() ? nextPromptFor(thread) : null;
  const suggestion =
    nextPrompt &&
    !nextPromptDismissed(nextPrompt.key) &&
    !draft &&
    !hasExtras(extras) &&
    !queuedEdit &&
    !editing &&
    !offline
      ? nextPrompt
      : null;
  const suggestionHint = useId();
  const acceptSuggestion = () => {
    if (!suggestion) return;
    history.current = null;
    update(suggestion.text);
    setCaret(suggestion.text.length);
    field.current?.focus();
  };
  const dismissSuggestion = () => {
    if (!suggestion) return;
    dismissNextPrompt(suggestion.key);
    setDismissedNext((n) => n + 1);
  };
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
    // The live background answers with a wave from the composer (lib/backdrop/events.ts).
    fieldBeat("send", fieldBox.current);
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
    const scheduled = scheduleMessage(thread.id, draft, extras, due);
    if ("refused" in scheduled) {
      // The field keeps the message and its attachments: nothing is scheduled without them.
      toast(
        scheduled.refused === "offline"
          ? "Not connected: the message was not scheduled."
          : scheduleRefusalText(scheduled.refused),
        "error",
      );
      return;
    }
    setSendLater(false);
    update("");
    change(() => emptyExtras);
    fadeClearedField(fieldBox.current);
  };

  const reference = (path: string) => {
    if (!at) return;
    // The drawer offers what this folder's picks pointed at last, first, next time it opens.
    noteRecentFile(cwdForFiles, path);
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
        const message = commandSubmission(command);
        void submit(message.text, message.extras);
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
      case "machine":
        addMention({ kind: "machine", id: entry.machine.id, name: entry.machine.name });
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
    // A completion menu keeps Tab and Escape; an open popover keeps its Escape.
    const onSuggestion = suggestion ? suggestionKeyAction(e, picks.length > 0 || !!drawer) : null;
    if (onSuggestion === "accept") {
      e.preventDefault();
      acceptSuggestion();
      return;
    }
    if (onSuggestion === "dismiss" && !overlayOpen()) {
      e.preventDefault();
      dismissSuggestion();
      return;
    }
    if (picks.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : picks.length - 1)) % picks.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !primaryModifier(e))) {
        e.preventDefault();
        const pick = picks[selected] ?? picks[0];
        if (!pick) return;
        if (pick.kind === "more") expand(pick.section);
        else if (pick.kind === "less") collapse(pick.section);
        else accept(pick.entry, e.key === "Enter");
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
    ? offReconnecting
      ? `Reconnecting${offMachine ? ` to ${offMachine}` : ""}… Messages can be sent once it is back…`
      : `Not connected${offMachine ? ` to ${offMachine}` : ""}. Messages can be sent once it is back…`
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

  return (
    <div
      ref={shell}
      role="region"
      aria-label="Message input"
      data-focus-area="composer"
      className={cx("rc-composer-shell relative @container/composer", className)}
    >
      {thread && <QueuedAnnouncer threadId={thread.id} />}
      {/* What waits for a time the user picked, over the box: it leaves the transcript's own lane
          alone, and the stack's bottom edge carries the box down as the cards come and go. */}
      {thread && <ScheduledMessages threadId={thread.id} />}
      {/* The box: the field and how it is sent. The drawer tucks under its top, the tray under its
          foot, so the three read as one floating surface over the transcript. */}
      <div className="relative">
        {drawer && (
          <ComposerDrawer
            drawerRef={drawerRef}
            drawerLabel={drawerLabel}
            anyRows={anyRows}
            emptyText={emptyText}
            drawerRows={drawerRows}
            drawerQuery={drawerQuery}
            selected={selected}
            setSelected={setSelected}
            expand={expand}
            collapse={collapse}
            complete={complete}
            reference={reference}
            accept={accept}
          />
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
              <Pencil size={12} />
              <b>Editing a queued message</b>
              <span className="text-fg-muted">· ⏎ saves it in the queue, esc leaves it</span>
              <Pill quiet size="sm" className="ml-auto" onClick={() => cancelQueuedEdit()}>
                Cancel
              </Pill>
            </div>
          )}
          {editing && (
            <div className="flex items-center gap-1.5 px-5 pt-2 text-caption text-accent">
              <Pencil size={12} />
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
                placeholder={suggestion ? `${suggestion.text}  ⇥` : placeholder}
                aria-label="Message"
                data-focus-default
                aria-describedby={suggestion ? suggestionHint : undefined}
                readOnly={offline}
                {...noTextHelpers}
                style={fadeStyle}
                className={cx(
                  "rc-field-fade relative block w-full resize-none bg-transparent py-1.5 text-body leading-normal outline-none transition-[height] duration-150 ease-snappy placeholder:text-fg-faint",
                  tokenized && "text-transparent caret-fg selection:bg-accent/30",
                )}
              />
              {suggestion && (
                <span id={suggestionHint} className="sr-only">
                  Suggestion: {suggestion.text} Press Tab to insert, Escape to dismiss.
                </span>
              )}
            </div>
          </div>
          {/* The card's own row: the + that adds to the message at one end, what sends it at the
              other. Where the thread works and what it may do are not here: they are the strip under
              the card (index.css `.rc-composer-footer`), so this row keeps the two things it is
              about — writing the message and sending it — apart and quiet. No divider: the field's
              own air, and the row's `pt-1`, separate them. */}
          {!offline && <MicrophoneNotice dictation={dictation} />}
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
              {!offline && <DictationButton dictation={dictation} />}
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
          {!thread && <MachineSelector />}
          {thread && <ThreadMachineChip host={host} />}
          <FolderMenu thread={thread} cwd={cwd} host={host} />
          {!thread && <WorktreeToggle draft={draft} compact={compact} />}
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
          <PermissionMenu threadId={thread?.id} cwd={thread ? undefined : cwd} compact={compact} />
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
