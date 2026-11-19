// The drawer over the composer's box: the commands `/` is looking for, the threads `#` is, or the
// agents and files `@` is. Split out of ui/Composer.tsx, which owns what the drawer lists and what a
// picked row does.

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

import { useEffect, useRef } from "react";
import type { SlashCommand } from "../acp/types";
import type { MentionState } from "../state/compose";
import { SCOPES } from "../state/fileRefs";
import type { MentionSection } from "../state/mentionDrawer";
import { shortAge } from "../state/thread";
import type { DrawerEntry, DrawerRowItem } from "./composerMentions";
import { DRAWER_NAME, Marked } from "./drawerMarks";
import { HarnessIcon } from "./HarnessIcon";
import { ChevronDown, ChevronUp, Command, FileText } from "./icons";
import { MachineMentionRow } from "./machineMentions";
import { cx } from "./primitives";

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
const DRAWER_ROW = "flex w-full items-center gap-2 rounded-lg px-2 py-1 text-left";

export function ComposerDrawer({
  drawerRef,
  drawerLabel,
  anyRows,
  emptyText,
  drawerRows,
  drawerQuery,
  selected,
  setSelected,
  expand,
  collapse,
  complete,
  reference,
  accept,
}: {
  drawerRef: React.RefObject<HTMLDivElement | null>;
  drawerLabel: string;
  anyRows: boolean;
  emptyText: string;
  drawerRows: DrawerRowItem[];
  drawerQuery: string;
  selected: number;
  setSelected: (i: number) => void;
  expand: (section: MentionSection) => void;
  collapse: (section: MentionSection) => void;
  complete: (command: SlashCommand, run: boolean) => void;
  reference: (path: string) => void;
  accept: (entry: DrawerEntry, run: boolean) => void;
}) {
  // The scroll follows the arrow keys: the list runs past the drawer's height, the row it is on
  // stays in view.
  const selectedRow = useRef<HTMLButtonElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the row under the keys is the one that moves
  useEffect(() => {
    selectedRow.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  // What every bound row is: one option of the list, lit while the arrows or the pointer are on it.
  const drawerRowProps = (i: number) => {
    const picked = i === selected;
    return {
      type: "button" as const,
      role: "option",
      "aria-selected": picked,
      ref: picked ? selectedRow : undefined,
      onMouseDown: (e: React.MouseEvent) => e.preventDefault(),
      onMouseEnter: () => setSelected(i),
      className: cx(DRAWER_ROW, picked && "bg-fill"),
    };
  };

  // The rest of a section, in place — or, with the section open, the way back to its few rows.
  const restRow = (section: MentionSection, hidden: number, i: number) => (
    <button key={`m:${section}`} {...drawerRowProps(i)} onClick={() => expand(section)}>
      <ChevronDown size={16} className="shrink-0 text-fg-muted" />
      <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">
        {hidden > 0 ? `Show ${hidden} more` : "Show more"}
      </span>
    </button>
  );

  const fewerRow = (section: MentionSection, i: number) => (
    <button key={`l:${section}`} {...drawerRowProps(i)} onClick={() => collapse(section)}>
      <ChevronUp size={16} className="shrink-0 text-fg-muted" />
      <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">Show fewer</span>
    </button>
  );

  const drawerRow = (entry: DrawerEntry, i: number) => {
    const row = drawerRowProps(i);
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
            {i === selected && (
              <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 font-mono text-caption text-fg-muted">
                @{keyword.word}
              </span>
            )}
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
            {/* One muted detail: the effort it would start on. The harness is the group's header, the
                login and plan are said there once. */}
            <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">{entry.agent.effort}</span>
            {/* The token the row writes, on the row under the keys only, so what the message will
                say is no surprise without every row carrying it. */}
            {i === selected && (
              <span className="ml-auto max-w-[220px] shrink-0 truncate rounded-xs bg-fill px-1.5 font-mono text-caption text-fg-muted">
                @{entry.agent.token}
              </span>
            )}
          </button>
        );
      case "machine":
        return (
          <MachineMentionRow
            key={`mc:${entry.machine.id}`}
            machine={entry.machine}
            query={drawerQuery}
            row={row}
            onPick={() => accept(entry, false)}
          />
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
    <div
      ref={drawerRef}
      className="rc-composer-drawer rc-in absolute inset-x-6 bottom-[calc(100%-1rem)] z-0 flex max-h-64 flex-col overflow-y-auto px-1.5 pt-1.5 pb-[calc(1rem+6px)]"
      role="listbox"
      aria-label={drawerLabel}
    >
      {/* Nothing at all to offer: the drawer says so where the field is about to be typed in. */}
      {!anyRows && <p className="px-5 pt-4 pb-7 text-fg-muted text-sub">{emptyText}</p>}
      {drawerRows.map((item) => {
        if (item.kind === "heading") {
          // A section's name: the rows under it are the drawer's options, never this line.
          return (
            <div
              key={`g:${item.label}`}
              role="presentation"
              className="px-2 pt-1.5 pb-0.5 font-medium text-caption text-fg-muted"
            >
              {item.label}
            </div>
          );
        }
        if (item.kind === "harness") {
          // Which harness the rows under it run on, marked as the app marks that harness.
          return (
            <div
              key={`h:${item.harness}`}
              role="presentation"
              className="flex items-center gap-1.5 px-2 pt-1 pb-0.5 text-caption text-fg-muted"
            >
              <HarnessIcon id={item.harness} name={item.name} size={12} />
              <span className="truncate">{item.name}</span>
              {/* The login and what its plan has left, once for the group's rows. */}
              {(item.account || item.plan) && (
                <span className="ml-auto flex shrink-0 items-center gap-2 text-fg-faint">
                  {item.account && <span className="max-w-[140px] truncate">{item.account}</span>}
                  {item.plan && <span className="tabular">{item.plan}</span>}
                </span>
              )}
            </div>
          );
        }
        if (item.kind === "more") return restRow(item.section, item.hidden, item.index);
        if (item.kind === "less") return fewerRow(item.section, item.index);
        return drawerRow(item.entry, item.index);
      })}
    </div>
  );
}
