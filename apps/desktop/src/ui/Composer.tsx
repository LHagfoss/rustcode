// The composer: a floating rounded box with the field, the queue, slash
// command suggestions, the model pill and send/queue/stop. Keys follow the
// SwiftUI app: ⏎ send (or queue while running), ⇧⏎ newline, ⌘⏎ steer,
// Esc closes suggestions, then clears the draft, then stops the turn.

import { ArrowUp, ChevronDown, Folder, ListEnd, Pencil, Square, X } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { projectName, type SlashCommand } from "../acp/types";
import { pickFolder } from "../lib/native";
import { controlDisabledReason } from "../state/harness";
import {
  cancel,
  canSteer,
  removeQueued,
  sendQueuedNow,
  setDraft,
  setProjectCwd,
  steerOrSubmit,
  submit,
  useApp,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { ModelPicker } from "./ModelPicker";
import { cx, Tooltip } from "./primitives";

function suggestions(draft: string, commands: SlashCommand[]): SlashCommand[] {
  if (!draft.startsWith("/") || /\s/.test(draft)) return [];
  const typed = draft.toLowerCase();
  const prefixed = commands.filter((c) => c.name.toLowerCase().startsWith(typed));
  const others =
    typed.length > 1
      ? commands.filter((c) => !prefixed.includes(c) && c.name.toLowerCase().includes(typed.slice(1)))
      : [];
  return [...prefixed, ...others].slice(0, 40);
}

const LINE = 18;

export function Composer({ thread, compact }: { thread?: ThreadState; compact?: boolean }) {
  const key = thread?.id ?? "new";
  const draft = useApp((s) => s.drafts[key] ?? "");
  const commands = useApp((s) => s.commands);
  const projectCwd = useApp((s) => s.projectCwd);
  const harnesses = useApp((s) => s.harnesses);
  const status = useApp((s) => s.status);
  const field = useRef<HTMLTextAreaElement>(null);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);

  const running = thread?.running ?? false;
  const steerable = thread ? canSteer(thread) : false;
  const list = useMemo(
    () => (dismissed === draft ? [] : suggestions(draft, commands)),
    [draft, commands, dismissed],
  );
  const harnessName = harnesses?.harnesses.find((h) => h.id === thread?.harness?.id)?.name;
  const cancelReason = controlDisabledReason("cancel", thread?.harness?.capabilities, harnessName);

  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus on thread change
  useEffect(() => {
    field.current?.focus();
  }, [key]);

  // Grow from 2 to 10 lines, then scroll.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measure on every draft change
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(Math.max(el.scrollHeight, LINE * 2), LINE * 10)}px`;
  }, [draft]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new suggestion list starts at the top
  useEffect(() => setSelected(0), [list.length]);

  const update = (text: string) => setDraft(key, text);

  const send = (steer: boolean) => {
    const text = draft;
    if (!text.trim() && !(steer && running)) return;
    update("");
    void (steer ? steerOrSubmit(text) : submit(text));
  };

  const complete = (command: SlashCommand, run: boolean) => {
    if (run && !command.takesArguments) {
      update("");
      void submit(command.name);
    } else {
      update(`${command.name} `);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (list.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.metaKey)) {
        e.preventDefault();
        const command = list[selected];
        if (command) complete(command, e.key === "Enter");
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(draft);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      send(e.metaKey || e.ctrlKey);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      if (draft) update("");
      else if (running) void cancel(thread?.id);
      return;
    }
    if (e.key === "ArrowUp" && !draft && thread) {
      const last = [...thread.items].reverse().find((i) => i.kind === "user");
      if (last && last.kind === "user") {
        e.preventDefault();
        update(last.text);
      }
    }
  };

  const placeholder = running
    ? steerable
      ? "⏎ queues for the next turn, ⌘⏎ steers this one…"
      : "Queue a message…"
    : "Message RustCode…";

  return (
    <div className="relative">
      {list.length > 0 && (
        <div
          className="surface absolute right-0 bottom-full left-0 mb-2 max-h-[280px] overflow-y-auto rounded-[20px] border border-hairline p-1.5 rc-in"
          role="listbox"
          aria-label="Commands"
        >
          {list.map((c, i) => (
            <button
              key={c.name}
              type="button"
              role="option"
              aria-selected={i === selected}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => complete(c, true)}
              onMouseEnter={() => setSelected(i)}
              className={cx(
                "flex w-full items-baseline gap-3 rounded-[14px] px-3 py-[7px] text-left",
                i === selected && "bg-brand/18",
              )}
            >
              <span className="min-w-[110px] font-semibold text-brand text-callout">{c.name}</span>
              <span className="truncate text-callout text-fg-2">{c.description}</span>
            </button>
          ))}
        </div>
      )}
      <div
        className="surface relative z-10 flex flex-col gap-2 rounded-[22px] p-2.5"
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            field.current?.focus();
          }
        }}
      >
        {thread && thread.queue.length > 0 && (
          <div className="flex flex-col gap-1.5 px-1">
            <div className="flex items-center gap-1.5 text-caption text-brand">
              <ListEnd size={12} />
              <b>Queued</b>
              <span className="text-fg-2">
                · sent when this turn ends{steerable ? ", or now with ⌘⏎" : ""}
              </span>
            </div>
            {thread.queue.map((q, i) => (
              <div key={q.id} className="flex items-center gap-2 rounded-xl bg-fill-2 px-2 py-1.5 rc-in">
                <span className="font-mono text-caption text-fg-3">{i + 1}</span>
                <span className="line-clamp-2 flex-1 text-callout">{q.text}</span>
                {steerable && (
                  <Tooltip label="Send now: the turn reads it after its current step">
                    <button
                      type="button"
                      aria-label="Send now"
                      onClick={() => void sendQueuedNow(thread.id, q.id)}
                      className="text-fg-2 hover:text-fg"
                    >
                      <ArrowUp size={13} />
                    </button>
                  </Tooltip>
                )}
                <Tooltip label="Edit">
                  <button
                    type="button"
                    aria-label="Edit queued message"
                    onClick={() => {
                      removeQueued(thread.id, q.id);
                      update(q.text);
                      field.current?.focus();
                    }}
                    className="text-fg-2 hover:text-fg"
                  >
                    <Pencil size={12} />
                  </button>
                </Tooltip>
                <Tooltip label="Remove">
                  <button
                    type="button"
                    aria-label="Remove queued message"
                    onClick={() => removeQueued(thread.id, q.id)}
                    className="text-fg-3 hover:text-fg"
                  >
                    <X size={13} />
                  </button>
                </Tooltip>
              </div>
            ))}
          </div>
        )}
        <textarea
          ref={field}
          value={draft}
          onChange={(e) => {
            update(e.target.value);
            setDismissed(null);
          }}
          onKeyDown={onKeyDown}
          rows={2}
          placeholder={placeholder}
          aria-label="Message"
          spellCheck
          className="w-full resize-none bg-transparent px-1.5 pt-1 text-body leading-[18px] outline-none placeholder:text-fg-3"
        />
        <div className="flex items-center gap-2">
          <span className="flex-1" />
          <ModelPicker thread={thread} compact={compact} />
          {running && draft.trim() && (
            <Tooltip label="Queue for when this turn ends (⏎)">
              <button
                type="button"
                aria-label="Queue message"
                onClick={() => send(false)}
                className="flex size-[30px] items-center justify-center rounded-full bg-brand-fill text-white"
              >
                <ListEnd size={13} strokeWidth={2.5} />
              </button>
            </Tooltip>
          )}
          {running ? (
            <Tooltip label={cancelReason ?? "Stop turn (Esc)"}>
              <button
                type="button"
                aria-label="Stop turn"
                disabled={!!cancelReason}
                onClick={() => void cancel(thread?.id)}
                className="flex size-[30px] items-center justify-center rounded-full bg-gradient-to-b from-[#ff5f57] to-[#e0352b] text-white disabled:opacity-40"
              >
                <Square size={10} fill="currentColor" strokeWidth={0} />
              </button>
            </Tooltip>
          ) : (
            <Tooltip label="Send (⏎)">
              <button
                type="button"
                aria-label="Send"
                disabled={!draft.trim() || status !== "ready" || (!thread && !projectCwd)}
                onClick={() => send(false)}
                className="flex size-[30px] items-center justify-center rounded-full bg-brand-fill text-white transition-colors disabled:bg-fg-2/25"
              >
                <ArrowUp size={14} strokeWidth={2.75} />
              </button>
            </Tooltip>
          )}
        </div>
      </div>
      <FooterTray thread={thread} cwd={thread?.cwd ?? projectCwd} />
    </div>
  );
}

/** The tray tucked under the composer: the folder, and the harness a thread runs on. */
function FooterTray({ thread, cwd }: { thread?: ThreadState; cwd: string }) {
  const harnesses = useApp((s) => s.harnesses);
  const sessions = useApp((s) => s.sessions);
  const harness = thread?.harness ? harnesses?.harnesses.find((h) => h.id === thread.harness?.id) : undefined;
  const projects = useMemo(
    () => [...new Set(sessions.map((s) => s.cwd).filter(Boolean))].slice(0, 12),
    [sessions],
  );
  const [open, setOpen] = useState(false);
  return (
    <div className="mx-3 -mt-3.5 flex items-center gap-1 rounded-b-[18px] bg-fill-3 px-1.5 pt-[19px] pb-[5px] text-callout text-fg-2">
      <div className="relative">
        <button
          type="button"
          disabled={!!thread}
          onClick={() => setOpen(!open)}
          aria-haspopup="menu"
          aria-expanded={open}
          className="flex h-[26px] items-center gap-1.5 rounded-xl px-2.5 hover:bg-fill-2 disabled:hover:bg-transparent"
        >
          <Folder size={12} />
          <span className="max-w-[260px] truncate">{cwd ? projectName(cwd) : "Choose a folder"}</span>
          {!thread && <ChevronDown size={11} className="text-fg-3" />}
        </button>
        {open && (
          <div
            role="menu"
            className="surface absolute bottom-full left-0 z-30 mb-2 w-[300px] rounded-[20px] border border-hairline p-1.5 rc-in"
            onMouseLeave={() => setOpen(false)}
          >
            {projects.map((p) => (
              <button
                key={p}
                type="button"
                role="menuitem"
                onClick={() => {
                  setProjectCwd(p);
                  setOpen(false);
                }}
                className={cx(
                  "flex w-full flex-col rounded-[14px] px-2.5 py-1.5 text-left hover:bg-fill-2",
                  p === cwd && "bg-brand/15",
                )}
              >
                <span className="text-body text-fg">{projectName(p)}</span>
                <span className="truncate text-caption text-fg-2" dir="rtl">
                  <bdi>{p}</bdi>
                </span>
              </button>
            ))}
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                void pickFolder(cwd).then((p) => p && setProjectCwd(p));
              }}
              className="mt-0.5 flex w-full rounded-[14px] px-2.5 py-2 text-left text-body hover:bg-fill-2"
            >
              Choose Folder…
            </button>
          </div>
        )}
      </div>
      <span className="flex-1" />
      {harness && !harness.builtin && (
        <Tooltip
          label={`${harness.name} runs this thread with its own sign-in, tools and permission rules. RustCode's sandbox does not apply.`}
        >
          <span className="flex h-[26px] items-center gap-1.5 rounded-xl px-2.5 text-fg-2">
            <span className="size-1.5 rounded-full bg-brand" />
            {harness.name}
          </span>
        </Tooltip>
      )}
    </div>
  );
}
