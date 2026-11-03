// Transcript rows, after TranscriptRows.swift: user bubbles on the right,
// replies full width without a bubble, quiet thought disclosures, one-line
// tool rows that open to their diff or output, and a turn footer.

import {
  ArrowRight,
  Book,
  Brain,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleCheck,
  CircleX,
  FilePlus,
  FileText,
  Globe,
  Image as ImageIcon,
  Layers,
  MoveRight,
  Pencil,
  RotateCcw,
  Search,
  SquareStop,
  Terminal,
  Trash2,
  Users,
  Wrench,
} from "lucide-react";
import { memo, type ReactNode, useMemo, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import type { FileDiff } from "../acp/types";
import { displayCommand } from "../lib/command";
import { splitReply } from "../smartui/parse";
import { splitImageMarkers, splitUrls } from "../state/compose";
import { foldLabel, type ToolGroup, type TurnFold } from "../state/fold";
import { editMessage, retryMessage, useApp } from "../state/store";
import {
  diffCounts,
  diffLines,
  formatDuration,
  type TranscriptItem,
  type TurnCompletion,
  thoughtHeadings,
} from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { openLightbox } from "./Lightbox";
import { LinkChip } from "./LinkChip";
import { CopyButton, Markdown } from "./Markdown";
import { Callout, cx, Spinner, Tooltip } from "./primitives";
import { SmartUIView } from "./SmartUIView";
import { VizView } from "./Viz";

type Item<K extends TranscriptItem["kind"]> = Extract<TranscriptItem, { kind: K }>;

const timeLabel = (at?: number) =>
  at ? new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }) : "";

function TextWithLinks({ text }: { text: string }) {
  const parts = useMemo(() => splitUrls(text), [text]);
  return <>{parts.map((p, i) => (p.url ? <LinkChip key={i} url={p.url} className="mx-px" /> : p.text))}</>;
}

export const UserRow = memo(function UserRow({
  item,
  threadId,
  rewind,
}: {
  item: Item<"user">;
  threadId?: string;
  /** Edit and Retry take the thread back to this message (`_rustcode/rewind`). */
  rewind?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const { text, images: markers } = useMemo(() => splitImageMarkers(item.text), [item.text]);
  const long = text.length > 600 || text.split("\n").length > 12;
  const attachments = item.attachments ?? [];
  const placeholders = attachments.some((a) => a.kind === "image") ? 0 : markers;
  return (
    <div className="group flex flex-col items-end pb-2.5 pl-20">
      {(attachments.length > 0 || placeholders > 0) && (
        <div className="mb-1.5 flex max-w-full flex-wrap justify-end gap-1.5">
          {attachments.map((a, i) =>
            a.kind === "image" && a.preview ? (
              <button
                key={i}
                type="button"
                aria-label={`Open ${a.name}`}
                className="rounded-lg"
                onClick={() => {
                  const pictures = attachments.flatMap((x) =>
                    x.kind === "image" && x.preview ? [{ src: x.preview, alt: x.name }] : [],
                  );
                  openLightbox(
                    pictures,
                    pictures.findIndex((x) => x.src === a.preview),
                  );
                }}
              >
                <img
                  src={a.preview}
                  alt={a.name}
                  title={a.name}
                  className="max-h-[160px] max-w-[240px] cursor-zoom-in rounded-lg object-cover shadow-1"
                />
              </button>
            ) : (
              <span
                key={i}
                className="flex h-[26px] items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 text-callout"
              >
                {a.kind === "image" ? (
                  <ImageIcon size={12} />
                ) : (
                  <FileText size={12} className="text-fg-muted" />
                )}
                <span className="max-w-[200px] truncate">{a.name}</span>
              </span>
            ),
          )}
          {Array.from({ length: placeholders }, (_, i) => (
            <span
              key={`m${i}`}
              className="flex h-[26px] items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 text-callout text-fg-muted"
            >
              <ImageIcon size={12} /> Image
            </span>
          ))}
        </div>
      )}
      <div className={cx("relative max-w-full rounded-xl bg-fill px-3.5 py-[9px]", !text && "hidden")}>
        <div
          className={cx(
            "selectable text-body leading-[20px] whitespace-pre-wrap break-words",
            long &&
              !open &&
              "max-h-[240px] overflow-hidden [mask-image:linear-gradient(to_bottom,black_75%,transparent)]",
          )}
        >
          <TextWithLinks text={text} />
        </div>
        {long && (
          <button
            type="button"
            onClick={() => setOpen(!open)}
            className="mt-1.5 -mr-1 ml-auto flex h-[26px] items-center gap-1.5 rounded-md px-3 font-medium text-caption text-fg-muted hover:bg-fill hover:text-fg"
          >
            {open ? "Show less" : "Show more"}
            {open ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
          </button>
        )}
      </div>
      <div className="mt-1 flex h-[22px] items-center gap-1 text-caption text-fg-faint">
        {item.steered && <span className="text-accent">Steered</span>}
        {item.fromView && <span className="text-accent">From view</span>}
        <Tooltip label={item.at ? new Date(item.at).toLocaleString() : undefined}>
          <span className="tabular">{timeLabel(item.at)}</span>
        </Tooltip>
        <CopyButton text={item.text} iconOnly label="Copy message" />
        {threadId && (
          <>
            <Tooltip
              label={
                rewind ? "Edit and resend: the thread goes back to this message" : "Edit in the composer"
              }
            >
              <button
                type="button"
                aria-label="Edit message"
                onClick={() => editMessage(threadId, item.id)}
                className="inline-flex size-[22px] items-center justify-center rounded-md text-fg-muted opacity-0 transition-opacity hover:bg-fill hover:text-fg focus-visible:opacity-100 group-hover:opacity-100"
              >
                <Pencil size={12} />
              </button>
            </Tooltip>
            <Tooltip label={rewind ? "Retry: send it again from here" : "Send it again"}>
              <button
                type="button"
                aria-label="Retry message"
                onClick={() => retryMessage(threadId, item.id)}
                className="inline-flex size-[22px] items-center justify-center rounded-md text-fg-muted opacity-0 transition-opacity hover:bg-fill hover:text-fg focus-visible:opacity-100 group-hover:opacity-100"
              >
                <RotateCcw size={12} />
              </button>
            </Tooltip>
          </>
        )}
      </div>
    </div>
  );
});

export const AssistantRow = memo(function AssistantRow({
  item,
  streaming = false,
  threadId,
  copyable = true,
}: {
  item: Item<"assistant">;
  streaming?: boolean;
  threadId?: string;
  /** False for the reply the turn's footer already copies: one copy button per reply. */
  copyable?: boolean;
}) {
  const segments = useMemo(() => splitReply(item.text), [item.text]);
  // Each reply block copies on its own, from the margin: faint until the row is hovered, and out of
  // the text's flow so a streaming block never moves.
  const copy = copyable && item.text.trim() && (
    <span className="absolute top-0 right-0 opacity-0 transition-opacity group-hover/reply:opacity-100 focus-within:opacity-100">
      <CopyButton text={item.text} iconOnly label="Copy this reply" />
    </span>
  );
  if (segments.length === 1 && segments[0]!.kind === "markdown")
    return (
      <div className={cx("group/reply relative", copy && "pr-7")}>
        <Markdown text={item.text} streaming={streaming} className={streaming ? "rc-stream" : undefined} />
        {copy}
      </div>
    );
  return (
    <div className={cx("group/reply relative flex flex-col gap-2.5", copy && "pr-7")}>
      {copy}
      {segments.map((segment, i) =>
        segment.kind === "markdown" ? (
          <Markdown key={i} text={segment.text} streaming={streaming && i === segments.length - 1} />
        ) : segment.kind === "ui" ? (
          <SmartUIView
            key={`ui${segment.index}`}
            source={segment.source}
            complete={segment.complete}
            streaming={streaming}
            threadId={threadId}
          />
        ) : (
          <VizView key={`viz${segment.index}`} source={segment.source} complete={segment.complete} />
        ),
      )}
    </div>
  );
});

export const ThoughtRow = memo(function ThoughtRow({ item, live }: { item: Item<"thought">; live: boolean }) {
  const [open, setOpen] = useState(false);
  const label = live
    ? "Thinking…"
    : item.durationMs !== undefined && item.durationMs >= 1000
      ? `Thought for ${formatDuration(item.durationMs)}`
      : item.summary
        ? "Reasoning summary"
        : "Thinking";
  const headings = thoughtHeadings(item.text);
  const tail = live ? item.text.split("\n").filter(Boolean).slice(-5).join("\n") : "";
  return (
    <div className="text-callout">
      <div className="flex max-w-full items-center gap-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="-mx-1 flex min-w-0 max-w-full items-center gap-1.5 rounded-sm px-1 text-fg-faint hover:text-fg-muted"
        >
          {live && <Spinner size={10} />}
          <span className="shrink-0">{label}</span>
          {!open && !live && headings.length > 0 && (
            <span className="truncate italic">· {headings.join(" · ")}</span>
          )}
          <ChevronRight
            size={11}
            className={cx("shrink-0 transition-transform duration-200", open && "rotate-90")}
          />
        </button>
        {open && item.text.trim() && <CopyButton text={item.text} iconOnly label="Copy thinking" />}
      </div>
      {(open || (live && tail)) && (
        <div className={cx("pt-2 text-fg-muted", live && !open && "line-clamp-5")}>
          <Markdown text={open ? item.text : tail} streaming={live} className="gap-1.5 text-callout" />
        </div>
      )}
    </div>
  );
});

// ---------------------------------------------------------------- tools

function toolKind(item: Item<"tool">): string {
  if (item.toolKind && item.toolKind !== "other") return item.toolKind;
  const t = item.title.toLowerCase();
  if (/^(glob|grep|rg|search|find|list|ls)/.test(t)) return "search";
  if (/^(read|cat|view|open)/.test(t)) return "read";
  if (/^(edit|write|apply|patch|create|replace)/.test(t)) return "edit";
  if (/^(bash|shell|exec|run|command)/.test(t)) return "execute";
  if (/^(web|fetch|http|read url|browse)/.test(t)) return "fetch";
  if (/(agent|task|delegate)/.test(t)) return "agent";
  if (/skill/.test(t)) return "skill";
  return "other";
}

const KIND_ICON: Record<string, typeof Wrench> = {
  read: FileText,
  edit: Pencil,
  delete: Trash2,
  move: MoveRight,
  search: Search,
  execute: Terminal,
  think: Brain,
  fetch: Globe,
  agent: Users,
  skill: Book,
  other: Wrench,
};

function inputField(input: Json | undefined, ...keys: string[]): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  for (const key of keys) {
    const v = input[key];
    if (typeof v === "string" && v) return v;
  }
  return undefined;
}

function toolDetail(item: Item<"tool">, kind: string): { text?: string; mono: boolean } {
  if (item.detail) return { text: item.detail, mono: true };
  if (kind === "execute") return { text: inputField(item.input, "command", "cmd"), mono: true };
  if (kind === "search") return { text: inputField(item.input, "pattern", "query", "path"), mono: true };
  if (kind === "fetch") return { text: inputField(item.input, "url", "query"), mono: false };
  return { text: inputField(item.input, "path", "file_path", "filePath", "file"), mono: true };
}

export function DiffCount({ added, removed }: { added: number; removed: number }) {
  if (!added && !removed) return null;
  return (
    <span className="shrink-0 font-medium font-mono text-caption tabular">
      {added > 0 && <span className="text-success">+{added}</span>}
      {added > 0 && removed > 0 && " "}
      {removed > 0 && <span className="text-danger">−{removed}</span>}
    </span>
  );
}

function TextBox({ text, label }: { text: string; label: string }) {
  const [all, setAll] = useState(false);
  const lines = text.replace(/\n$/, "").split("\n");
  const shown = all || lines.length <= 14 ? lines : lines.slice(0, 14);
  return (
    <div>
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="font-semibold text-caption text-fg-muted">{label}</span>
        {text.trim() && <CopyButton text={text} iconOnly label={`Copy ${label.toLowerCase()}`} />}
      </div>
      <pre className="selectable m-0 max-h-[420px] overflow-auto rounded-md bg-fill-subtle p-2.5 font-mono text-caption leading-[15px] whitespace-pre-wrap break-all">
        {shown.join("\n")}
      </pre>
      {lines.length > 14 && (
        <button
          type="button"
          onClick={() => setAll(!all)}
          className="mt-1 text-caption text-accent hover:underline"
        >
          {all ? "Show less" : `Show all ${lines.length} lines`}
        </button>
      )}
    </div>
  );
}

export function DiffView({ diff, header = true }: { diff: FileDiff; header?: boolean }) {
  const lines = useMemo(() => diffLines(diff), [diff]);
  const [all, setAll] = useState(false);
  const counts = useMemo(() => diffCounts([diff]), [diff]);
  const shown = all || lines.length <= 15 ? lines : lines.slice(0, 12);
  const width = String(
    Math.max(...lines.map((l) => (l.kind === "removed" ? l.oldNumber : l.newNumber) ?? 0), 1),
  ).length;
  return (
    <div>
      {header && (
        <div className="mb-1 flex items-center gap-1.5 text-caption">
          {diff.oldText === undefined ? (
            <FilePlus size={12} className="text-fg-muted" />
          ) : (
            <FileText size={12} className="text-fg-muted" />
          )}
          <span className="truncate font-mono">{diff.path}</span>
          <DiffCount {...counts} />
        </div>
      )}
      <div className="selectable overflow-x-auto rounded-md bg-fill-subtle py-1 font-mono text-caption leading-[16px]">
        <div className="min-w-fit">
          {shown.map((line, i) => (
            <div
              key={i}
              className={cx(
                "flex pr-3",
                line.kind === "added" && "bg-[var(--rc-diff-add)]",
                line.kind === "removed" && "bg-[var(--rc-diff-del)]",
              )}
            >
              <span
                className="shrink-0 pr-1.5 pl-2 text-right text-fg-muted/60 tabular"
                style={{ width: `${width + 2}ch` }}
              >
                {(line.kind === "removed" ? line.oldNumber : line.newNumber) ?? ""}
              </span>
              <span
                className={cx(
                  "w-3 shrink-0",
                  line.kind === "added" && "text-success",
                  line.kind === "removed" && "text-danger",
                )}
              >
                {line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}
              </span>
              <span className="whitespace-pre">{line.text || " "}</span>
            </div>
          ))}
        </div>
      </div>
      {lines.length > 15 && (
        <button
          type="button"
          onClick={() => setAll(!all)}
          className="mt-1 text-caption text-accent hover:underline"
        >
          {all ? "Show less" : `Show all ${lines.length} lines`}
        </button>
      )}
    </div>
  );
}

function inputWithoutSummary(input: Json | undefined): string | undefined {
  if (input === undefined || input === null) return undefined;
  if (typeof input !== "object" || Array.isArray(input)) return JSON.stringify(input, null, 2);
  const { summary: _summary, ...rest } = input;
  return Object.keys(rest).length ? JSON.stringify(rest, null, 2) : undefined;
}

export const ToolRow = memo(function ToolRow({ item, cwd }: { item: Item<"tool">; cwd: string }) {
  const kind = toolKind(item);
  const [open, setOpen] = useState(kind === "edit" && item.diffs.length > 0 && item.diffs.length <= 2);
  const Icon = KIND_ICON[kind] ?? Wrench;
  const running = item.status === "pending" || item.status === "in_progress";
  const failed = item.status === "failed";
  const detail = toolDetail(item, kind);
  const title = displayCommand(item.title);
  const relative =
    detail.text && cwd && detail.text.startsWith(`${cwd}/`) ? detail.text.slice(cwd.length + 1) : detail.text;
  const trimmed = relative && displayCommand(relative);
  // A command whose title already is the command line shows it once.
  const shownDetail = trimmed && !title.includes(trimmed) ? trimmed : undefined;
  // Why it failed, on the row itself: the first line of what it printed, when no exit code says it.
  const reason =
    failed && item.exitCode === undefined ? (item.output?.trim().split("\n")[0]?.slice(0, 120) ?? "") : "";
  const counts = useMemo(() => diffCounts(item.diffs), [item.diffs]);
  const tailLines =
    running && kind === "execute" && item.output ? item.output.replace(/\n$/, "").split("\n").slice(-6) : [];
  const input = inputWithoutSummary(item.input);
  return (
    <div className="font-rounded">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          {running ? (
            <Spinner size={12} />
          ) : failed ? (
            <CircleX size={13} className="text-danger" />
          ) : (
            <Icon size={13} className="text-fg-muted" />
          )}
        </span>
        <span className="min-w-0 max-w-full shrink truncate font-medium text-callout" title={item.title}>
          {title}
        </span>
        {item.summary && (
          <span className="min-w-0 shrink truncate text-callout text-fg-muted">{item.summary}</span>
        )}
        {shownDetail && (
          <span
            className={cx(
              "min-w-0 max-w-[42ch] shrink truncate text-caption text-fg-faint",
              detail.mono && "font-mono",
            )}
            dir="auto"
          >
            {shownDetail}
          </span>
        )}
        {failed && item.exitCode === undefined && (
          <span
            className="min-w-0 max-w-[45%] shrink truncate text-caption text-danger"
            title={reason || undefined}
          >
            {reason || "Failed"}
          </span>
        )}
        <DiffCount {...counts} />
        {item.exitCode !== undefined && item.exitCode !== 0 && (
          <span className="font-mono text-caption text-danger">exit {item.exitCode}</span>
        )}
        <ChevronRight
          size={11}
          className={cx("shrink-0 text-fg-faint transition-transform duration-200", open && "rotate-90")}
        />
      </button>
      {tailLines.length > 0 && !open && (
        <pre className="m-0 mt-0.5 ml-[26px] overflow-hidden font-mono text-caption text-fg-muted leading-[15px] whitespace-pre-wrap">
          {tailLines.join("\n")}
        </pre>
      )}
      {open && (
        <div className="rc-expand mt-1.5 mb-1 ml-[26px] flex flex-col gap-2.5">
          {item.diffs.length > 0 ? (
            item.diffs.map((diff, i) => <DiffView key={i} diff={diff} />)
          ) : (
            <>
              {input && <TextBox label="Input" text={input} />}
              {item.output && <TextBox label="Output" text={item.output} />}
              {!input && !item.output && <span className="text-caption text-fg-faint">No output</span>}
            </>
          )}
        </div>
      )}
    </div>
  );
});

/**
 * Where the thread moved to another harness, as T3 Code draws its context
 * handoff: a quiet divider, `Claude Code → Codex`, with the model it took.
 */
export function HandoffRow({ item }: { item: Item<"handoff"> }) {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const name = (id: string) => harnesses?.find((h) => h.id === id)?.name ?? id;
  return (
    <div className="flex items-center gap-3 py-1 font-rounded text-caption text-fg-muted" role="separator">
      <span className="h-px flex-1 bg-border" />
      <span className="flex items-center gap-1.5">
        <HarnessIcon id={item.from} size={12} />
        {name(item.from)}
        <ArrowRight size={11} className="text-fg-faint" />
        <HarnessIcon id={item.to} size={12} />
        <span className="text-fg">{name(item.to)}</span>
        {item.model && <span className="text-fg-faint">· {item.model}</span>}
      </span>
      <Tooltip label="The new harness gets the conversation so far with your next message">
        <span className="text-fg-faint">context handed over</span>
      </Tooltip>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

export function NoticeRow({ item, action }: { item: Item<"notice">; action?: ReactNode }) {
  return (
    <Callout tone="warning" action={action}>
      {item.text}
    </Callout>
  );
}

/** What a finished turn changed and which checks it ran, as TurnCompletionCard.swift draws it. */
export function CompletionCard({ completion }: { completion: TurnCompletion }) {
  const [open, setOpen] = useState(false);
  const failed = completion.checks.filter((c) => c.outcome === "failed");
  const passed = completion.checks.length > 0 && completion.checks.every((c) => c.outcome === "passed");
  // The files are what RustCode's own tools edited. An external harness edits on its own, so an
  // empty list does not mean nothing changed (the turn's Changes chip says what did): say nothing.
  if (!completion.files.length && !completion.checks.length) return null;
  return (
    // A pill at the left, as wide as what it says; the files open under it.
    <div className="mb-2 font-rounded text-callout">
      <div className="inline-flex h-7 items-center gap-2 rounded-full bg-fill-subtle px-3">
        {completion.files.length ? (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="-mx-1 flex items-center gap-1.5 rounded-sm px-1 py-0.5 hover:bg-fill-subtle"
          >
            <span className="font-medium">
              {completion.files.length} file{completion.files.length === 1 ? "" : "s"} changed
            </span>
            <DiffCount added={completion.added} removed={completion.removed} />
            <ChevronRight
              size={11}
              className={cx("text-fg-faint transition-transform duration-200", open && "rotate-90")}
            />
          </button>
        ) : null}
        {completion.files.length > 0 && (failed.length > 0 || passed) && (
          <span aria-hidden className="text-fg-faint">
            ·
          </span>
        )}
        {failed.length > 0 ? (
          <Tooltip label={`Failed on its last run:\n${failed.map((c) => c.command).join("\n")}`}>
            <span className="flex items-center gap-1 font-medium text-caption text-danger">
              <CircleX size={12} />
              {failed.length === 1 ? "1 check failed" : `${failed.length} checks failed`}
            </span>
          </Tooltip>
        ) : passed ? (
          <Tooltip label={completion.checks.map((c) => c.command).join("\n")}>
            <span className="flex items-center gap-1 font-medium text-caption text-success">
              <CircleCheck size={12} /> Checks passed
            </span>
          </Tooltip>
        ) : null}
      </div>
      {open && (
        <div className="mt-1 rounded-lg bg-fill-subtle px-3 py-1.5">
          {completion.files.map((f) => (
            <div key={f.path} className="flex h-[22px] items-center gap-2 text-caption">
              <FileText size={11} className="shrink-0 text-fg-muted" />
              <span className="min-w-0 flex-1 truncate text-left font-mono" dir="rtl">
                <bdi>{f.path}</bdi>
              </span>
              <DiffCount added={f.added} removed={f.removed} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * A finished turn's work, folded (state/fold.ts): `Worked for 1m 20s · 14 steps
 * · 3 files changed`, a chevron, and the steps as they streamed under it when
 * opened. The final answer, errors and the footer stay outside the fold.
 */
export const TurnFoldRow = memo(function TurnFoldRow({
  fold,
  onToggle,
}: {
  fold: TurnFold;
  onToggle: (key: string) => void;
}) {
  return (
    <button
      type="button"
      aria-expanded={fold.open}
      onClick={() => onToggle(fold.key)}
      title={fold.open ? "Hide the work" : "Show the work"}
      className="-mx-1 flex max-w-full items-center gap-1.5 rounded-sm px-1 text-callout text-fg-faint hover:text-fg-muted"
    >
      <span className="truncate tabular">{foldLabel(fold, formatDuration)}</span>
      {fold.failed > 0 && <CircleX size={11} className="shrink-0 text-danger" aria-hidden />}
      <ChevronRight
        size={11}
        className={cx("shrink-0 transition-transform duration-200", fold.open && "rotate-90")}
      />
    </button>
  );
});

/**
 * A run of finished tool calls as one line, in a tool row's own grammar (an
 * icon in the 18px column, then the words): `Read 4 files · Ran 2 commands`.
 * Opening it lists the calls under it.
 */
export const ToolGroupRow = memo(function ToolGroupRow({
  group,
  onToggle,
}: {
  group: ToolGroup;
  onToggle: (key: string) => void;
}) {
  return (
    <div className="font-rounded">
      <button
        type="button"
        aria-expanded={group.open}
        onClick={() => onToggle(group.key)}
        title={group.open ? "Hide these steps" : `Show ${group.count} steps`}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          <Layers size={13} className="text-fg-muted" />
        </span>
        <span className="min-w-0 truncate text-callout text-fg-muted">{group.label}</span>
        {group.failed > 0 && (
          <span className="flex shrink-0 items-center gap-0.5 text-caption text-danger">
            <CircleX size={11} aria-hidden />
            {group.failed} failed
          </span>
        )}
        <ChevronRight
          size={11}
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200",
            group.open && "rotate-90",
          )}
        />
      </button>
    </div>
  );
});

export function TurnEndRow({
  item,
  reply,
  completion,
  changes,
  folded,
}: {
  item: Item<"turnEnd">;
  /** The turn's work is folded into a summary row that already says how long it took. */
  folded?: boolean;
  reply?: string;
  completion?: TurnCompletion | null;
  /** The turn's checkpoint chip (Changes), when the engine keeps checkpoints. */
  changes?: ReactNode;
}) {
  const parts = [
    item.outcome === "cancelled"
      ? `Stopped by you${item.elapsedMs ? ` · after ${formatDuration(item.elapsedMs)}` : ""}`
      : item.outcome === "failed"
        ? "The turn failed"
        : item.elapsedMs !== undefined && !folded
          ? `Worked for ${formatDuration(item.elapsedMs)}`
          : undefined,
    timeLabel(item.at),
  ].filter(Boolean);
  return (
    <div className="-mt-2">
      {completion && <CompletionCard completion={completion} />}
      <div className="flex h-[22px] items-center gap-1.5 text-caption text-fg-faint">
        {reply && <CopyButton text={reply} iconOnly label="Copy reply" />}
        {item.outcome === "cancelled" && <SquareStop size={11} />}
        <span className="tabular">{parts.join(" · ")}</span>
        {changes}
      </div>
    </div>
  );
}

/** The live line under a running turn: what it does now and for how long, with a shimmer. */
/**
 * The turn is done for now but parked on background commands: a slow pulse
 * and a plain line, quieter than Working. The turn resumes when they finish.
 */
export function WaitingRow({ count }: { count: number }) {
  return (
    <div className="flex items-center gap-2 text-callout text-fg-muted" role="status" aria-live="polite">
      <span className="relative flex size-2 items-center justify-center">
        <span className="absolute size-2 rounded-full bg-accent/40 rc-breathe" />
        <span className="size-1.5 rounded-full bg-accent" />
      </span>
      Waiting on {count === 1 ? "a background command" : `${count} background commands`}
      <span className="text-fg-faint">· continues when {count === 1 ? "it finishes" : "they finish"}</span>
    </div>
  );
}

export function WorkingRow({ label, startedAt, now }: { label: string; startedAt?: number; now: number }) {
  const seconds = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;
  const clock = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  return (
    <div className="flex items-center gap-1.5 text-callout" role="status" aria-live="polite">
      <span className="rc-shimmer truncate">{label}</span>
      <span className="text-fg-faint tabular">· {clock}</span>
    </div>
  );
}
