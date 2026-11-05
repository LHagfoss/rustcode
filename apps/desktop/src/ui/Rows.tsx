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
  CornerDownRight,
  ExternalLink,
  FileText,
  GitFork,
  Globe,
  Hourglass,
  Image as ImageIcon,
  Layers,
  MessageCircleQuestion,
  MessagesSquare,
  MoveRight,
  Pencil,
  RotateCcw,
  Search,
  Shield,
  SquareStop,
  Terminal,
  Trash2,
  Users,
  Wrench,
} from "lucide-react";
import { memo, type ReactNode, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { effortLabel, modelName } from "../acp/types";
import { clip } from "../lib/clip";
import { displayCommand } from "../lib/command";
import { observeSize } from "../lib/resize";
import { splitReply } from "../smartui/parse";
import { messageCollapses, splitImageMarkers, splitSlashCommand } from "../state/compose";
import { foldLabel, inferToolKind, type ToolGroup, type TurnFold } from "../state/fold";
import { stateOf } from "../state/host";
import { LINK_LABEL } from "../state/orchestration";
import { editMessage, forkThread, retryMessage, selectThread, useApp } from "../state/store";
import {
  diffCounts,
  formatDuration,
  messageTime,
  type TranscriptItem,
  type TurnCompletion,
  thoughtHeadings,
} from "../state/thread";
import { displayTool, orchestrationTool, sessionIdOf, usefulSummary } from "../state/toolNames";
import { type Waiting, waitingLabel, waitingTitle } from "../state/waiting";
import { DiffView } from "./DiffView";
import { HarnessIcon } from "./HarnessIcon";
import { openLightbox } from "./Lightbox";
import { LinkStateIcon } from "./Links";
import { COMMAND_MARK, CopyButton, Markdown } from "./Markdown";
import { Callout, cx, Pill, Spinner, Tooltip } from "./primitives";
import { replyActions, turnFooterHidden } from "./replyActions";
import { SmartUIView } from "./SmartUIView";
import { VizView } from "./Viz";

type Item<K extends TranscriptItem["kind"]> = Extract<TranscriptItem, { kind: K }>;

/** What a sent message reads as in Markdown: a leading command becomes the chip Markdown draws for it. */
function messageMarkdown(text: string): string {
  const slash = splitSlashCommand(text);
  return slash ? `\`${COMMAND_MARK}${slash.command}\` ${slash.rest}` : text;
}

export const UserRow = memo(function UserRow({
  item,
  threadId,
  rewind,
  before,
  after,
}: {
  item: Item<"user">;
  threadId?: string;
  /** Edit and Retry take the thread back to this message (`_rustcode/rewind`). */
  rewind?: boolean;
  /** The message after this one is the sender's too. */
  before?: boolean;
  /** The message before this one is the sender's too. */
  after?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const { text, images: markers } = useMemo(() => splitImageMarkers(item.text), [item.text]);
  const markdown = useMemo(() => messageMarkdown(text), [text]);
  // A turn the app woke the thread for (children finished, a child asks): a
  // divider row like a compaction, never a message that reads as the user's.
  const wakeLine = item.wake ? (item.text.split("\n")[0] ?? item.text) : null;
  const body = useRef<HTMLDivElement>(null);
  const [long, setLong] = useState(false);
  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const measure = () => setLong(messageCollapses(el.scrollHeight));
    measure();
    return observeSize(el, measure);
  }, []);
  const attachments = item.attachments ?? [];
  const placeholders = attachments.some((a) => a.kind === "image") ? 0 : markers;
  if (wakeLine) {
    return (
      <div title={item.text}>
        <NoticeRow flat item={{ kind: "notice", id: item.id, text: wakeLine, tone: "info" }} />
      </div>
    );
  }
  return (
    <div className="group flex flex-col items-end">
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
      <div
        className={cx(
          // A card a step off the pane, not a fill that moves with the text colour. Messages sent
          // one after another soften the corners they share, so they read as one run.
          "relative max-w-[80%] rounded-2xl bg-message p-3",
          // A message after this one continues the run: the bottom-right corner joins it.
          after && "rounded-br-md",
          before && "rounded-tr-md",
          !text && "hidden",
        )}
      >
        <div
          ref={body}
          className={cx(
            long &&
              !open &&
              "max-h-[176px] overflow-hidden [mask-image:linear-gradient(to_bottom,black_calc(100%-28px),transparent)]",
          )}
        >
          <Markdown
            text={markdown}
            className="text-fg [&_li]:whitespace-pre-wrap [&_p]:whitespace-pre-wrap"
          />
        </div>
        {long && (
          <Pill quiet className="mt-1.5 -mr-1 ml-auto" onClick={() => setOpen(!open)}>
            {open ? "Show less" : "Show full message"}
            {open ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
          </Pill>
        )}
      </div>
      {/* Time and actions show on hover, clipped to the bubble's width; `.rc-meta-reveal` is the
          same reveal the queued row's foot uses. */}
      <div className="rc-meta-reveal mt-0.5 flex h-[22px] max-w-[80%] min-w-0 items-center gap-1 overflow-hidden whitespace-nowrap text-sub text-fg-faint tabular">
        {item.steered && <span className="rc-keep text-accent">Steered</span>}
        {item.fromView && <span className="rc-keep text-accent">From view</span>}
        <Tooltip label={item.at ? new Date(item.at).toLocaleString() : undefined}>
          <span className="truncate tabular">{messageTime(item.at)}</span>
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

/**
 * A question the harness asked, and the answer given: kept in the transcript after it resolves,
 * so the thread says what was decided where the work went on. The engine's history has no
 * permission events, so this row is the app's own record (`state/thread.ts` `questionItem`).
 */
export const QuestionRow = memo(function QuestionRow({
  item,
}: {
  item: Extract<TranscriptItem, { kind: "question" }>;
}) {
  return (
    <div className="flex flex-col gap-1 rounded-lg bg-fill-subtle px-3 py-2">
      <div className="flex min-w-0 items-center gap-1.5 text-callout text-fg-muted">
        <MessageCircleQuestion size={13} className="shrink-0 text-accent" />
        <span className="min-w-0 truncate font-medium">{item.header || "Question"}</span>
      </div>
      <div className="selectable line-clamp-3 text-body text-fg-muted">{item.text}</div>
      <div className="flex min-w-0 items-start gap-1.5 text-body text-fg">
        <CornerDownRight size={13} className="mt-[3px] shrink-0 text-success" />
        <span className="min-w-0 break-words">{item.answer}</span>
      </div>
    </div>
  );
});

/** The fork action in a reply's row and in a finished turn's footer. */
function ReplyForkButton({ threadId }: { threadId: string }) {
  return (
    <Tooltip label="Fork the chat: a new thread gets this conversation">
      <button
        type="button"
        aria-label="Fork chat"
        onClick={() => void forkThread(threadId)}
        className="inline-flex size-[22px] items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
      >
        <GitFork size={12} />
      </button>
    </Tooltip>
  );
}

export const AssistantRow = memo(function AssistantRow({
  item,
  streaming = false,
  threadId,
  inTurnFoot = false,
}: {
  item: Item<"assistant">;
  streaming?: boolean;
  threadId?: string;
  /** The turn footer draws this reply's copy and fork: the row under it stays clean. */
  inTurnFoot?: boolean;
}) {
  const segments = useMemo(() => splitReply(item.text), [item.text]);
  const reveal = useApp((s) => s.animateStreaming);
  // Under the reply, on hover: copy, fork and the time, 12px, in that order.
  const row = replyActions({
    where: "reply",
    text: item.text,
    at: item.at,
    threadId,
    streaming,
    lastInTurn: inTurnFoot,
  });
  const actions = row.row ? (
    <div className="mt-0.5 flex h-[22px] items-center gap-1 text-sub text-fg-faint tabular opacity-0 transition-opacity focus-within:opacity-100 group-hover/reply:opacity-100 [@media(pointer:coarse)]:opacity-100">
      {row.copy && <CopyButton text={row.copy} iconOnly label="Copy this reply" />}
      {row.fork && <ReplyForkButton threadId={threadId!} />}
      {row.time && (
        <Tooltip label={new Date(item.at!).toLocaleString()}>
          <span className="truncate">{messageTime(item.at)}</span>
        </Tooltip>
      )}
    </div>
  ) : null;
  if (segments.length === 1 && segments[0]!.kind === "markdown")
    return (
      <div className="group/reply">
        <Markdown
          text={item.text}
          streaming={streaming}
          reveal={reveal}
          revealKey={item.id}
          // `.rc-stream` fades each block in. With the per-word reveal on, that would fade a block
          // again as it stops being the last one, so the reveal covers it instead.
          className={streaming && !reveal ? "rc-stream" : undefined}
        />
        {actions}
      </div>
    );
  return (
    <div className="group/reply flex flex-col gap-2.5">
      {segments.map((segment, i) =>
        segment.kind === "markdown" ? (
          <Markdown
            key={i}
            text={segment.text}
            streaming={streaming && i === segments.length - 1}
            reveal={reveal}
            revealKey={`${item.id}:${i}`}
          />
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
      {actions}
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

export const KIND_ICON: Record<string, typeof Wrench> = {
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
  // RustCode's orchestration calls: threads started, messaged, read or listed, and waited on.
  start: MessagesSquare,
  message: MessagesSquare,
  read_thread: MessagesSquare,
  list_thread: MessagesSquare,
  wait: Hourglass,
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
  // Folds only when that hides a few lines, not one.
  const folds = lines.length > 17;
  const shown = all || !folds ? lines : lines.slice(0, 14);
  return (
    <div>
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="font-semibold text-caption text-fg-muted">{label}</span>
        {text.trim() && <CopyButton text={text} iconOnly label={`Copy ${label.toLowerCase()}`} />}
      </div>
      <pre className="selectable m-0 max-h-[420px] overflow-auto rounded-md bg-fill-subtle p-2.5 font-mono text-caption leading-[15px] whitespace-pre-wrap break-all">
        {shown.join("\n")}
      </pre>
      {folds && (
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
  const kind = inferToolKind(item);
  const [open, setOpen] = useState(kind === "edit" && item.diffs.length > 0 && item.diffs.length <= 2);
  const Icon = KIND_ICON[kind] ?? Wrench;
  const running = item.status === "pending" || item.status === "in_progress";
  const failed = item.status === "failed";
  const detail = toolDetail(item, kind);
  // MCP calls come named `mcp__server__tool`: the row says what they did instead.
  const label = displayTool(item.title, item.input, item.status);
  const title = displayCommand(label.title);
  // The line that tells two calls apart; a lone fence marker is not one.
  const summary = usefulSummary(item.summary) ?? label.summary;
  const startedId = orchestrationTool(item.title) === "start_session" ? sessionIdOf(item.output) : undefined;
  const link = useApp((s) => (startedId ? s.links.links.find((l) => l.target === startedId) : undefined));
  const linkState = useApp((s) => (link ? stateOf(s, link) : undefined));
  const opensThread = !!startedId && !!link;
  const relative =
    detail.text && cwd && detail.text.startsWith(`${cwd}/`) ? detail.text.slice(cwd.length + 1) : detail.text;
  const trimmed = relative && displayCommand(relative);
  // A command whose title already is the command line shows it once.
  const shownDetail = trimmed && !title.includes(trimmed) ? trimmed : undefined;
  // Why it failed, on the row itself: the first line of what it printed, when no exit code says it.
  const reason =
    failed && item.exitCode === undefined ? clip(item.output?.trim().split("\n")[0] ?? "", 120) : "";
  const counts = useMemo(() => diffCounts(item.diffs), [item.diffs]);
  const tailLines =
    running && kind === "execute" && item.output ? item.output.replace(/\n$/, "").split("\n").slice(-6) : [];
  const input = inputWithoutSummary(item.input);
  return (
    <div>
      <div className="group flex items-center gap-0.5">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="-mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left leading-relaxed hover:bg-fill-subtle"
        >
          <span className="flex w-[18px] shrink-0 justify-center">
            {running ? (
              <Spinner size={14} />
            ) : failed ? (
              <CircleX size={16} className="text-danger" />
            ) : (
              <Icon size={16} className="text-fg-muted" />
            )}
          </span>
          <span className="min-w-0 max-w-full shrink truncate text-body text-fg-muted" title={item.title}>
            {title}
          </span>
          {summary && <span className="min-w-0 shrink truncate text-body text-fg-faint">{summary}</span>}
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
          {item.autoAllowed && (
            <Tooltip label={`Allowed by ${item.autoAllowed}`}>
              <span className="flex shrink-0 items-center text-fg-faint">
                <Shield size={11} />
              </span>
            </Tooltip>
          )}
          {linkState && (
            <Tooltip label={`“${link?.targetTitle}” · ${LINK_LABEL[linkState]}`}>
              <span className="flex shrink-0 items-center">
                <LinkStateIcon state={linkState} size={11} />
              </span>
            </Tooltip>
          )}
          <ChevronRight
            size={11}
            className={cx(
              "shrink-0 text-fg-faint transition-[transform,opacity] duration-200",
              open ? "rotate-90" : "opacity-0 group-hover:opacity-100",
            )}
          />
        </button>
        {startedId && opensThread && (
          <Tooltip label="Open thread">
            <button
              type="button"
              aria-label="Open thread"
              onClick={() => selectThread(startedId)}
              className="flex size-5 shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg"
            >
              <ExternalLink size={11} />
            </button>
          </Tooltip>
        )}
      </div>
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

/** Where the thread's model or effort changed: a quiet divider, `gpt-6-luna → gpt-6 · Low → High`. */
export function SwitchRow({ item }: { item: Item<"switch"> }) {
  const { modelFrom, modelTo, effortFrom, effortTo } = item.change;
  const models = modelFrom !== modelTo && modelTo;
  const efforts = effortFrom !== effortTo && effortTo;
  return (
    <div className="flex items-center gap-3 py-1 text-caption text-fg-muted" role="separator">
      <span className="h-px flex-1 bg-border" />
      <span className="flex items-center gap-1.5">
        {models && (
          <>
            {modelFrom && (
              <>
                {modelName(modelFrom)}
                <ArrowRight size={11} className="text-fg-faint" />
              </>
            )}
            <span className="text-fg">{modelName(modelTo)}</span>
          </>
        )}
        {models && efforts && <span className="text-fg-faint">·</span>}
        {efforts && (
          <>
            {effortFrom && (
              <>
                {effortLabel(effortFrom)}
                <ArrowRight size={11} className="text-fg-faint" />
              </>
            )}
            <span className="text-fg">{effortLabel(effortTo)}</span>
          </>
        )}
      </span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

/**
 * Where the thread moved to another harness: a quiet divider, `Claude Code → Codex`, with the model it took.
 */
export function HandoffRow({ item }: { item: Item<"handoff"> }) {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const name = (id: string) => harnesses?.find((h) => h.id === id)?.name ?? id;
  return (
    <div className="flex items-center gap-3 py-1 text-caption text-fg-muted" role="separator">
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

export function NoticeRow({
  item,
  action,
  flat,
}: {
  item: Item<"notice">;
  action?: ReactNode;
  /** A wake divider sits in the transcript's own rhythm, without the padding a card needs. */
  flat?: boolean;
}) {
  // Something that happened to the thread (it compacted), not a problem: a divider, like a switch.
  if (item.tone === "info")
    return (
      <div
        className={cx("flex items-center gap-3 text-caption text-fg-muted", flat ? "py-0" : "py-1")}
        role="separator"
      >
        <span className="h-px flex-1 bg-border" />
        <span className="flex min-w-0 items-center gap-1.5">
          <Layers size={11} className="shrink-0 text-fg-faint" />
          <span className="truncate">{item.text}</span>
        </span>
        <span className="h-px flex-1 bg-border" />
      </div>
    );
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
    <div className="mb-2 text-callout">
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
 * A finished turn's work, folded (state/fold.ts):
 * `Worked for 1m 3s ›` over a hairline, and the steps as they streamed under
 * it when opened. The final answer, errors and the footer stay outside it.
 * A turn that wrote no answer takes the time here (`Worked for 4s · 09:55 PM`)
 * and keeps no footer: there is nothing under it to copy or fork.
 */
export const TurnFoldRow = memo(function TurnFoldRow({
  fold,
  onToggle,
  rule = true,
}: {
  fold: TurnFold;
  onToggle: (key: string) => void;
  /** The hairline under it: dropped when the row above is already a divider. */
  rule?: boolean;
}) {
  return (
    <button
      type="button"
      aria-expanded={fold.open}
      onClick={() => onToggle(fold.key)}
      title={
        fold.open ? "Hide the work" : `Show the work: ${fold.steps} ${fold.steps === 1 ? "step" : "steps"}`
      }
      className={cx(
        "group flex w-full items-center gap-1.5 pt-1 pb-2 text-left text-body text-fg-muted hover:text-fg",
        rule && "border-border/60 border-b",
      )}
    >
      {fold.stopped && <SquareStop size={12} className="shrink-0" aria-hidden />}
      <span className="truncate tabular">{foldLabel(fold, formatDuration)}</span>
      {!fold.answered && fold.time !== undefined && (
        <span className="shrink-0 text-caption text-fg-faint tabular">· {messageTime(fold.time)}</span>
      )}
      {fold.failedTurn && <CircleX size={12} className="shrink-0 text-danger" aria-label="The turn failed" />}
      <ChevronRight
        size={12}
        className={cx("shrink-0 text-fg-faint transition-transform duration-200", fold.open && "rotate-90")}
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
    // Two or more finished calls read as one step of the turn, closed by a hairline so the run
    // below it is told apart from the answer that follows.
    <div className="border-border/60 border-b pt-1 pb-2">
      <button
        type="button"
        aria-expanded={group.open}
        onClick={() => onToggle(group.key)}
        title={group.open ? "Hide these steps" : `Show ${group.count} steps`}
        data-kind={group.kind}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left leading-relaxed hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          {(() => {
            const Icon = (group.kind && KIND_ICON[group.kind]) || Layers;
            return <Icon size={16} className="text-fg-muted" />;
          })()}
        </span>
        <span className="min-w-0 truncate text-body text-fg-muted">{group.label}</span>
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
  timeInFold,
  threadId,
  lastTurn,
}: {
  item: Item<"turnEnd">;
  /** The turn's work is folded into a summary row that already says how long it took. */
  folded?: boolean;
  /** The summary row above carries the time too: the turn wrote no answer. */
  timeInFold?: boolean;
  /** The turn's last reply, whose copy and fork show here rather than under the reply. */
  reply?: string;
  completion?: TurnCompletion | null;
  /** The turn's checkpoint chip (Changes), when the engine keeps checkpoints. */
  changes?: ReactNode;
  /** The thread, so the footer's fork can copy its conversation. */
  threadId?: string;
  /** The newest finished turn: its actions stay in view, the older ones wait for a hover. */
  lastTurn?: boolean;
}) {
  // One row for the turn: its outcome and time, then copy and fork, then the changes chip.
  const row = replyActions({ where: "turnEnd", text: reply, at: item.at, threadId });
  // An empty turn that folded its work: its header already says the time, and there is no reply
  // to copy or fork, so a footer row of icons would sit over nothing.
  if (
    turnFooterHidden({ folded, reply, completion: !!completion, changes: !!changes, outcome: item.outcome })
  )
    return null;
  // The outcome, in words, and the time. How long the turn took is the fold header's to say.
  const parts = [
    item.outcome === "cancelled"
      ? `Stopped by you${item.elapsedMs ? ` · after ${formatDuration(item.elapsedMs)}` : ""}`
      : item.outcome === "failed"
        ? "The turn failed"
        : undefined,
    ...(timeInFold ? [] : [messageTime(item.at)]),
  ].filter(Boolean);
  const actions = (
    <>
      {row.copy && <CopyButton text={row.copy} iconOnly label="Copy reply" />}
      {row.fork && <ReplyForkButton threadId={threadId!} />}
    </>
  );
  return (
    <div className="-mt-2">
      {completion && <CompletionCard completion={completion} />}
      <div className="group/turn flex h-[22px] items-center gap-1.5 text-caption text-fg-faint">
        {item.outcome === "cancelled" && !folded && <SquareStop size={11} />}
        {parts.length > 0 && <span className="tabular">{parts.join(" · ")}</span>}
        {/* Time first, so the actions appearing on a hover never shift it. */}
        {(row.copy || row.fork) &&
          (lastTurn ? (
            actions
          ) : (
            <span className="flex items-center gap-1.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/turn:opacity-100 [@media(pointer:coarse)]:opacity-100">
              {actions}
            </span>
          ))}
        {changes}
      </div>
    </div>
  );
}

/** The live line under a running turn: what it does now and for how long, with a shimmer. */
/**
 * The foot of a quiet thread that waits on background work: the threads it
 * started or messaged that still run, or the commands its last turn parked on.
 * Not a turn (the composer stays idle), and it goes when they finish.
 */
export function WaitingRow({ wait }: { wait: Waiting }) {
  return (
    <Tooltip label={waitingTitle(wait)}>
      <div className="flex items-center gap-2 text-callout text-fg-muted" role="status" aria-live="polite">
        <span className="relative flex size-2 items-center justify-center">
          <span className="absolute size-2 rounded-full bg-accent/40 rc-breathe" />
          <span className="size-1.5 rounded-full bg-accent" />
        </span>
        {waitingLabel(wait)}
      </div>
    </Tooltip>
  );
}

/**
 * The running turn's header, right under the message that started it:
 * `Working for 12s` over a hairline. What the turn does now
 * shows at the foot ({@link LiveActivityRow}).
 */
export function WorkingRow({ startedAt, now }: { startedAt?: number; now: number }) {
  const elapsed = startedAt ? Math.max(0, now - startedAt) : undefined;
  return (
    <div
      className="flex h-[30px] items-center border-border/60 border-b pt-1 pb-2 text-body text-fg-muted tabular"
      role="status"
    >
      {elapsed !== undefined
        ? `Working for ${formatDuration(Math.floor(elapsed / 1000) * 1000)}`
        : "Working…"}
    </div>
  );
}

/** The running turn's foot while nothing else shows it working: what it does now, shimmering. */
export function LiveActivityRow({ label }: { label: string }) {
  return (
    <div className="flex min-h-6 items-center gap-2 text-body" role="status" aria-live="polite">
      <span className="flex w-[18px] shrink-0 justify-center">
        <Spinner size={14} />
      </span>
      <span className="rc-shimmer truncate">{label}</span>
    </div>
  );
}

// The diff row lives in its own file now (ui/DiffView.tsx); this keeps its importers.
export { DiffView };
