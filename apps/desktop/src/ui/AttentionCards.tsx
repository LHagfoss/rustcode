// What the thread is waiting on: an approval is a row in the transcript (ApprovalRow) saying what
// it wants, answered from one slim bar over the composer (ApprovalNotice), which owns the buttons
// and the keys (when no text field has focus): 1–9 pick an option in the engine's order reject → always → allow once, and ⏎ approves
// while esc declines. A question keeps its form in the bar, because the answer is a choice, and
// leaves a row in the transcript once it is answered (Rows.tsx QuestionRow). A request from a
// thread below this one (ChildRequestCard) uses the same cards, under a header naming it.

import {
  ArrowUpRight,
  CheckCheck,
  ChevronDown,
  CircleArrowUp,
  CornerDownRight,
  Hand,
  MessageCircleQuestion,
  ShieldAlert,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { isAllow, type PermissionOption, type PermissionRequest } from "../acp/types";
import { displayCommand } from "../lib/command";
import {
  answerPermission,
  answerQuestionOptions,
  answerQuestionText,
  approveAll,
  selectThread,
} from "../state/store";
import { cx, Kbd, MenuContent, MenuShortcut, menuItem, Pill, Tooltip } from "./primitives";
import { DiffView } from "./Rows";

function orderedOptions(options: PermissionOption[]): PermissionOption[] {
  const rank = (o: PermissionOption) =>
    o.kind === "reject_once" || o.kind === "reject_always" ? 0 : o.kind === "allow_once" ? 2 : 1;
  return [...options].sort((a, b) => rank(a) - rank(b));
}

function field(input: Json | undefined, key: string): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const v = input[key];
  return typeof v === "string" ? v : undefined;
}

function useDigitKeys(count: number, pick: (index: number) => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable]") || e.metaKey || e.ctrlKey || e.altKey) return;
      const n = Number.parseInt(e.key, 10);
      if (n >= 1 && n <= count) {
        e.preventDefault();
        pick(n - 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [count, pick]);
}

/** ⏎ approves and Esc declines, unless something else has the keyboard: a field, a menu or a dialog. */
function useApprovalKeys(decline?: () => void, approve?: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" && e.key !== "Enter") return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || e.isComposing) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, button, a, [contenteditable]")) return;
      if (document.querySelector('[role="menu"], [role="dialog"], [role="listbox"]')) return;
      const run = e.key === "Enter" ? approve : decline;
      if (!run) return;
      e.preventDefault();
      run();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [decline, approve]);
}

/**
 * The answers to one approval, ordered for the digit keys, and the three that get a button of
 * their own: reject, allow once, always allow. Shared by the timeline row and the bar over the
 * composer, so both offer the same answers in the same order.
 */
function useApproval(request: PermissionRequest) {
  const options = useMemo(() => orderedOptions(request.options), [request.options]);
  const pick = (i: number) => {
    const option = options[i];
    if (option) answerPermission(request, option);
  };
  const decline =
    options.find((o) => o.kind === "reject_once") ?? options.find((o) => o.kind === "reject_always");
  const allow =
    options.find((o) => o.kind === "allow_once") ?? options.find((o) => o.kind === "allow_always");
  const alwaysAllow = options.find((o) => o.kind === "allow_always" && o !== allow);
  const always = options.filter((o) => o !== decline && o !== allow && o !== alwaysAllow);
  return { options, pick, decline, allow, alwaysAllow, always };
}

/**
 * The buttons either of them offers, at the timeline's 24px pill: the rarer answers in More
 * (always allow, and Allow all when the turn waits on a batch), then always allow, Decline and
 * Approve.
 */
function ApprovalActions({
  request,
  pending = 1,
  answers,
}: {
  request: PermissionRequest;
  pending?: number;
  answers: ReturnType<typeof useApproval>;
}) {
  const { options, pick, decline, allow, alwaysAllow, always } = answers;
  return (
    <>
      {(always.length > 0 || pending > 1) && (
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <Pill size="sm" aria-label="More answers">
              More <ChevronDown size={11} />
            </Pill>
          </DropdownMenu.Trigger>
          <MenuContent side="top" align="end" className="max-w-[360px] min-w-[220px]">
            {always.map((o) => (
              <DropdownMenu.Item key={o.id} onSelect={() => pick(options.indexOf(o))} className={menuItem}>
                <span className="min-w-0 flex-1 break-words">{o.name}</span>
                <MenuShortcut>{options.indexOf(o) + 1}</MenuShortcut>
              </DropdownMenu.Item>
            ))}
            {pending > 1 && (
              <DropdownMenu.Item onSelect={() => approveAll(request.sessionId)} className={menuItem}>
                <CheckCheck size={13} /> Allow all {pending} waiting
              </DropdownMenu.Item>
            )}
          </MenuContent>
        </DropdownMenu.Root>
      )}
      {alwaysAllow && (
        <Tooltip label={`${alwaysAllow.name} (${options.indexOf(alwaysAllow) + 1})`}>
          <Pill size="sm" quiet onClick={() => pick(options.indexOf(alwaysAllow))}>
            Always allow
          </Pill>
        </Tooltip>
      )}
      {decline && (
        <Tooltip label={`${decline.name} (${options.indexOf(decline) + 1})`}>
          <Pill size="sm" onClick={() => pick(options.indexOf(decline))}>
            Decline <Kbd>esc</Kbd>
          </Pill>
        </Tooltip>
      )}
      {allow && (
        <Tooltip label={`${allow.name} (${options.indexOf(allow) + 1})`}>
          <Pill size="sm" prominent onClick={() => pick(options.indexOf(allow))}>
            Approve <Kbd>⏎</Kbd>
          </Pill>
        </Tooltip>
      )}
    </>
  );
}

function Shell({ children, label }: { children: React.ReactNode; label: string }) {
  return (
    <section aria-label={label} className="elevated flex flex-col gap-3 p-4 rc-in">
      {children}
    </section>
  );
}

/**
 * An approval as a row in the transcript, at the call it is about: the command it wants to run
 * (at most 160px, then it scrolls) and any change it wants to make. The bar over the composer
 * answers the first one waiting, so only a later approval, which the bar does not show yet, has
 * its own buttons here.
 */
export function ApprovalRow({
  request,
  pending = 1,
  number = 1,
}: {
  request: PermissionRequest;
  pending?: number;
  /** Which of the waiting approvals this is, from 1. */
  number?: number;
}) {
  const answers = useApproval(request);
  const inBar = number === 1;
  const command = field(request.input, "command") ?? field(request.input, "cmd");
  const note = field(request.input, "reason");
  return (
    <div className="flex flex-col gap-1.5" role="group" aria-label="Approval needed">
      <div className="flex items-center gap-2">
        <span className="flex w-[18px] shrink-0 justify-center">
          <Hand size={16} className="text-warning" fill="currentColor" fillOpacity={0.2} />
        </span>
        <span className="shrink-0 font-medium text-body text-warning">Waiting for approval</span>
        <span className="min-w-0 truncate font-mono text-callout text-fg-muted" title={request.title}>
          {displayCommand(request.title)}
        </span>
        {pending > 1 && (
          <span className="shrink-0 text-caption text-fg-muted tabular">
            {number} of {pending}
          </span>
        )}
      </div>
      {(command || request.diffs.length > 0 || request.input !== undefined) && (
        // One body, at most 160px: the command and the change it would make, then it scrolls.
        <div className="ml-[26px] flex max-h-[160px] flex-col gap-2 overflow-y-auto">
          {command && (
            <pre className="selectable m-0 rounded-md bg-fill-subtle p-2.5 font-mono text-callout whitespace-pre-wrap">
              <span className="text-fg-muted">$ </span>
              {command}
            </pre>
          )}
          {request.diffs.map((diff, i) => (
            <DiffView key={i} diff={diff} />
          ))}
          {!command && !request.diffs.length && request.input !== undefined && (
            <pre className="selectable m-0 rounded-md bg-fill-subtle p-2.5 font-mono text-caption">
              {JSON.stringify(request.input, null, 2)}
            </pre>
          )}
        </div>
      )}
      {note && (
        <div className="ml-[26px] flex items-center gap-1.5 text-warning text-callout">
          <ShieldAlert size={13} />
          {note}
        </div>
      )}
      {!inBar && (
        <div className="ml-[26px] flex items-center gap-1.5">
          <ApprovalActions request={request} pending={pending} answers={answers} />
        </div>
      )}
    </div>
  );
}

/**
 * The bar over the composer: one slim line naming the approval that waits, with its answers. It is
 * also the keyboard target (1–9, ⏎, esc), so it can be answered from anywhere in the thread.
 */
export function ApprovalNotice({
  request,
  pending = 1,
  number = 1,
  from,
  keys = true,
}: {
  request: PermissionRequest;
  pending?: number;
  /** Which of `pending` approvals this is, from 1. */
  number?: number;
  /** The thread asking, when it is a thread this one started and not this one. */
  from?: string;
  /** Whether 1–9, ⏎ and esc answer it: only the first card on screen takes them. */
  keys?: boolean;
}) {
  const answers = useApproval(request);
  const { options, pick, decline, allow } = answers;
  useDigitKeys(keys ? options.length : 0, pick);
  useApprovalKeys(
    keys && decline ? () => pick(options.indexOf(decline)) : undefined,
    keys && allow ? () => pick(options.indexOf(allow)) : undefined,
  );
  return (
    <section
      aria-label="Approval needed"
      className="elevated flex min-h-[34px] items-center gap-2 rounded-2xl px-2.5 py-1 rc-in"
    >
      <Hand size={14} className="ml-1 shrink-0 text-warning" fill="currentColor" fillOpacity={0.2} />
      {from && <span className="max-w-[30%] shrink-0 truncate text-callout text-fg-muted">From {from}</span>}
      <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">
        Allow{" "}
        <span className="font-medium text-body text-fg" title={request.title}>
          {displayCommand(request.title)}
        </span>
        ?
      </span>
      {pending > 1 && (
        <span className="shrink-0 text-caption text-fg-muted tabular">
          {number} of {pending}
        </span>
      )}
      <span className="flex shrink-0 items-center gap-1">
        <ApprovalActions request={request} pending={pending} answers={answers} />
      </span>
    </section>
  );
}

export function QuestionCard({
  request,
  from,
  keys = true,
}: {
  request: PermissionRequest;
  from?: string;
  /** Whether 1–9 answer it: only the first card on screen takes them. */
  keys?: boolean;
}) {
  const q = request.question!;
  const answers = request.options.filter((o) => isAllow(o.kind) || !o.kind);
  const reject = request.options.find((o) => o.kind === "reject_once" || o.kind === "reject_always");
  const [ticked, setTicked] = useState<string[]>([]);
  const [text, setText] = useState("");
  useDigitKeys(keys && !q.multiple ? answers.length : 0, (i) => {
    const option = answers[i];
    if (option) answerQuestionOptions(request, [option]);
  });
  return (
    <Shell label="Question">
      <div className="flex items-center gap-2 font-semibold text-body text-accent">
        <MessageCircleQuestion size={15} />
        {from ? `From ${from}: ` : ""}
        {q.header || "RustCode has a question"}
      </div>
      <div className="selectable text-body">{q.text}</div>
      <div className={cx("flex flex-col", q.multiple ? "gap-1.5" : "gap-1")}>
        {answers.map((o, i) =>
          q.multiple ? (
            <label key={o.id} className="flex items-start gap-2 rounded-lg px-1 py-0.5">
              <input
                type="checkbox"
                className="mt-0.5 accent-[var(--rc-accent)]"
                checked={ticked.includes(o.id)}
                onChange={(e) =>
                  setTicked(e.target.checked ? [...ticked, o.id] : ticked.filter((x) => x !== o.id))
                }
              />
              <span>
                <span className="block text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block text-caption text-fg-muted">{q.descriptions[i]}</span>
                )}
              </span>
            </label>
          ) : (
            <button
              key={o.id}
              type="button"
              onClick={() => answerQuestionOptions(request, [o])}
              className="group flex w-full items-center gap-2 rounded-lg bg-fill-subtle px-3 py-[9px] text-left hover:bg-accent/14"
            >
              <Kbd>{i + 1}</Kbd>
              <span className="flex-1">
                <span className="block text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block text-caption text-fg-muted">{q.descriptions[i]}</span>
                )}
              </span>
              <CircleArrowUp
                size={16}
                className="text-accent opacity-0 transition-opacity group-hover:opacity-100"
              />
            </button>
          ),
        )}
      </div>
      {q.multiple && (
        <div className="flex justify-end">
          <Pill
            prominent
            disabled={!ticked.length}
            onClick={() =>
              answerQuestionOptions(
                request,
                answers.filter((o) => ticked.includes(o.id)),
              )
            }
          >
            Submit
          </Pill>
        </div>
      )}
      <form
        className="flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          answerQuestionText(request, text);
        }}
      >
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Or write your own answer…"
          aria-label="Your own answer"
          className="h-[30px] flex-1 rounded-full bg-fill-subtle px-3.5 text-body outline-none placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
        />
        <Pill prominent type="submit" disabled={!text.trim()}>
          Send
        </Pill>
        {reject && <Pill onClick={() => answerPermission(request, reject)}>{reject.name}</Pill>}
      </form>
    </Shell>
  );
}

export function AttentionCard({
  request,
  pending = 1,
  number = 1,
  from,
  keys = true,
}: {
  request: PermissionRequest;
  pending?: number;
  number?: number;
  from?: string;
  keys?: boolean;
}) {
  return request.question ? (
    <QuestionCard key={`${request.rpcId}`} request={request} from={from} keys={keys} />
  ) : (
    <ApprovalNotice
      key={`${request.rpcId}`}
      request={request}
      pending={pending}
      number={number}
      from={from}
      keys={keys}
    />
  );
}

/**
 * A request from a thread this one started, at the transcript's foot: a small
 * header naming the thread (a click opens it), then the same card that thread's
 * own chat shows. Its answer goes through that card's own function, so the two
 * places cannot answer differently.
 */
export function ChildRequestCard({
  request,
  from,
  pending = 1,
  number = 1,
  keys = false,
}: {
  request: PermissionRequest;
  from: string;
  pending?: number;
  number?: number;
  /** Only the first card on screen takes the digit and ⏎/esc keys. */
  keys?: boolean;
}) {
  return (
    <div
      className="flex flex-col gap-1.5"
      role="group"
      aria-label={`${request.question ? "Question" : "Approval"} from ${from}`}
    >
      <button
        type="button"
        onClick={() => selectThread(request.sessionId)}
        aria-label={`Open ${from}`}
        className="group flex w-full min-w-0 items-center gap-1.5 rounded-sm px-0.5 py-0.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
      >
        <CornerDownRight size={12} className="shrink-0 text-fg-faint" />
        <span className="min-w-0 truncate">
          From <span className="font-medium text-fg">{from}</span>
        </span>
        <ArrowUpRight
          size={11}
          className="shrink-0 text-fg-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
        />
      </button>
      <AttentionCard request={request} pending={pending} number={number} keys={keys} />
    </div>
  );
}
