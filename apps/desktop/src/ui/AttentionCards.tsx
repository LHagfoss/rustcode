// Approval and question cards over the composer, after AttentionCards.swift.
// Only the selected thread's first pending request shows. An approval offers
// Decline and Allow (prominent); the rarer answers (always allow, always
// reject, approve all) sit in a More menu, as T3 Code does. Keys (when no
// text field has focus): 1–9 pick an option in the engine's order
// reject → always → allow once, shown on each button's tooltip.

import {
  CheckCheck,
  ChevronDown,
  ChevronRight,
  CircleArrowUp,
  Hand,
  MessageCircleQuestion,
  ShieldAlert,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { isAllow, type PermissionOption, type PermissionRequest } from "../acp/types";
import { displayCommand } from "../lib/command";
import { answerPermission, answerQuestionOptions, answerQuestionText, approveAll } from "../state/store";
import { cx, Kbd, Pill, Tooltip } from "./primitives";
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

function Shell({ children, label }: { children: React.ReactNode; label: string }) {
  return (
    <section aria-label={label} className="elevated flex flex-col gap-3 rounded-xl p-4 rc-in">
      {children}
    </section>
  );
}

/**
 * An approval as one slim bar over the composer: what it wants, Details to
 * open the command, diff and raw input, and the engine's options. When the
 * turn waits on several at once (a batch), "1 of N" and Approve all.
 */
export function PermissionCard({ request, pending = 1 }: { request: PermissionRequest; pending?: number }) {
  const command = field(request.input, "command");
  const summary = field(request.input, "summary");
  const [details, setDetails] = useState(request.diffs.length > 0 && request.diffs.length <= 1);
  const options = useMemo(() => orderedOptions(request.options), [request.options]);
  const pick = (i: number) => {
    const option = options[i];
    if (option) answerPermission(request, option);
  };
  useDigitKeys(options.length, pick);
  // The two everyday answers get buttons; the rest go in More.
  const decline =
    options.find((o) => o.kind === "reject_once") ?? options.find((o) => o.kind === "reject_always");
  const allow =
    options.find((o) => o.kind === "allow_once") ?? options.find((o) => o.kind === "allow_always");
  const always = options.filter((o) => o !== decline && o !== allow);
  const cwd = field(request.input, "cwd") ?? field(request.input, "workdir");
  const note = field(request.input, "reason");
  const expandable = request.input !== undefined || request.diffs.length > 0;
  const title = displayCommand(request.title);
  const shownCommand = command && displayCommand(command);
  // The command once: harnesses that title the request with it get no second copy beside it.
  const aside = summary ?? (shownCommand && !title.includes(shownCommand) ? shownCommand : undefined);
  return (
    <section
      aria-label="Approval needed"
      className="elevated flex flex-col gap-2 rounded-xl px-2.5 py-2 rc-in"
    >
      <div className="flex min-h-[30px] items-center gap-2">
        <Hand size={14} className="ml-1 shrink-0 text-warning" fill="currentColor" fillOpacity={0.2} />
        <button
          type="button"
          disabled={!expandable}
          aria-expanded={details}
          onClick={() => setDetails(!details)}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded-sm px-1 py-0.5 text-left enabled:hover:bg-fill-subtle"
        >
          {/* A long command truncates inside the bar; Details shows it whole. */}
          <span className="min-w-0 shrink truncate font-semibold text-body" title={request.title}>
            {title}
          </span>
          {aside && (
            <span
              className={cx("min-w-0 flex-1 truncate text-callout text-fg-muted", !summary && "font-mono")}
            >
              {aside}
            </span>
          )}
          {!aside && <span className="flex-1" />}
          {expandable && (
            <ChevronRight
              size={11}
              className={cx(
                "shrink-0 text-fg-faint transition-transform duration-200",
                details && "rotate-90",
              )}
            />
          )}
        </button>
        {pending > 1 && <span className="shrink-0 text-caption text-fg-muted tabular">1 of {pending}</span>}
      </div>
      {details && (
        <div className="flex flex-col gap-2 px-1 pb-1">
          {command && (
            <div>
              <pre className="selectable m-0 max-h-[140px] overflow-auto rounded-md bg-fill-subtle p-2.5 font-mono text-callout whitespace-pre-wrap">
                <span className="text-fg-muted">$ </span>
                {command}
              </pre>
              {cwd && (
                <div className="mt-1 truncate text-caption text-fg-muted" dir="rtl">
                  <bdi>{cwd}</bdi>
                </div>
              )}
            </div>
          )}
          {request.diffs.length > 0 && (
            <div className="flex max-h-[260px] flex-col gap-2 overflow-y-auto">
              {request.diffs.map((d, i) => (
                <DiffView key={i} diff={d} />
              ))}
            </div>
          )}
          {note && (
            <div className="flex items-center gap-1.5 text-warning text-callout">
              <ShieldAlert size={13} />
              {note}
            </div>
          )}
          {!command && !request.diffs.length && request.input !== undefined && (
            <pre className="selectable m-0 max-h-[160px] overflow-auto rounded-md bg-fill-subtle p-2.5 font-mono text-caption">
              {JSON.stringify(request.input, null, 2)}
            </pre>
          )}
        </div>
      )}
      <div className="flex items-center gap-1.5 pl-1">
        <span className="flex min-w-0 flex-1 items-center gap-1 text-caption text-warning">
          <ShieldAlert size={12} className="shrink-0" />
          <span className="truncate">Waiting for your approval</span>
        </span>
        {(always.length > 0 || pending > 1) && (
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <Pill aria-label="More answers">
                More <ChevronDown size={11} />
              </Pill>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                side="top"
                align="end"
                sideOffset={6}
                className="elevated z-50 max-w-[360px] min-w-[220px] rounded-xl p-1.5 rc-in"
              >
                {always.map((o) => (
                  <DropdownMenu.Item
                    key={o.id}
                    onSelect={() => pick(options.indexOf(o))}
                    className="flex items-center gap-2 rounded-lg px-3 py-[7px] text-body outline-none data-[highlighted]:bg-fill"
                  >
                    <span className="min-w-0 flex-1 break-words">{o.name}</span>
                    <Kbd>{options.indexOf(o) + 1}</Kbd>
                  </DropdownMenu.Item>
                ))}
                {pending > 1 && (
                  <DropdownMenu.Item
                    onSelect={() => approveAll(request.sessionId)}
                    className="flex items-center gap-2 rounded-lg px-3 py-[7px] text-body outline-none data-[highlighted]:bg-fill"
                  >
                    <CheckCheck size={13} /> Allow all {pending} waiting
                  </DropdownMenu.Item>
                )}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        )}
        {decline && (
          <Tooltip label={`${decline.name} (${options.indexOf(decline) + 1})`}>
            <Pill onClick={() => pick(options.indexOf(decline))}>Decline</Pill>
          </Tooltip>
        )}
        {allow && (
          <Tooltip label={`${allow.name} (${options.indexOf(allow) + 1})`}>
            <Pill prominent onClick={() => pick(options.indexOf(allow))}>
              Allow
            </Pill>
          </Tooltip>
        )}
      </div>
    </section>
  );
}

export function QuestionCard({ request }: { request: PermissionRequest }) {
  const q = request.question!;
  const answers = request.options.filter((o) => isAllow(o.kind) || !o.kind);
  const reject = request.options.find((o) => o.kind === "reject_once" || o.kind === "reject_always");
  const [ticked, setTicked] = useState<string[]>([]);
  const [text, setText] = useState("");
  useDigitKeys(q.multiple ? 0 : answers.length, (i) => {
    const option = answers[i];
    if (option) answerQuestionOptions(request, [option]);
  });
  return (
    <Shell label="Question">
      <div className="flex items-center gap-2 font-semibold text-body text-accent">
        <MessageCircleQuestion size={15} />
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
          className="h-[30px] flex-1 rounded-full bg-fill-subtle px-3.5 text-body outline-none ring-1 ring-border placeholder:text-fg-faint focus:ring-accent/50"
        />
        <Pill prominent type="submit" disabled={!text.trim()}>
          Send
        </Pill>
        {reject && <Pill onClick={() => answerPermission(request, reject)}>{reject.name}</Pill>}
      </form>
    </Shell>
  );
}

export function AttentionCard({ request, pending = 1 }: { request: PermissionRequest; pending?: number }) {
  return request.question ? (
    <QuestionCard key={`${request.rpcId}`} request={request} />
  ) : (
    <PermissionCard key={`${request.rpcId}`} request={request} pending={pending} />
  );
}
