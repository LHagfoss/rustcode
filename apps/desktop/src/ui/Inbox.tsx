// The needs-you inbox: every thread's approvals, questions, failed turns and
// delegates in one list, newest first, each answerable where it stands. It is a
// view of the shell (`view.kind === "inbox"`), reached from the sidebar row, the
// command palette and its shortcut (lib/keys.ts). Answered rows fold into the
// faded "Decided" section instead of vanishing, so a click has a visible result.
//
// The derivation is pure (state/inbox.ts); this file draws it and owns the keys:
// ↑/↓ move the row, ⏎ opens its thread, 1–9 run its answers.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { isAllow, type PermissionOption, type PermissionRequest, projectName } from "../acp/types";
import { assessApproval, grantAllowed, optionScope } from "../lib/approval";
import { useClock } from "../lib/useNow";
import {
  type DecidedItem,
  foldDecided,
  groupInbox,
  type InboxGroup,
  type InboxItem,
  readOnlyApprovals,
} from "../state/inbox";
import { useInboxItems } from "../state/inboxRead";
import { filteredNeedsCount } from "../state/sidebarGroups";
import {
  answerPermission,
  answerQuestionOptions,
  dismissInbox,
  retryThread,
  selectThread,
  setView,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { orderedOptions, QuestionCard, RiskBadge } from "./AttentionCards";
import { ChevronDown, CircleAlert, CircleCheck, CornerDownRight, Hand, MessageCircleQuestion } from "./icons";
import { ViewHeader } from "./MainHeader";
import { ProjectMark } from "./ProjectMark";
import { cx, Kbd, Pill, Tooltip } from "./primitives";

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
    // Every answer, in the order the question card numbers them, so a digit picks the same one
    // in the row and in the card the selected row opens.
    const answers = request.options.filter((option) => isAllow(option.kind) || !option.kind);
    return answers.slice(0, 7).map(
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

/** The answers, with their digit hints: shown on the selected row. */
function RowActions({
  item,
  onRun,
  withoutAnswers,
}: {
  item: InboxItem;
  onRun: (action: RowAction) => void;
  /** The question's card is open under the row and holds its answers: only Open and Dismiss here. */
  withoutAnswers?: boolean;
}) {
  const actions = inboxActions(item);
  const answers = withoutAnswers ? rowAnswers(item).length : 0;
  return (
    <>
      {actions.map((action, i) =>
        i < answers ? null : (
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
        ),
      )}
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
 * answer at rest. The selected row shows all answers with their digit keys.
 */
function InboxRow({
  item,
  active,
  now,
  onSelect,
  onOpen,
  onRun,
}: {
  item: InboxItem;
  active: boolean;
  now: number;
  onSelect: () => void;
  onOpen: () => void;
  onRun: (action: RowAction) => void;
}) {
  const actions = inboxActions(item);
  const risk =
    item.kind === "approval" && item.request ? assessApproval(item.request, item.project) : undefined;
  const compact =
    risk?.level === "destructive"
      ? openAction(item)
      : (actions.find((action) => action.primary) ?? actions[0]);
  // The selected question answers in place: its whole card (every answer with what it means, a
  // multiple choice's ticks, the user's own words and files) opens under the row, the same draft
  // as the thread's own card (state/questionDrafts.ts).
  const opensCard = active && !!item.request?.question && !item.request.secret;
  return (
    <li
      data-inbox-key={item.key}
      aria-current={active || undefined}
      className={cx(
        "flex items-start gap-2 rounded-row px-2 py-2",
        active ? "bg-surface" : "hover:bg-fill-subtle",
      )}
    >
      <span className="mt-1 shrink-0">
        <KindIcon kind={item.kind} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          {opensCard ? (
            // The card under the row says the question whole; the row keeps its count and age.
            <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
              <RowActions item={item} onRun={onRun} withoutAnswers />
            </span>
          ) : (
            <button
              type="button"
              onClick={onOpen}
              className={cx(
                "min-w-0 flex-1 basis-32 text-left text-callout hover:underline",
                active ? "wrap-anywhere text-fg" : "truncate text-fg-muted",
              )}
            >
              {item.detail}
            </button>
          )}
          {risk && risk.level !== "safe" && <RiskBadge risk={risk} />}
          {item.pending > 1 && (
            <span className="shrink-0 text-caption text-fg-faint tabular">
              {item.number} of {item.pending}
            </span>
          )}
          <span className="shrink-0 text-caption text-fg-faint tabular">{relativeAge(item.at, now)}</span>
          {!active && compact && (
            <Pill size="sm" prominent={compact.primary} onClick={() => onRun(compact)} className="max-w-full">
              <span className="min-w-0 truncate">{compact.label}</span>
            </Pill>
          )}
        </div>
        {active && !opensCard && (
          <div className="flex min-w-0 flex-wrap items-center gap-1.5 pt-1">
            <RowActions item={item} onRun={onRun} />
          </div>
        )}
        {opensCard && item.request && <QuestionCard request={item.request} keys={false} />}
      </div>
      <button
        type="button"
        aria-label={`Request actions: ${item.detail}`}
        aria-expanded={active}
        onClick={onSelect}
        className="flex size-6 shrink-0 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
      >
        <ChevronDown size={14} className={active ? "rotate-180" : undefined} />
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
 * It counts distinct threads in the sidebar's active filter; the destination still
 * opens the full inbox. The 6px under it closes the top block rather than heading the list.
 */
export function NeedsYouRow({ threadIds }: { threadIds: ReadonlySet<string> }) {
  const items = useInboxItems();
  const here = useApp((s) => s.view.kind === "inbox");
  const count = filteredNeedsCount(items, threadIds);
  if (!count) return null;
  return (
    <div className="shrink-0 px-3 pb-1.5">
      <Tooltip label="Threads in the current filter that need you. Open the inbox for all projects.">
        <button
          type="button"
          onClick={() => setView({ kind: "inbox" })}
          aria-current={here ? "page" : undefined}
          className="flex h-[38px] w-full items-center gap-2 rounded-md px-3 text-left text-callout text-fg-muted hover:bg-fill hover:text-fg"
        >
          <Hand size={14} className="shrink-0 text-warning" fill="currentColor" fillOpacity={0.2} />
          <span className="min-w-0 flex-1 truncate font-medium">Needs you</span>
          <span
            key={count}
            className="rc-count-roll shrink-0 rounded-full bg-warning/14 px-1.5 font-medium text-caption text-warning tabular"
          >
            {count > 99 ? "99+" : count}
          </span>
        </button>
      </Tooltip>
    </div>
  );
}

/**
 * The inbox itself. It keeps one row lit for the keyboard, folds answered rows
 * into Decided.
 */
export function Inbox() {
  const items = useInboxItems();
  const now = useClock();
  const groups = useMemo(() => groupInbox(items), [items]);
  const threadCount = useMemo(() => new Set(items.map((item) => item.threadId)).size, [items]);
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

  const readOnly = useMemo(() => readOnlyApprovals(items), [items]);
  const allowReadOnly = useCallback(() => {
    for (const item of readOnly) {
      const option =
        item.request && orderedOptions(item.request.options).find((o) => optionScope(o) === "once");
      if (!item.request || !option) continue;
      labels.current[item.key] = "Approved";
      answerPermission(item.request, option);
    }
  }, [readOnly]);

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
          if (at < 0) return e.key === "ArrowDown" ? 0 : items.length - 1;
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
    <main
      className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
      aria-label="Needs you"
      data-focus-area="main"
    >
      <ViewHeader
        title={
          <span className="flex items-center gap-2">
            Needs you
            {threadCount > 0 && (
              <span
                key={threadCount}
                className="rc-count-roll rounded-full bg-warning/14 px-1.5 font-medium text-caption text-warning tabular"
              >
                {threadCount > 99 ? "99+" : threadCount}
              </span>
            )}
          </span>
        }
      >
        {readOnly.length > 1 && (
          <Tooltip label="Approve once every waiting call that only reads: listing, searching, viewing files">
            <Pill size="sm" onClick={allowReadOnly}>
              Allow {readOnly.length} read-only
            </Pill>
          </Tooltip>
        )}
        {items.length > 0 && (
          <Pill size="sm" quiet onClick={dismissAll}>
            Dismiss all
          </Pill>
        )}
      </ViewHeader>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-10 sm:px-12">
        <div className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-4 pt-1">
          {items.length > 0 ? (
            <section aria-label="Waiting on you" className="flex flex-col">
              <span className="px-2 pb-1 font-medium text-caption text-fg-faint">Waiting on you</span>
              <ul ref={list} className="flex flex-col gap-3">
                {groups.map((group, g) => (
                  <li key={`${group.threadId}:${group.items[0]!.key}`} className="flex flex-col">
                    <InboxGroupHeader group={group} onOpen={() => selectThread(group.threadId)} />
                    <ul className="flex flex-col gap-1">
                      {group.items.map((item, i) => (
                        <InboxRow
                          key={item.key}
                          item={item}
                          active={starts[g]! + i === active}
                          now={now}
                          onSelect={() => setActive((at) => (at === starts[g]! + i ? -1 : starts[g]! + i))}
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
              <CircleCheck size={18} className="text-fg-faint" />
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
