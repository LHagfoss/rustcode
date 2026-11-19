// The composer's foot and the strip under its box: the limit wait, the worktree toggle, the
// machine chip, the folder, access and + menus. Split out of ui/Composer.tsx.

import { formatBinding } from "../lib/keys";

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

import { useEffect } from "react";
import { projectName } from "../acp/types";
import { suggestBranch, validBranch } from "../acp/v2";
import { pickFolder } from "../lib/native";
import { useMachines } from "../state/machines";
import { modeLabel, PERMISSION_MODES, type PermissionMode } from "../state/permissions";
import {
  cancelWait,
  featureMissing,
  HOME_FOLDER,
  hasOwnPermissionMode,
  inheritedModeOf,
  limitTime,
  openSettings,
  permissionModeOf,
  resumeWaitNow,
  setNewWorktree,
  setPermissionMode,
  setProjectCwd,
  startNewThread,
  useApp,
} from "../state/store";
import { isRunning, type ThreadState } from "../state/thread";
import { CardMenuItem, CardPopover, menuKeys } from "./cardPopover";
import { threadMachine } from "./composerFooterLayout";
import {
  Check,
  ChevronDown,
  Clock,
  Folder,
  GitBranch,
  Paperclip,
  Shield,
  ShieldCheck,
  ShieldOff,
  SquarePen,
  WifiOff,
} from "./icons";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { ProjectPicker, useMoveBlockedReason } from "./ProjectPicker";
import { cx, MenuQuickActions, noTextHelpers, Pill, Tooltip } from "./primitives";

/**
 * The thread waits to go on by itself: its plan's limit resetting, a provider that
 * was overloaded retrying, or the machine's network coming back. Send now skips the
 * wait; Cancel stops it continuing on its own.
 */
export function LimitWaitRow({ threadId }: { threadId: string }) {
  const wait = useApp((s) => s.limitWaits[threadId]);
  const offline = useApp((s) => s.internetWaits[threadId]);
  // Once it is continuing, the turn says so; the wait is settled when it ends.
  const running = useApp((s) => isRunning(s.threads[threadId]));
  if ((!wait && !offline) || running) return null;
  const text = offline
    ? "Waiting for the internet, then it continues"
    : wait?.reason === "overload"
      ? `The provider was overloaded — trying again at ${limitTime(wait.until)}`
      : wait
        ? `Waiting for the limit to reset (${limitTime(wait.until)})`
        : "";
  return (
    <div className="flex items-center gap-1.5 px-5 pt-2 text-caption text-fg-muted">
      {offline ? <WifiOff size={12} className="shrink-0" /> : <Clock size={12} className="shrink-0" />}
      <span className="min-w-0 truncate">{text}</span>
      <Pill quiet size="sm" className="ml-auto shrink-0" onClick={() => resumeWaitNow(threadId)}>
        Send now
      </Pill>
      <Pill quiet size="sm" className="shrink-0" onClick={() => cancelWait(threadId)}>
        Cancel
      </Pill>
    </div>
  );
}

/**
 * "New worktree" for a new thread: the thread runs on its own branch in its
 * own folder, so parallel threads never edit the same checkout. The branch
 * name follows the message until the user edits it.
 */
export function WorktreeToggle({ draft, compact }: { draft: string; compact?: boolean }) {
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
 * Where an open thread runs, when that is a paired machine: its glyph and name before the folder,
 * a quiet label rather than a button (the machine is fixed once a thread exists). A thread on this
 * computer shows nothing, as in the sidebar. The name is the first label to give way under a
 * container width, and the glyph keeps the name for assistive tech.
 */
export function ThreadMachineChip({ host }: { host: string }) {
  const machines = useMachines((s) => s.machines);
  const there = threadMachine(host, machines);
  if (!there) return null;
  return (
    <Tooltip side="bottom" label={there.title}>
      <span
        role="img"
        aria-label={there.title}
        className="flex h-7 min-w-0 shrink items-center gap-1.5 whitespace-nowrap rounded-full px-2 text-callout text-fg-muted"
      >
        <span className="relative flex shrink-0">
          <DeviceGlyphIcon glyph={there.glyph} tint={there.tint} size={14} />
          {there.away && (
            <span className="-right-0.5 -top-0.5 absolute size-1.5 rounded-full bg-warning ring-2 ring-[var(--rc-composer-footer-bg)]" />
          )}
        </span>
        <span className="min-w-0 max-w-[160px] truncate @max-lg/composer:max-w-[96px] @max-md/composer:hidden">
          {there.name}
        </span>
      </span>
    </Tooltip>
  );
}

/**
/**
 * The folder button under the composer: the project picker (ui/ProjectPicker.tsx), which starts a
 * new thread in a project or moves the open thread to another one.
 */
export function FolderMenu({ thread, cwd, host }: { thread?: ThreadState; cwd: string; host: string }) {
  const blocked = useMoveBlockedReason(thread);
  return (
    <ProjectPicker
      thread={thread}
      cwd={cwd}
      host={host}
      // Always above the button, pinned: the list is what it is about, and the card above it has
      // the room (docs/design.md › Composer strip).
      side="top"
      tooltip={blocked ? `${cwd} · ${blocked}` : cwd || "Choose a folder"}
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
            {cwd === HOME_FOLDER ? "Home folder" : cwd ? projectName(cwd) : "Choose a folder"}
          </span>
          <ChevronDown size={12} className="shrink-0 text-fg-faint" />
        </button>
      }
    />
  );
}

// A button in the composer's footer: quiet until hovered, and its fill is its open state. It is
// not `shrink-0`: a long folder name gives way to the controls to its right (see FolderMenu).
// Its height is the footer's one height at every width, so the air around it stays even.
export const footerButton =
  "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong";

const PERMISSION_ICON: Record<PermissionMode, typeof Shield> = {
  ask: Shield,
  edits: ShieldCheck,
  full: ShieldOff,
};

/**
 * What the thread may do without asking. Full access is tinted so it is never on unnoticed. A thread
 * follows its project's default (Settings › Permissions) until a mode is picked here; the menu says
 * which, and "Use default" goes back to following it.
 */
export function PermissionMenu({
  threadId,
  cwd,
  compact,
}: {
  threadId?: string;
  /** A new thread's folder, whose project may have its own default. */
  cwd?: string;
  compact?: boolean;
}) {
  const mode = useApp((s) => permissionModeOf(s, threadId, cwd));
  const own = useApp((s) =>
    threadId ? hasOwnPermissionMode(s, threadId) : s.newThreadPermissionMode !== null,
  );
  const inherited = useApp((s) => inheritedModeOf(s, threadId, cwd));
  const current = PERMISSION_MODES.find((m) => m.id === mode)!;
  const Icon = PERMISSION_ICON[mode];
  const source = own ? "Set for this thread" : "Default";
  return (
    <CardPopover
      role="menu"
      label="Permissions"
      tooltip={`${current.label} · ${current.detail}${own ? "" : " (the default)"}`}
      onKeyDown={menuKeys}
      className="w-[280px]"
      trigger={
        <button
          type="button"
          aria-label={`Permissions: ${current.label}${own ? "" : ", the default"}`}
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
        <div className="px-2 pt-1 pb-1.5 text-caption text-fg-muted">{source}</div>
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
        <div role="separator" className="mx-1.5 my-1 h-px shrink-0 bg-border" />
        {own && (
          <CardMenuItem role="menuitem" onSelect={() => setPermissionMode(threadId, null)}>
            <span className="text-body text-fg">Use default</span>
            <span className="ms-auto text-caption text-fg-muted">{modeLabel(inherited)}</span>
          </CardMenuItem>
        )}
        <CardMenuItem role="menuitem" onSelect={() => openSettings("permissions")}>
          <span className="text-body text-fg">Permission settings…</span>
        </CardMenuItem>
      </div>
    </CardPopover>
  );
}

/**
 * The + as one filled shape on a 16px grid: 2px arms, each 10px long, crossing at (8, 8). Every
 * edge is a whole pixel, at 1x and (as whole device pixels) at 2x, and a single path means the
 * crossing is painted once.
 */
function PlusMark() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="currentColor"
      shapeRendering="crispEdges"
      aria-hidden="true"
      className="shrink-0"
    >
      <path d="M7 3h2v4h4v2H9v4H7V9H3V7h4z" />
    </svg>
  );
}

/** The + menu at the start of the composer's row, after the SwiftUI app's AddPanel. */
export function PlusMenu({
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
      className="w-[280px]"
      trigger={
        // The one button the message is added to: the send circle's own diameter with a 16px mark, so
        // the row's two ends read as the row's two ends and sit on one line. The mark is one filled
        // outline on a whole-pixel grid (`PlusMark`), not two stroked lines: lucide's Plus draws two
        // 24-unit strokes scaled to 16px, so their ends land on thirds of a pixel and the crossing
        // is painted twice, which reads as "-|-" instead of one plus.
        <button
          type="button"
          aria-label="Attach and more"
          className="flex size-9 shrink-0 items-center justify-center rounded-full bg-fill-subtle text-fg hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill-strong sm:size-8"
        >
          <PlusMark />
        </button>
      }
    >
      <MenuQuickActions
        Item={CardMenuItem}
        actions={[
          { label: "Attach", description: attachLabel, icon: <Paperclip />, onSelect: onAttach },
          {
            label: "Later",
            description: sendLaterDisabled ? `Send later: ${sendLaterTip}` : "Send later…",
            icon: <Clock />,
            onSelect: onSendLater,
            disabled: sendLaterDisabled,
          },
          {
            label: "New",
            description: `New Thread · ${formatBinding("mod+n")}`,
            icon: <SquarePen />,
            onSelect: () => startNewThread(),
          },
          {
            label: "Folder",
            description: "New Thread in Folder…",
            icon: <Folder />,
            onSelect: () =>
              void pickFolder(cwd).then((p) => {
                if (!p) return;
                setProjectCwd(p);
                startNewThread(p);
              }),
          },
        ]}
      />
    </CardPopover>
  );
}
