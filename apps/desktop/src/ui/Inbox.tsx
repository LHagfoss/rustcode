// The needs-you inbox: every thread's approvals, questions, failed turns and
// delegates in one list, newest first, each answerable where it stands. It is a
// view of the shell (`view.kind === "inbox"`), reached from the sidebar row, the
// command palette and its shortcut (lib/keys.ts). Answered rows fold into the
// faded "Decided" section instead of vanishing, so a click has a visible result.
//
// The derivation is pure (state/inbox.ts); this file draws it and owns the keys:
// ↑/↓ move the row, ⏎ opens its thread, 1–9 run its answers.

import {
  ArrowUpRight,
  CircleAlert,
  CircleCheck,
  CornerDownRight,
  Hand,
  MessageCircleQuestion,
  Sparkles,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { isAllow, type PermissionOption, type PermissionRequest, projectName } from "../acp/types";
import { type DecidedItem, foldDecided, type InboxItem } from "../state/inbox";
import { useInboxItems } from "../state/inboxRead";
import {
  answerPermission,
  answerQuestionOptions,
  retryThread,
  selectThread,
  setView,
  showRecap,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { orderedOptions } from "./AttentionCards";
import { ViewHeader } from "./Board";
import { ProjectMark } from "./ProjectMark";
import { cx, Kbd, Pill } from "./primitives";
import { WhileAwayCard } from "./WhileAway";

function useNow(step = 30_000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), step);
    return () => clearInterval(t);
  }, [step]);
  return now;
}

/** One thing a row offers, in the order the digit keys pick it. */
interface RowAction {
  label: string;
  run: () => void;
  /** The answer the row leads with. */
  primary?: boolean;
  /** What the decided row says it did. */
  decided: string;
}

function optionLabel(option: PermissionOption): string {
  if (option.kind === "allow_once") return "Allow";
  if (option.kind === "allow_always") return "Always allow";
  if (option.kind === "reject_once" || option.kind === "reject_always") return "Decline";
  return option.name;
}

function decidedLabel(option: PermissionOption): string {
  if (option.kind === "allow_once" || option.kind === "allow_always") return "Approved";
  if (option.kind === "reject_once" || option.kind === "reject_always") return "Declined";
  return "Answered";
}

/** The actions one row offers, the same answers the thread's own cards offer. */
export function inboxActions(item: InboxItem): RowAction[] {
  const open: RowAction = {
    label: "Open",
    run: () => selectThread(item.threadId),
    decided: "Opened",
  };
  if (item.kind === "failed") {
    return [
      {
        label: "Retry",
        run: () => void retryThread(item.threadId),
        primary: true,
        decided: "Retried",
      },
      open,
    ];
  }
  const request: PermissionRequest | undefined = item.request;
  if (!request) return [open];
  if (request.question) {
    const answers = request.options.filter((option) => isAllow(option.kind) || !option.kind);
    const actions = answers.slice(0, 3).map(
      (option, i): RowAction => ({
        label: option.name,
        run: () => answerQuestionOptions(request, [option]),
        primary: i === 0,
        decided: "Answered",
      }),
    );
    return [...actions, open];
  }
  const actions = orderedOptions(request.options)
    .slice(0, 3)
    .map(
      (option): RowAction => ({
        label: optionLabel(option),
        run: () => answerPermission(request, option),
        primary: option.kind === "allow_once",
        decided: decidedLabel(option),
      }),
    );
  return [...actions, open];
}

function KindIcon({ kind }: { kind: InboxItem["kind"] }) {
  if (kind === "failed") return <CircleAlert size={14} className="text-danger" />;
  if (kind === "delegate") return <CornerDownRight size={14} className="text-fg-muted" />;
  if (kind === "question") return <MessageCircleQuestion size={14} className="text-accent" />;
  return <Hand size={14} className="text-warning" />;
}

function RowActions({ item, onRun }: { item: InboxItem; onRun: (action: RowAction) => void }) {
  return (
    <>
      {inboxActions(item).map((action, i) => (
        <Pill key={`${action.label}-${i}`} size="sm" prominent={action.primary} onClick={() => onRun(action)}>
          {action.label}
          {i < 9 && <Kbd>{i + 1}</Kbd>}
        </Pill>
      ))}
    </>
  );
}

/** One thread waiting on the user: what it is, what it wants, its age and answers. */
function InboxRow({
  item,
  active,
  now,
  onOpen,
  onRun,
}: {
  item: InboxItem;
  active: boolean;
  now: number;
  onOpen: () => void;
  onRun: (action: RowAction) => void;
}) {
  return (
    <li
      data-inbox-key={item.key}
      aria-current={active || undefined}
      className={cx(
        "flex items-start gap-2.5 rounded-lg px-2.5 py-2",
        active ? "bg-fill" : "hover:bg-fill-subtle",
      )}
    >
      <span className="mt-0.5">
        <ProjectMark name={projectName(item.project)} size={18} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex items-baseline gap-2">
          <button
            type="button"
            onClick={onOpen}
            className="min-w-0 flex-1 truncate text-left font-medium text-body hover:text-fg"
          >
            {item.title}
          </button>
          <span className="shrink-0 text-caption text-fg-faint tabular">{relativeAge(item.at, now)}</span>
        </div>
        <div className="flex min-w-0 items-center gap-1.5 text-callout text-fg-muted">
          <KindIcon kind={item.kind} />
          <span className="min-w-0 truncate" title={item.detail}>
            {item.detail}
          </span>
          {item.pending > 1 && (
            <span className="shrink-0 text-caption text-fg-faint tabular">
              {item.number} of {item.pending}
            </span>
          )}
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-1.5">
          <RowActions item={item} onRun={onRun} />
        </div>
      </div>
    </li>
  );
}

/** A row that was handled: faded, still clickable, and what the answer was. */
function DecidedRow({ item }: { item: DecidedItem }) {
  return (
    <li className="flex items-center gap-2 rounded-lg px-2.5 py-1.5">
      <ProjectMark name={projectName(item.project)} size={14} />
      <button
        type="button"
        onClick={() => selectThread(item.threadId)}
        className="min-w-0 flex-1 truncate text-left text-callout text-fg-muted hover:text-fg"
      >
        <span className="text-fg">{item.title}</span> · {item.detail}
      </button>
      <span className="shrink-0 text-caption text-fg-faint">{item.label}</span>
    </li>
  );
}

/**
 * The sidebar's destination row: "Needs you", with the count, near the top of
 * the list. Its badge rolls when the number changes (`rc-count-roll`).
 */
export function NeedsYouRow() {
  const items = useInboxItems();
  const here = useApp((s) => s.view.kind === "inbox");
  const count = items.length;
  return (
    <button
      type="button"
      onClick={() => setView({ kind: "inbox" })}
      aria-current={here ? "page" : undefined}
      title="Approvals, questions, failed turns and delegates across every thread"
      className="flex h-8 w-full items-center gap-2 rounded-lg px-2.5 text-left text-body hover:bg-fill"
    >
      <Hand
        size={14}
        className={cx("shrink-0", count ? "text-warning" : "text-fg-muted")}
        fill="currentColor"
        fillOpacity={count ? 0.2 : 0}
      />
      <span className={cx("min-w-0 flex-1 truncate", count ? "font-medium text-fg" : "text-fg-muted")}>
        Needs you
      </span>
      {count > 0 && (
        <span
          key={count}
          className="rc-count-roll shrink-0 rounded-full bg-warning/14 px-1.5 font-medium text-caption text-warning tabular"
        >
          {count > 99 ? "99+" : count}
        </span>
      )}
    </button>
  );
}

/**
 * The inbox itself. It keeps one row lit for the keyboard, folds answered rows
 * into Decided, and holds the away recap at its top while one is due.
 */
export function Inbox() {
  const items = useInboxItems();
  const recapSince = useApp((s) => s.recapSince);
  const recapDismissed = useApp((s) => s.recapDismissed);
  const now = useNow();
  const [active, setActive] = useState(0);
  const [decided, setDecided] = useState<DecidedItem[]>([]);
  const previous = useRef<InboxItem[]>([]);
  const labels = useRef<Record<string, string>>({});
  const list = useRef<HTMLUListElement>(null);

  // What the inbox had and no longer has was answered, retried or opened.
  useEffect(() => {
    setDecided((kept) => foldDecided(previous.current, items, kept, { labels: labels.current }));
    previous.current = items;
  }, [items]);

  useEffect(() => {
    setActive((at) => Math.min(at, Math.max(0, items.length - 1)));
  }, [items.length]);

  useEffect(() => {
    const item = items[active];
    if (!item) return;
    list.current
      ?.querySelector<HTMLElement>(`[data-inbox-key="${CSS.escape(item.key)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active, items]);

  const run = useCallback((item: InboxItem, action: RowAction) => {
    labels.current[item.key] = action.decided;
    action.run();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      const s = useApp.getState();
      if (s.settings || s.switcherOpen || s.onboardingOpen || s.confirm) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable]")) return;
      if (document.querySelector('[role="menu"], [role="dialog"], [role="listbox"]')) return;
      const item = items[active];
      const n = Number.parseInt(e.key, 10);
      if (n >= 1 && n <= 9) {
        const action = item && inboxActions(item)[n - 1];
        if (!action) return;
        e.preventDefault();
        run(item, action);
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        if (!items.length) return;
        e.preventDefault();
        setActive((at) => {
          const next = at + (e.key === "ArrowDown" ? 1 : -1);
          return Math.min(items.length - 1, Math.max(0, next));
        });
        return;
      }
      if (e.key === "Enter" && item) {
        // A focused control keeps its own Enter: it is the control the user chose.
        if (target?.closest("button, a, [role=button]")) return;
        e.preventDefault();
        selectThread(item.threadId);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [items, active, run]);

  return (
    <main className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg" aria-label="Needs you">
      <ViewHeader
        title={
          <span className="flex items-center gap-2">
            Needs you
            {items.length > 0 && (
              <span
                key={items.length}
                className="rc-count-roll rounded-full bg-warning/14 px-1.5 font-medium text-caption text-warning tabular"
              >
                {items.length > 99 ? "99+" : items.length}
              </span>
            )}
          </span>
        }
      />
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-10 sm:px-12">
        <div className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-4 pt-1">
          <WhileAwayCard />
          {recapSince !== null && recapDismissed && (
            <button
              type="button"
              onClick={showRecap}
              className="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
            >
              <Sparkles size={13} className="shrink-0 text-accent" />
              <span className="min-w-0 flex-1">Show the while-you-were-away recap</span>
              <ArrowUpRight size={12} className="shrink-0 text-fg-faint" />
            </button>
          )}

          {items.length > 0 ? (
            <section aria-label="Waiting on you" className="flex flex-col">
              <span className="px-2.5 pb-1 font-medium text-caption text-fg-faint">Waiting on you</span>
              <ul ref={list} className="flex flex-col">
                {items.map((item, i) => (
                  <InboxRow
                    key={item.key}
                    item={item}
                    active={i === active}
                    now={now}
                    onOpen={() => selectThread(item.threadId)}
                    onRun={(action) => run(item, action)}
                  />
                ))}
              </ul>
            </section>
          ) : (
            <div className="flex flex-col items-center gap-1.5 py-16 text-center">
              <CircleCheck size={20} className="text-fg-faint" />
              <p className="font-medium text-body text-fg-muted">Nothing needs you</p>
              <p className="text-caption text-fg-faint">
                Approvals, questions, failed turns and delegates land here.
              </p>
            </div>
          )}

          {decided.length > 0 && (
            <section aria-label="Decided" className="flex flex-col opacity-50">
              <div className="flex items-center gap-2 px-2.5 pb-1">
                <span className="font-medium text-caption text-fg-faint">Decided</span>
                <button
                  type="button"
                  onClick={() => setDecided([])}
                  className="ml-auto rounded-xs px-1 text-caption text-fg-faint hover:text-fg"
                >
                  Clear
                </button>
              </div>
              <ul className="flex flex-col">
                {decided.map((item) => (
                  <DecidedRow key={`${item.key}:${item.decidedAt}`} item={item} />
                ))}
              </ul>
            </section>
          )}
        </div>
      </div>
    </main>
  );
}
