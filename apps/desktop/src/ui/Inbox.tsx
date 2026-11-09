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
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { isAllow, type PermissionOption, type PermissionRequest, projectName } from "../acp/types";
import { assessApproval, grantAllowed, optionScope } from "../lib/approval";
import {
  type DecidedItem,
  foldDecided,
  groupInbox,
  type InboxGroup,
  type InboxItem,
  showsNeedsYou,
} from "../state/inbox";
import { useInboxItems } from "../state/inboxRead";
import {
  answerPermission,
  answerQuestionOptions,
  dismissInbox,
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
  const scope = optionScope(option);
  if (scope === "once") return "Allow";
  if (scope === "hour") return "1 hour";
  if (scope === "always") return "Always";
  if (scope === "decline") return "Decline";
  return option.name;
}

function decidedLabel(option: PermissionOption): string {
  const scope = optionScope(option);
  if (scope === "hour") return "Allowed for 1 hour";
  if (scope === "once" || scope === "always") return "Approved";
  if (scope === "decline") return "Declined";
  return "Answered";
}

/**
 * The actions one row offers: the same answers the thread's own cards offer, then
 * Open and Dismiss, which every row has. Dismiss puts the row away without
 * answering it, and the row comes back if the thread asks again (state/store.ts
 * `dismissInbox`).
 */
export function inboxActions(item: InboxItem): RowAction[] {
  return [...rowAnswers(item), openAction(item), dismissAction(item)];
}

function openAction(item: InboxItem): RowAction {
  return { label: "Open", run: () => selectThread(item.threadId), decided: "Opened" };
}

/** Put this row away without answering it: the thread is untouched, so it goes on as it was. */
export function dismissAction(item: InboxItem): RowAction {
  return { label: "Dismiss", run: () => dismissInbox([item]), decided: "Dismissed" };
}

/** The answers a row offers, before Open and Dismiss. */
function rowAnswers(item: InboxItem): RowAction[] {
  if (item.kind === "failed") {
    return [
      {
        label: "Retry",
        run: () => void retryThread(item.threadId),
        primary: true,
        decided: "Retried",
      },
    ];
  }
  const request: PermissionRequest | undefined = item.request;
  if (!request) return [];
  if (request.question) {
    // A multiple-choice question needs the thread's own form: one inline answer could
    // send half of it.
    if (request.question.multiple) return [];
    const answers = request.options.filter((option) => isAllow(option.kind) || !option.kind);
    return answers.slice(0, 3).map(
      (option, i): RowAction => ({
        label: option.name,
        run: () => answerQuestionOptions(request, [option]),
        primary: i === 0,
        decided: "Answered",
      }),
    );
  }
  // The answer the row leads with is a single call; a grant is a choice of how
  // long it lasts, the same one the thread's own card offers. A destructive
  // call gets neither: it is answered once, or declined.
  const options = orderedOptions(request.options);
  const scopeOf = (scope: ReturnType<typeof optionScope>) =>
    options.find((option) => optionScope(option) === scope);
  const grants = grantAllowed(assessApproval(request, item.project));
  const scopes: ("once" | "hour" | "always")[] = grants ? ["once", "hour", "always"] : ["once"];
  const actions: RowAction[] = scopes.flatMap((scope) => {
    const option = scopeOf(scope);
    return option
      ? [
          {
            label: optionLabel(option),
            run: () => answerPermission(request, option),
            primary: scope === "once",
            decided: decidedLabel(option),
          },
        ]
      : [];
  });
  const decline = scopeOf("decline");
  if (decline) {
    actions.push({
      label: optionLabel(decline),
      run: () => answerPermission(request, decline),
      decided: decidedLabel(decline),
    });
  }
  return actions;
}

function KindIcon({ kind }: { kind: InboxItem["kind"] }) {
  if (kind === "failed") return <CircleAlert size={14} className="text-danger" />;
  if (kind === "delegate") return <CornerDownRight size={14} className="text-fg-muted" />;
  if (kind === "question") return <MessageCircleQuestion size={14} className="text-accent" />;
  return <Hand size={14} className="text-warning" />;
}

/** The answers, with their digit hints: shown on the row the keyboard is on, or hovered. */
function RowActions({ item, onRun }: { item: InboxItem; onRun: (action: RowAction) => void }) {
  return (
    <>
      {inboxActions(item).map((action, i) => (
        <Pill
          key={`${action.label}-${i}`}
          size="sm"
          prominent={action.primary}
          onClick={() => onRun(action)}
          className="max-w-full overflow-hidden"
        >
          <span className="min-w-0 truncate">{action.label}</span>
          {i < 9 && <Kbd>{i + 1}</Kbd>}
        </Pill>
      ))}
    </>
  );
}

/** One thread's requests: one header naming it, then a row for each thing it wants. */
function InboxGroupHeader({ group, onOpen }: { group: InboxGroup; onOpen: () => void }) {
  return (
    <div className="flex items-center gap-2 px-2 pt-2 pb-1">
      <ProjectMark name={projectName(group.project)} size={16} />
      <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-baseline gap-1.5 text-left">
        <span className="min-w-0 truncate font-medium text-body hover:text-fg">{group.title}</span>
        {group.items.length > 1 && (
          <span className="shrink-0 text-caption text-fg-faint">· {group.items.length} requests</span>
        )}
      </button>
    </div>
  );
}

/**
 * One request under its thread's header: what it wants, its age, and one compact
 * answer at rest. The full answers with their digit keys appear on the row the
 * keyboard is on, or under the pointer: the list stays scannable at rest.
 */
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
  const actions = inboxActions(item);
  const compact = actions.find((action) => action.primary) ?? actions[0];
  return (
    <li
      data-inbox-key={item.key}
      aria-current={active || undefined}
      className={cx(
        "group flex items-start gap-2 rounded-lg px-2 py-1.5",
        active ? "bg-fill" : "hover:bg-fill-subtle",
      )}
    >
      <span className="mt-1 shrink-0">
        <KindIcon kind={item.kind} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <button
            type="button"
            onClick={onOpen}
            title={item.detail}
            className={cx(
              "min-w-0 flex-1 text-left text-callout text-fg-muted hover:text-fg",
              // The selected row reads its question whole: the answers drop to their own line.
              active ? "break-words whitespace-normal" : "truncate",
            )}
          >
            {item.detail}
          </button>
          {item.pending > 1 && (
            <span className="shrink-0 text-caption text-fg-faint tabular">
              {item.number} of {item.pending}
            </span>
          )}
          <span className="shrink-0 text-caption text-fg-faint tabular">{relativeAge(item.at, now)}</span>
          {compact && (
            <span
              className={cx("shrink-0", active ? "hidden" : "group-hover:hidden group-focus-within:hidden")}
            >
              <Pill size="sm" prominent={compact.primary} onClick={() => onRun(compact)}>
                {compact.label}
              </Pill>
            </span>
          )}
        </div>
        <span
          className={cx(
            "min-w-0 flex-wrap items-center gap-1.5",
            active ? "flex" : "hidden group-hover:flex group-focus-within:flex",
          )}
        >
          <RowActions item={item} onRun={onRun} />
        </span>
      </div>
      {/* The row's own x: the quick way out for a pointer, beside the answers the
          hovered line offers and the digit key that does the same. */}
      <button
        type="button"
        onClick={() => onRun(dismissAction(item))}
        title="Dismiss"
        aria-label={`Dismiss ${item.title}`}
        className="mt-0.5 flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 transition-opacity hover:bg-fill hover:text-fg focus-visible:opacity-100 group-focus-within:opacity-100 group-hover:opacity-100"
      >
        <X size={13} />
      </button>
    </li>
  );
}

/** A row that was handled: faded, still clickable, and what the answer was. */
function DecidedRow({ item }: { item: DecidedItem }) {
  return (
    <li className="flex items-center gap-2 rounded-lg px-2 py-1.5">
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
 * the list. Its badge rolls when the number changes (`rc-count-roll`). Drawn only
 * while something waits on the user (`showsNeedsYou`), so an empty row never
 * stands over the threads as if it headed them.
 *
 * It counts inbox items, not threads — an approval, a question, a failed turn, a
 * delegate — and opens the page that lists them all, so it keeps the name; the
 * list's own "Asking you" section (`ui/Sidebar.tsx` `LABELS`) is the threads that
 * have one of those open, and its heading says so. The 6px under it matches the
 * brand header's, so the row closes the top block instead of heading the list.
 */
export function NeedsYouRow() {
  const items = useInboxItems();
  const here = useApp((s) => s.view.kind === "inbox");
  if (!showsNeedsYou(items)) return null;
  const count = items.length;
  return (
    <div className="shrink-0 px-2 pb-1.5">
      <button
        type="button"
        onClick={() => setView({ kind: "inbox" })}
        aria-current={here ? "page" : undefined}
        title="Approvals, questions, failed turns and delegates across every thread"
        className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left text-body hover:bg-fill"
      >
        <Hand size={14} className="shrink-0 text-warning" fill="currentColor" fillOpacity={0.2} />
        <span className="min-w-0 flex-1 truncate font-medium text-fg">Needs you</span>
        <span
          key={count}
          className="rc-count-roll shrink-0 rounded-full bg-warning/14 px-1.5 font-medium text-caption text-warning tabular"
        >
          {count > 99 ? "99+" : count}
        </span>
      </button>
    </div>
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
  const groups = useMemo(() => groupInbox(items), [items]);
  // Where each group's first row sits in the flat list the keyboard moves through.
  const starts = useMemo(() => {
    let at = 0;
    return groups.map((group) => {
      const start = at;
      at += group.items.length;
      return start;
    });
  }, [groups]);
  const [active, setActive] = useState(0);
  const [decided, setDecided] = useState<DecidedItem[]>([]);
  const previous = useRef<InboxItem[]>([]);
  const labels = useRef<Record<string, string>>({});
  const list = useRef<HTMLUListElement>(null);

  // What the inbox had and no longer has was answered, retried or opened. The
  // previous list is read before it is replaced: the state updater runs later,
  // once this effect is done, and would otherwise find the list already moved on.
  useEffect(() => {
    const before = previous.current;
    previous.current = items;
    setDecided((kept) => foldDecided(before, items, kept, { labels: labels.current }));
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

  // Dismiss all: every row as it stands, with one Undo, and each folded into Decided
  // as one the user put away rather than answered.
  const dismissAll = useCallback(() => {
    for (const item of items) labels.current[item.key] = "Dismissed";
    dismissInbox(items);
  }, [items]);

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
      >
        {items.length > 0 && (
          <Pill size="sm" quiet onClick={dismissAll}>
            Dismiss all
          </Pill>
        )}
      </ViewHeader>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-10 sm:px-12">
        <div className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-4 pt-1">
          <WhileAwayCard />
          {recapSince !== null && recapDismissed && (
            <button
              type="button"
              onClick={showRecap}
              className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
            >
              <Sparkles size={13} className="shrink-0 text-accent" />
              <span className="min-w-0 flex-1">Show the while-you-were-away recap</span>
              <ArrowUpRight size={12} className="shrink-0 text-fg-faint" />
            </button>
          )}

          {items.length > 0 ? (
            <section aria-label="Waiting on you" className="flex flex-col">
              <span className="px-2 pb-1 font-medium text-caption text-fg-faint">Waiting on you</span>
              <ul ref={list} className="flex flex-col">
                {groups.map((group, g) => (
                  <li key={`${group.threadId}:${group.items[0]!.key}`} className="flex flex-col">
                    <InboxGroupHeader group={group} onOpen={() => selectThread(group.threadId)} />
                    <ul className="flex flex-col">
                      {group.items.map((item, i) => (
                        <InboxRow
                          key={item.key}
                          item={item}
                          active={starts[g]! + i === active}
                          now={now}
                          onOpen={() => selectThread(item.threadId)}
                          onRun={(action) => run(item, action)}
                        />
                      ))}
                    </ul>
                  </li>
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
              <div className="flex items-center gap-2 px-2 pb-1">
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
