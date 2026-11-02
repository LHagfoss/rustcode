// Approval and question cards over the composer, after AttentionCards.swift.
// Only the selected thread's first pending request shows. Buttons keep the
// engine's option names, ordered reject → always → allow once (prominent).
// Keys (when no text field has focus): 1–9 pick an option in button order.

import { CircleArrowUp, Hand, MessageCircleQuestion, ShieldAlert } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { isAllow, type PermissionOption, type PermissionRequest } from "../acp/types";
import { answerPermission, answerQuestionOptions, answerQuestionText } from "../state/store";
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
    <section
      aria-label={label}
      className="surface flex flex-col gap-3 rounded-[20px] border border-fg/10 p-4 rc-in"
    >
      {children}
    </section>
  );
}

export function PermissionCard({ request }: { request: PermissionRequest }) {
  const [details, setDetails] = useState(false);
  const options = useMemo(() => orderedOptions(request.options), [request.options]);
  const pick = (i: number) => {
    const option = options[i];
    if (option) answerPermission(request, option);
  };
  useDigitKeys(options.length, pick);
  const command = field(request.input, "command");
  const cwd = field(request.input, "cwd") ?? field(request.input, "workdir");
  const summary = field(request.input, "summary");
  const note = field(request.input, "reason");
  return (
    <Shell label="Approval needed">
      <div className="flex items-start gap-2.5">
        <Hand size={15} className="mt-0.5 shrink-0 text-attention" fill="currentColor" fillOpacity={0.2} />
        <div className="min-w-0 flex-1">
          <div className="truncate font-semibold text-body">{request.title}</div>
          {summary && <div className="text-callout text-fg-2">{summary}</div>}
        </div>
        <span className="shrink-0 font-medium text-attention text-caption">Needs approval</span>
        {request.input !== undefined && (
          <button
            type="button"
            onClick={() => setDetails(!details)}
            className="shrink-0 text-brand text-caption hover:underline"
          >
            {details ? "Hide details" : "Details"}
          </button>
        )}
      </div>
      {command && (
        <div>
          <pre className="selectable m-0 max-h-[140px] overflow-auto rounded-lg bg-fill-3 p-2.5 font-mono text-callout whitespace-pre-wrap">
            <span className="text-fg-2">$ </span>
            {command}
          </pre>
          {cwd && (
            <div className="mt-1 truncate text-caption text-fg-2" dir="rtl">
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
        <div className="flex items-center gap-1.5 text-attention text-callout">
          <ShieldAlert size={13} />
          {note}
        </div>
      )}
      {details && (
        <pre className="selectable m-0 max-h-[160px] overflow-auto rounded-lg bg-fill-3 p-2.5 font-mono text-caption">
          {JSON.stringify(request.input, null, 2)}
        </pre>
      )}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {options.map((o, i) => {
          const backtick = o.kind === "allow_always" && o.name.includes("`");
          return (
            <Tooltip key={o.id} label={backtick ? o.name : undefined}>
              <Pill size="lg" prominent={o.kind === "allow_once"} onClick={() => pick(i)}>
                {backtick ? "Always allow this" : o.name}
                <span
                  className={cx(
                    "font-mono text-caption",
                    o.kind === "allow_once" ? "text-white/60" : "text-fg-3",
                  )}
                >
                  {i + 1}
                </span>
              </Pill>
            </Tooltip>
          );
        })}
      </div>
    </Shell>
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
      <div className="flex items-center gap-2 font-semibold text-body text-brand">
        <MessageCircleQuestion size={15} />
        {q.header || "RustCode has a question"}
      </div>
      <div className="selectable text-body">{q.text}</div>
      <div className={cx("flex flex-col", q.multiple ? "gap-1.5" : "gap-1")}>
        {answers.map((o, i) =>
          q.multiple ? (
            <label key={o.id} className="flex items-start gap-2 rounded-xl px-1 py-0.5">
              <input
                type="checkbox"
                className="mt-0.5 accent-[var(--rc-brand-fill)]"
                checked={ticked.includes(o.id)}
                onChange={(e) =>
                  setTicked(e.target.checked ? [...ticked, o.id] : ticked.filter((x) => x !== o.id))
                }
              />
              <span>
                <span className="block text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block text-caption text-fg-2">{q.descriptions[i]}</span>
                )}
              </span>
            </label>
          ) : (
            <button
              key={o.id}
              type="button"
              onClick={() => answerQuestionOptions(request, [o])}
              className="group flex w-full items-center gap-2 rounded-xl bg-fill-3 px-3 py-[9px] text-left transition-colors hover:bg-brand/14"
            >
              <Kbd>{i + 1}</Kbd>
              <span className="flex-1">
                <span className="block text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block text-caption text-fg-2">{q.descriptions[i]}</span>
                )}
              </span>
              <CircleArrowUp
                size={16}
                className="text-brand opacity-0 transition-opacity group-hover:opacity-100"
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
          className="h-[30px] flex-1 rounded-full bg-field px-3.5 text-body outline-none ring-1 ring-hairline placeholder:text-fg-3 focus:ring-brand/50"
        />
        <Pill prominent type="submit" disabled={!text.trim()}>
          Send
        </Pill>
        {reject && <Pill onClick={() => answerPermission(request, reject)}>{reject.name}</Pill>}
      </form>
    </Shell>
  );
}

export function AttentionCard({ request }: { request: PermissionRequest }) {
  return request.question ? (
    <QuestionCard key={`${request.rpcId}`} request={request} />
  ) : (
    <PermissionCard key={`${request.rpcId}`} request={request} />
  );
}
