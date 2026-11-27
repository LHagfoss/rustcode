// Transcript rows, after TranscriptRows.swift: user bubbles on the right,
// replies full width without a bubble, quiet thought disclosures, one-line
// tool rows that open to their diff or output, and a turn footer.

import { Fragment, memo, type ReactNode, useLayoutEffect, useMemo, useRef, useState } from "react";
import { effortLabel, type HarnessDenial, modelName } from "../acp/types";
import { type AnsiLine, ansiClass } from "../lib/ansi";
import { displayCommand } from "../lib/command";
import { openPath } from "../lib/native";
import { observeSize } from "../lib/resize";
import { type ArgRow, type ToolBody, toolBody } from "../lib/toolBody";
import { hasTimeline } from "../lib/trace";
import { turnFlavor } from "../lib/workingMotion";
import { splitReply } from "../smartui/parse";
import { messageCollapses, splitImageMarkers, splitSlashCommand } from "../state/compose";
import { foldLabel, inferToolKind, type ToolGroup, type TurnFold } from "../state/fold";
import { stateOf } from "../state/host";
import { LINK_LABEL, parseDelivered } from "../state/orchestration";
import { harnessDenialText } from "../state/permissions";
import { type PullRequestEvent, pullRequestEvent } from "../state/pullRequestEvent";
import {
  editMessage,
  forkThread,
  openSettings,
  openSideChat,
  retryMessage,
  selectThread,
  sendSideChatToParent,
  useApp,
} from "../state/store";
import {
  diffCounts,
  formatDuration,
  isRunning,
  messageTime,
  noticeLine,
  stripMarks,
  type TranscriptItem,
  type TurnCompletion,
  thoughtPreview,
  toolInFlight,
  wakeLine,
} from "../state/thread";
import { isRemoteThread } from "../state/threadKeys";
import { failureReason, orchestrationTool, printedExitCode, runIdOf, sessionIdOf } from "../state/toolNames";
import { formatToolRow } from "../state/toolRowFormat";
import { type Waiting, waitingLabel, waitingNext, waitingTitle } from "../state/waiting";
import { harnessId, wakeNotice } from "../state/wakeNotice";
import { DiffView } from "./DiffView";
import { HarnessIcon } from "./HarnessIcon";
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
  GitMerge,
  Globe,
  Hourglass,
  Image as ImageIcon,
  Layers,
  MessageCircleQuestion,
  MessageSquare,
  MessageSquareQuote,
  MessagesSquare,
  MoveRight,
  Paperclip,
  Pencil,
  RefreshCw,
  Search,
  Shield,
  SquareStop,
  Terminal,
  Trash2,
  Users,
  Wrench,
} from "./icons";
import { LinkStateIcon } from "./Links";
import { COMMAND_MARK, CopyButton, Markdown } from "./Markdown";
import { McpAppSlot } from "./McpAppView";
import { ImageUnavailable, MessageGallery, useStoredImages } from "./MessageGallery";
import { PasteChip } from "./PasteChip";
import { Callout, cx, Pill, Spinner, Tooltip } from "./primitives";
import { turnActions, turnFooterHidden } from "./replyActions";
import { SmartUIView } from "./SmartUIView";
import { VizView } from "./Viz";
import { WorkflowCard } from "./WorkflowRuns";
import { DotMatrix, WaveText, WaveWord } from "./WorkingIndicator";

type Item<K extends TranscriptItem["kind"]> = Extract<TranscriptItem, { kind: K }>;

/** What a sent message reads as in Markdown: a leading command becomes the chip Markdown draws for it. */
function messageMarkdown(text: string): string {
  const slash = splitSlashCommand(text);
  return slash ? `\`${COMMAND_MARK}${slash.command}\` ${slash.rest}` : text;
}

/** An image a message recorded whose file is not on the thread's computer (any more). */
/**
 * A message's Edit and Retry: shown on hover with Copy, and dimmed rather than removed while a
 * turn runs (`aria-disabled` keeps the tooltip that says why).
 */
const MESSAGE_ACTION =
  "inline-flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg aria-disabled:cursor-default aria-disabled:text-fg-faint aria-disabled:hover:bg-transparent aria-disabled:hover:text-fg-faint";

export const UserRow = memo(function UserRow({
  item,
  threadId,
  busy,
  rewind,
  before,
  after,
  newDay,
  stacked,
}: {
  item: Item<"user">;
  threadId?: string;
  /**
   * A turn runs: Edit and Retry keep their place but are off until it ends, so the message's
   * actions are the same three whether the thread runs or not.
   */
  busy?: boolean;
  /** Edit and Retry take the thread back to this message (`_rustcode/rewind`). */
  rewind?: boolean;
  /** The message after this one is the sender's too. */
  before?: boolean;
  /** The message before this one is the sender's too. */
  after?: boolean;
  /** The first message of its day (`newDayMessages`): its time stays in view, with the day. */
  newDay?: boolean;
  /** The row above is a wake divider too: this one joins its list instead of drawing its own rules. */
  stacked?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const { text, paths, lost } = useMemo(() => splitImageMarkers(stripMarks(item.text)), [item.text]);
  // A message another thread sent with its id: the heading becomes a link to that thread, and the
  // ids under it are not shown. One from before the ids reads as it was written.
  const sender = useMemo(() => {
    const parsed = parseDelivered(text);
    return parsed?.session ? { ...parsed, session: parsed.session } : null;
  }, [text]);
  const markdown = useMemo(() => messageMarkdown(sender ? sender.body : text), [sender, text]);
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
  // A message sent from here shows the pictures it carried; a replayed one reads its files back
  // from the computer the thread runs on, and an image whose file is gone says so.
  const sentHere = attachments.some((a) => a.kind === "image");
  const stored = sentHere ? [] : paths;
  const gone = sentHere ? 0 : lost;
  const sentPictures = useMemo(
    () =>
      (item.attachments ?? []).flatMap((a) =>
        a.kind === "image" && a.preview ? [{ src: a.preview, name: a.name }] : [],
      ),
    [item.attachments],
  );
  const storedPictures = useStoredImages(threadId ?? "", stored);
  // A turn the app woke the thread for (a delegate finished, or asks): a
  // divider row like a compaction, never a message that reads as the user's.
  if (item.wake) return <WakeRow text={item.text} flat stacked={stacked} />;
  // The app carrying the thread on after it compacted it: the same kind of divider.
  if (item.continued) {
    return <NoticeRow item={{ kind: "notice", id: item.id, text: noticeLine(item.text), tone: "info" }} />;
  }
  return (
    <div className="group ml-auto flex w-fit max-w-[70%] flex-col items-end">
      {(attachments.length > 0 || stored.length > 0 || gone > 0) && (
        <div className="mb-1.5 flex max-w-full flex-wrap justify-end gap-1.5">
          {attachments.map((a, i) =>
            // A message sent before a paste went into the text keeps the chip it was written as: the
            // panel opens the text itself. New messages carry the paste as their own text.
            a.kind === "text" ? (
              <PasteChip key={i} text={a.text ?? ""} />
            ) : a.kind === "image" && a.preview ? null : (
              <span
                key={i}
                className="flex h-[26px] items-center gap-1.5 self-center rounded-full bg-fill-subtle px-2 text-callout"
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
          <MessageGallery pictures={sentHere ? sentPictures : storedPictures} />
          {Array.from({ length: gone }, (_, i) => (
            <ImageUnavailable key={`g${i}`} />
          ))}
        </div>
      )}
      <div
        className={cx(
          // A card a step off the pane, not a fill that moves with the text colour. Messages sent
          // one after another soften the corners they share, so they read as one run.
          "relative max-w-full selectable rounded-bubble bg-message px-4 py-2 leading-chat",
          // A message after this one continues the run: the bottom-right corner joins it.
          after && "rounded-br-md",
          before && "rounded-tr-md",
          !text && "hidden",
        )}
      >
        {sender && (
          <div className="mb-1 text-callout text-fg-muted">
            From thread{" "}
            <Tooltip label="Open thread">
              <button
                type="button"
                onClick={() => selectThread(sender.session)}
                className="rounded-sm text-fg underline decoration-fg-faint underline-offset-2 hover:decoration-fg"
              >
                {sender.title || "untitled"}
              </button>
            </Tooltip>{" "}
            (agent)
          </div>
        )}
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
            className="text-fg [&>:is(h1,h2,h3,h4):first-child]:mt-0 [&_li]:whitespace-pre-wrap [&_p]:whitespace-pre-wrap"
          />
        </div>
        {long && (
          <Pill quiet className="mt-1.5 -mr-1 ml-auto" onClick={() => setOpen(!open)}>
            {open ? "Show less" : "Show full message"}
            {open ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
          </Pill>
        )}
      </div>
      <div className="rc-meta-reveal mt-0.5 flex h-7 max-w-full min-w-0 select-none items-center gap-1 whitespace-nowrap text-sub text-fg-faint tabular">
        <div className="pointer-events-none inline-flex shrink-0 gap-0.5 opacity-0 transition-opacity focus-within:pointer-events-auto focus-within:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100 pointer-coarse:pointer-events-auto pointer-coarse:opacity-100">
          <CopyButton text={item.text} iconOnly size="md" label="Copy message" />
          {threadId && (
            <>
              <Tooltip
                label={
                  busy
                    ? "Stop the turn to edit"
                    : rewind
                      ? "Edit and resend: the thread goes back to this message"
                      : "Edit in the composer"
                }
              >
                <button
                  type="button"
                  aria-label="Edit message"
                  aria-disabled={busy || undefined}
                  onClick={() => !busy && editMessage(threadId, item.id)}
                  className={MESSAGE_ACTION}
                >
                  <Pencil size={14} />
                </button>
              </Tooltip>
              <Tooltip
                label={
                  busy
                    ? "Stop the turn to retry"
                    : rewind
                      ? "Retry: send it again from here"
                      : "Send it again"
                }
              >
                <button
                  type="button"
                  aria-label="Retry message"
                  aria-disabled={busy || undefined}
                  onClick={() => !busy && retryMessage(threadId, item.id)}
                  className={MESSAGE_ACTION}
                >
                  <RefreshCw size={14} />
                </button>
              </Tooltip>
            </>
          )}
        </div>
        {item.steered && (
          <span className={cx("rc-keep truncate", !item.steering && "text-accent")}>
            {item.steering ? "Steering…" : "Steered"}
          </span>
        )}
        {item.fromView && <span className="rc-keep truncate text-accent">From view</span>}
        <Tooltip label={item.at ? new Date(item.at).toLocaleString() : undefined}>
          <span className={cx("truncate tabular", newDay && "rc-keep")}>
            {/* `messageTime` names the day from yesterday on; a day that starts today says so too. */}
            {newDay && item.at && new Date(item.at).toDateString() === new Date().toDateString()
              ? `today at ${messageTime(item.at)}`
              : messageTime(item.at)}
          </span>
        </Tooltip>
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
        <span className="selectable min-w-0 break-words">
          {item.answer || (item.files?.length ? "Sent files" : "")}
        </span>
      </div>
      {item.files && item.files.length > 0 && (
        <ul aria-label="Attached files" className="m-0 ml-[19px] flex list-none flex-wrap gap-1 p-0">
          {item.files.map((name, i) => (
            <li
              key={`${i}:${name}`}
              className="flex h-[22px] max-w-[240px] items-center gap-1 rounded-full bg-fill px-2 text-caption text-fg-muted"
            >
              <Paperclip size={12} className="shrink-0" aria-hidden />
              <span className="selectable truncate">{name}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
});

/** The fork action of a finished turn's one action row. */
function ReplyForkButton({ threadId }: { threadId: string }) {
  return (
    <Tooltip label="Fork the chat: a new thread gets this conversation">
      <button
        type="button"
        aria-label="Fork chat"
        onClick={() => void forkThread(threadId)}
        className="inline-flex size-6 items-center justify-center rounded-chip text-fg-faint hover:bg-fill hover:text-fg"
      >
        <GitFork size={12} />
      </button>
    </Tooltip>
  );
}

/** "Ask in a side chat" on a finished turn: a thread beside this one, seeded with the reply. */
function ReplySideChatButton({ threadId, reply }: { threadId: string; reply: string }) {
  // Only an engine that records the relationship can open one (`AgentInfo.lineage`).
  const supported = useApp((s) => !!s.agent?.lineage);
  if (!supported) return null;
  return (
    <Tooltip label="Ask in a side chat: a thread beside this one, with this reply quoted">
      <button
        type="button"
        aria-label="Ask in a side chat"
        onClick={() => void openSideChat(threadId, reply)}
        className="inline-flex size-6 items-center justify-center rounded-chip text-fg-faint hover:bg-fill hover:text-fg"
      >
        <MessageSquareQuote size={12} />
      </button>
    </Tooltip>
  );
}

const INTERIM_NOTE = "text-callout! leading-chat-sub! text-fg-muted!";

export const AssistantRow = memo(function AssistantRow({
  item,
  streaming = false,
  threadId,
  interim = false,
}: {
  item: Item<"assistant">;
  streaming?: boolean;
  threadId?: string;
  /** A note written between steps of the work, not the answer: it reads one step quieter. */
  interim?: boolean;
}) {
  const segments = useMemo(() => splitReply(item.text, !streaming), [item.text, streaming]);
  const reveal = useApp((s) => s.animateStreaming);
  // A reply draws no actions of its own: a finished turn keeps one row for the
  // turn, at the foot of its last reply (`TurnEndRow`), and nothing else has any.
  if (segments.length === 1 && segments[0]!.kind === "markdown")
    return (
      <div className="selectable" data-quote-source>
        <Markdown
          text={item.text}
          streaming={streaming}
          reveal={reveal}
          revealKey={item.id}
          // `.rc-stream` fades each block in. With the per-word reveal on, that would fade a block
          // again as it stops being the last one, so the reveal covers it instead.
          className={cx(streaming && !reveal && "rc-stream", interim && INTERIM_NOTE) || undefined}
        />
      </div>
    );
  // A view stands apart from the prose around it: 20px between segments, where 8px ran
  // the sentence after a view into its last row.
  return (
    <div className="flex flex-col gap-5 selectable" data-quote-source>
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
  const preview = thoughtPreview(item.text);
  return (
    <div className="text-callout">
      <div className={cx("group flex max-w-full items-center gap-1", open && "rc-sticky-head")}>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="-mx-0.5 flex min-h-6 min-w-0 max-w-[calc(100%+4px)] items-center gap-1.5 rounded-chip px-0.5 text-left leading-chat text-fg-faint hover:bg-fill-subtle hover:text-fg-muted"
        >
          <span className="flex w-[18px] shrink-0 justify-center">
            {live ? (
              <DotMatrix pattern="ripple" size={14} tone="spectrum" />
            ) : (
              <Brain size={14} className="text-fg-faint" />
            )}
          </span>
          {live ? <WaveWord word="Thinking" /> : <span className="shrink-0">{label}</span>}
          {!open && preview && (
            <>
              <span aria-hidden className="size-0.5 shrink-0 rounded-full bg-fg-faint" />
              <span className="min-w-0 truncate text-fg-faint">{preview}</span>
            </>
          )}
          <ChevronRight
            size={12}
            className={cx(
              "shrink-0 transition-[transform,opacity] duration-200",
              open ? "rotate-90" : "opacity-60 group-hover:opacity-100",
            )}
          />
        </button>
        {open && item.text.trim() && <CopyButton text={item.text} iconOnly label="Copy thinking" />}
      </div>
      {open && (
        <div className="ml-[26px] pt-1.5 text-fg-muted">
          <Markdown text={item.text} streaming={live} className="gap-1.5 text-callout" />
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

function TextBox({
  text,
  label,
  shown: display,
  styled,
}: {
  text: string;
  label: string;
  shown?: string;
  /** The text's lines with their terminal colours (lib/ansi.ts), drawn instead of `text`. */
  styled?: AnsiLine[];
}) {
  const [all, setAll] = useState(false);
  // `text` is what Copy copies; `display` is how it reads, where the two differ (a command's `~`).
  const lines = styled ?? (display ?? text).replace(/\n$/, "").split("\n");
  // Folds only when that hides a few lines, not one.
  const folds = lines.length > 17;
  const shown = all || !folds ? lines : lines.slice(0, 14);
  return (
    <div>
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="font-semibold text-caption text-fg-muted">{label}</span>
        {text.trim() && <CopyButton text={text} iconOnly label={`Copy ${label.toLowerCase()}`} />}
      </div>
      <pre className="selectable m-0 max-h-[420px] overflow-auto rounded-block bg-code-bg px-4 py-3 font-mono text-caption leading-[16px] whitespace-pre-wrap break-all">
        {styled ? <StyledLines lines={shown as AnsiLine[]} /> : (shown as string[]).join("\n")}
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

/** Output lines in their terminal colours, as theme tokens. */
function StyledLines({ lines }: { lines: AnsiLine[] }) {
  return lines.map((line, i) => (
    // biome-ignore lint/suspicious/noArrayIndexKey: lines have no identity but their place
    <Fragment key={i}>
      {i > 0 && "\n"}
      {line.map((span, j) => {
        const className = ansiClass(span.style);
        // biome-ignore lint/suspicious/noArrayIndexKey: spans have no identity but their place
        return className ? (
          <span key={j} className={className}>
            {span.text}
          </span>
        ) : (
          <Fragment key={j}>{span.text}</Fragment>
        );
      })}
    </Fragment>
  ));
}

/** A call's arguments as key/value rows: scalars beside their key, long text and objects under it. */
function ArgRows({ rows }: { rows: ArgRow[] }) {
  if (!rows.length) return null;
  return (
    <div>
      <span className="mb-1 block font-semibold text-caption text-fg-muted">Arguments</span>
      <dl className="m-0 flex flex-col gap-1 rounded-block bg-code-bg px-4 py-3 text-caption">
        {rows.map((row) => (
          <div
            key={row.key}
            className={cx(
              "group/arg flex min-w-0 gap-x-2",
              row.shape === "inline" ? "items-baseline" : "flex-col",
            )}
          >
            <div className="flex min-w-0 shrink-0 items-center gap-1">
              <dt className="font-mono text-fg-muted">{row.key}</dt>
              {row.shape !== "inline" && (
                <span className="opacity-0 group-hover/arg:opacity-100 group-focus-within/arg:opacity-100">
                  <CopyButton text={row.value} iconOnly label={`Copy ${row.key}`} />
                </span>
              )}
            </div>
            <dd
              className={cx(
                "selectable m-0 min-w-0 font-mono text-fg leading-[15px] break-words",
                row.shape === "json" && "max-h-[240px] overflow-auto whitespace-pre pl-2",
                row.shape === "text" && "max-h-[240px] overflow-auto whitespace-pre-wrap",
              )}
              dir="auto"
            >
              {row.value}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

/** A shell call: the command line, what it printed, and how it exited. */
function CommandBody({ body }: { body: Extract<ToolBody, { kind: "command" }> }) {
  return (
    <>
      {body.command && <TextBox label="Command" text={body.command} shown={displayCommand(body.command)} />}
      <ArgRows rows={body.args} />
      {body.output && <TextBox label="Output" text={body.output} styled={body.styled} />}
      {body.exitCode !== undefined && (
        <span className={cx("font-mono text-caption", body.exitCode === 0 ? "text-fg-faint" : "text-danger")}>
          Exit code {body.exitCode}
        </span>
      )}
      {!body.command && !body.output && body.exitCode === undefined && (
        <span className="text-caption text-fg-faint">No output</span>
      )}
    </>
  );
}

/**
 * A tool call's detail, by what the call did (lib/toolBody.ts): its diffs, a
 * command with its output and exit code, a search or read with what it found,
 * or any other call's arguments as key/value rows. Shared by the tool row's
 * expander and a turn's Trace, so both format a call the same way and both
 * offer Copy.
 */
export function ToolDetail({
  item,
  className,
}: {
  item: Item<"tool">;
  /** Padding around the detail, for a host that draws its own indent. */
  className?: string;
}) {
  const body = useMemo(() => toolBody(item, inferToolKind(item)), [item]);
  return (
    <div className={cx("flex flex-col gap-2", className)}>
      {body.kind === "diff" ? (
        item.diffs.map((diff, i) => <DiffView key={i} diff={diff} />)
      ) : body.kind === "command" ? (
        <CommandBody body={body} />
      ) : (
        <>
          <ArgRows rows={body.args} />
          {body.output && (
            <TextBox label={body.kind === "lookup" ? body.outputLabel : "Output"} text={body.output} />
          )}
          {!body.args.length && !body.output && <span className="text-caption text-fg-faint">No output</span>}
        </>
      )}
      {item.fullOutput && <FullOutput path={item.fullOutput} />}
    </div>
  );
}

/** The harness showed a preview and saved the whole output to a file (Antigravity). */
function FullOutput({ path }: { path: string }) {
  // A paired machine's file is not on this computer to open or reveal.
  const remote = useApp((s) => !!s.selectedId && isRemoteThread(s.selectedId));
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-caption text-fg-faint">
      <span>Preview only; full output saved to</span>
      <span className="selectable min-w-0 break-all font-mono">{path}</span>
      {!remote && (
        <Pill quiet size="sm" onClick={() => openPath(path)}>
          Open
        </Pill>
      )}
    </span>
  );
}

export const ToolRow = memo(function ToolRow({
  item,
  cwd,
  threadId,
}: {
  item: Item<"tool">;
  cwd: string;
  /** The thread the call is in: where an MCP App reaches its server. */
  threadId?: string;
}) {
  const kind = inferToolKind(item);
  const [open, setOpen] = useState(kind === "edit" && item.diffs.length > 0 && item.diffs.length <= 2);
  const Icon = KIND_ICON[kind] ?? Wrench;
  // Only a turn that runs has a call in flight: a row its turn left open does not spin.
  const turnRunning = useApp((s) => (threadId ? isRunning(s.threads[threadId]) : true));
  const running = toolInFlight(item.status, turnRunning);
  const failed = item.status === "failed";
  // MCP calls come named `mcp__server__tool`: the row says what they did instead.
  const row = formatToolRow(item, kind, cwd);
  const startedId = orchestrationTool(item.title) === "start_session" ? sessionIdOf(item.output) : undefined;
  const startedRun = orchestrationTool(item.title) === "run_workflow" ? runIdOf(item.output) : undefined;
  const link = useApp((s) => (startedId ? s.links.links.find((l) => l.target === startedId) : undefined));
  const linkState = useApp((s) => (link ? stateOf(s, link) : undefined));
  const opensThread = !!startedId && !!link;
  // Why it failed, on the row itself: the first line of what it printed, when no exit code says it.
  const exitCode = item.exitCode ?? (failed ? printedExitCode(item.output) : undefined);
  const reason =
    failed && item.denial
      ? `Denied by ${item.denial.by}`
      : failed && exitCode === undefined
        ? failureReason(item.output)
        : "";
  const counts = useMemo(() => diffCounts(item.diffs), [item.diffs]);
  return (
    <div>
      <div className={cx("group flex items-center gap-0.5", open && "rc-sticky-head")}>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="-mx-0.5 flex min-h-6 w-fit max-w-[calc(100%+4px)] items-center gap-1.5 rounded-chip px-0.5 text-left leading-chat hover:bg-fill-subtle"
        >
          <span className="flex w-[18px] shrink-0 justify-center">
            {running ? (
              <Icon size={16} className="rc-icon-pulse text-fg-muted" />
            ) : failed && exitCode === undefined ? (
              <CircleX size={16} className="text-danger" />
            ) : (
              <Icon size={16} className="text-fg-muted" />
            )}
          </span>
          <span
            className={cx(
              "min-w-0 truncate text-fg-muted",
              row.pathFile
                ? "shrink-0 text-body"
                : row.titleMono
                  ? "max-w-full shrink font-mono text-callout"
                  : row.summary || row.detail
                    ? "max-w-[55%] shrink-0 text-body"
                    : "max-w-full shrink text-body",
            )}
            title={item.title}
          >
            {row.title}
          </span>
          {row.pathFile && (
            <span className="flex min-w-0 shrink items-baseline truncate font-mono text-callout" dir="auto">
              {row.pathDir && <span className="min-w-0 truncate text-fg-faint">{row.pathDir}</span>}
              <span className="shrink-0 text-fg-muted">{row.pathFile}</span>
            </span>
          )}
          {row.lineRange && (
            <span className="shrink-0 font-mono text-caption text-fg-faint tabular">{row.lineRange}</span>
          )}
          {row.summary && (
            <>
              <span aria-hidden className="size-0.5 shrink-0 rounded-full bg-fg-faint" />
              <span className="min-w-0 shrink truncate text-callout leading-chat-sub text-fg-faint">
                {row.summary}
              </span>
            </>
          )}
          {row.detail && (
            <span
              className={cx(
                "min-w-0 shrink truncate text-caption text-fg-faint",
                row.detailMono && "font-mono",
              )}
              dir="auto"
            >
              {row.detail}
            </span>
          )}
          {item.background === "command" && (
            <span className="shrink-0 text-caption text-fg-faint">Started in background</span>
          )}
          {failed && exitCode === undefined && (
            <span
              className="min-w-0 max-w-[45%] shrink truncate text-caption text-danger"
              title={reason || undefined}
            >
              {reason || "Failed"}
            </span>
          )}
          <DiffCount {...counts} />
          {exitCode !== undefined && exitCode !== 0 && (
            <span className="flex h-[18px] shrink-0 items-center gap-1 rounded-full bg-fill px-1.5 font-mono text-caption text-fg-muted tabular">
              <span aria-hidden className="size-1.5 rounded-full bg-danger" />
              <span className="sr-only">Exit code </span>
              {exitCode}
            </span>
          )}
          {item.autoAllowed && (
            <Tooltip label={`Allowed by ${item.autoAllowed}`}>
              <span className="flex shrink-0 items-center text-fg-faint">
                <Shield size={12} />
              </span>
            </Tooltip>
          )}
          {linkState && (
            <Tooltip label={`“${link?.targetTitle}” · ${LINK_LABEL[linkState]}`}>
              <span className="flex shrink-0 items-center">
                <LinkStateIcon state={linkState} size={12} />
              </span>
            </Tooltip>
          )}
          <ChevronRight
            size={12}
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
              <ExternalLink size={12} />
            </button>
          </Tooltip>
        )}
      </div>
      {failed && item.denial && <HarnessDenialLine denial={item.denial} />}
      <McpAppSlot item={item} threadId={threadId} />
      {startedRun && threadId && <WorkflowCard threadId={threadId} runId={startedRun} />}
      {open && (
        <div className="rc-expand mt-1.5 mb-1 ml-[26px]">
          <ToolDetail item={item} />
        </div>
      )}
    </div>
  );
});

/**
 * A refusal by the harness's own permission layer, under its row: who said no and why, that
 * RustCode's access level does not control it, and where it is changed (`docs/permissions.md`).
 */
function HarnessDenialLine({ denial }: { denial: HarnessDenial }) {
  return (
    <p className="mt-0.5 ml-[26px] max-w-[72ch] text-caption text-fg-muted" role="note">
      {harnessDenialText(denial)}
    </p>
  );
}

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
                <ArrowRight size={12} className="text-fg-faint" />
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
                <ArrowRight size={12} className="text-fg-faint" />
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

export { HandoffRow } from "./HandoffRow";

/**
 * A turn the app woke the thread for: a delegate finished (or a few did), which
 * ones, on which harness and for how long. The same flat divider as a notice, one
 * line that never wraps: the title gives way, the harness and the time stay.
 * The whole wake message is the hover title, so the rest of it is a tip away.
 * Its words come from the text alone (`state/wakeNotice.ts`), so a thread
 * reloaded from history shows the row it showed live.
 */
export function WakeRow({ text, flat, stacked }: { text: string; flat?: boolean; stacked?: boolean }) {
  const notice = useMemo(() => wakeNotice(text), [text]);
  if (!notice) {
    const line = wakeLine(text);
    return (
      <NoticeRow
        flat={flat}
        stacked={stacked}
        event={pullRequestEvent(line)}
        item={{ kind: "notice", id: "wake", text: line, tone: "info" }}
      />
    );
  }
  const marks = notice.finished.filter((d) => d.harness).slice(0, 3);
  const askingMarks = notice.asking.filter((d) => d.harness).slice(0, 3);
  const icons = marks.length ? marks : askingMarks;
  return (
    <Tooltip label={text}>
      <div
        role="separator"
        aria-label={text}
        className={cx("flex items-center gap-3 text-caption text-fg-muted", flat ? "py-0" : "py-1")}
      >
        <span className="h-px flex-1 bg-border" />
        <span className="flex min-w-0 items-center gap-1.5 whitespace-nowrap">
          {icons.length ? (
            icons.map((d, i) => (
              <HarnessIcon key={d.linkId ?? i} id={harnessId(d.harness ?? "")} name={d.harness} size={13} />
            ))
          ) : notice.asking.length && !notice.finished.length ? (
            <MessageCircleQuestion size={12} className="shrink-0 text-fg-faint" aria-hidden />
          ) : (
            <Layers size={12} className="shrink-0 text-fg-faint" aria-hidden />
          )}
          {notice.failed && <CircleX size={12} className="shrink-0 text-danger" aria-hidden />}
          <span className="shrink-0">{notice.label}</span>
          {notice.titles && (
            <>
              <span aria-hidden className="text-fg-faint">
                ·
              </span>
              <span className="min-w-0 truncate">{notice.titles}</span>
            </>
          )}
          {notice.harness && (
            <>
              <span aria-hidden className="text-fg-faint">
                ·
              </span>
              <span className="shrink-0">{notice.harness}</span>
            </>
          )}
          {notice.worked && (
            <>
              <span aria-hidden className="text-fg-faint">
                ·
              </span>
              <span className="shrink-0 tabular">{notice.worked}</span>
            </>
          )}
        </span>
        <span className="h-px flex-1 bg-border" />
      </div>
    </Tooltip>
  );
}

export function NoticeRow({
  item,
  action,
  flat,
  stacked,
  event,
}: {
  item: Item<"notice">;
  action?: ReactNode;
  /** A wake divider sits in the transcript's own rhythm, without the padding a card needs. */
  flat?: boolean;
  /** Under another divider: one more line of the same list, without rules of its own. */
  stacked?: boolean;
  /** What a pull request did, so a merge and a failure do not look alike. */
  event?: PullRequestEvent;
}) {
  // Something that happened to the thread (it compacted), not a problem: a divider, like a switch.
  // Said many times in a row it is still one line, with the count on its end.
  const times = (item.count ?? 1) > 1 && (
    <span className="shrink-0 tabular text-fg-faint">×{item.count}</span>
  );
  if (item.tone === "info")
    return (
      <div
        className={cx("flex items-center gap-3 text-caption text-fg-muted", flat ? "py-0" : "py-1")}
        role="separator"
      >
        <span className={cx("h-px flex-1", !stacked && "bg-border")} />
        <span className={cx("flex min-w-0 items-center gap-1.5", event === "failed" && "text-fg")}>
          {item.progress ? <Spinner size={12} /> : <NoticeIcon event={event} />}
          <span className="truncate">{item.text}</span>
          {times}
          {item.setting === "auto-compact" && (
            <button
              type="button"
              onClick={() => openSettings("general")}
              className="shrink-0 rounded-sm text-fg-faint underline-offset-2 hover:text-fg hover:underline"
            >
              Auto-compact setting
            </button>
          )}
        </span>
        <span className={cx("h-px flex-1", !stacked && "bg-border")} />
      </div>
    );
  return (
    <Callout tone="warning" action={action}>
      {item.text} {times}
    </Callout>
  );
}

function NoticeIcon({ event }: { event?: PullRequestEvent }) {
  if (event === "merged") return <GitMerge size={12} className="shrink-0 text-success" />;
  if (event === "failed") return <CircleX size={12} className="shrink-0 text-danger" />;
  if (event === "comment") return <MessageSquare size={12} className="shrink-0 text-fg-faint" />;
  return <Layers size={12} className="shrink-0 text-fg-faint" />;
}

/** What a finished turn changed and which checks it ran, completing the engine's checkpoint chip
 * (the two are never drawn for the same turn, state/fold.ts `turnChangeLine`). */
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
              size={12}
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
              <FileText size={12} className="shrink-0 text-fg-muted" />
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
  // A turn that only replied has nothing to open: the time alone, over its answer.
  if (!fold.steps)
    return (
      <div
        className={cx(
          "flex w-full items-center gap-1.5 pt-0 pb-2 text-body leading-chat text-fg-faint",
          rule && "border-border/60 border-b",
        )}
      >
        {fold.stopped && <SquareStop size={12} className="shrink-0" aria-hidden />}
        <span className="truncate tabular">{foldLabel(fold, formatDuration)}</span>
        {!fold.answered && fold.time !== undefined && (
          <span className="shrink-0 text-caption text-fg-faint tabular">· {messageTime(fold.time)}</span>
        )}
        {fold.failedTurn && (
          <CircleX size={12} className="shrink-0 text-danger" aria-label="The turn failed" />
        )}
      </div>
    );
  return (
    <button
      type="button"
      aria-expanded={fold.open}
      onClick={() => onToggle(fold.key)}
      className={cx(
        "group flex w-full items-center gap-1.5 pt-0 pb-2 text-left text-body leading-chat text-fg-faint hover:text-fg-muted",
        rule && "border-border/60 border-b",
      )}
    >
      {fold.stopped && <SquareStop size={12} className="shrink-0" aria-hidden />}
      <span className="shrink-0 tabular">{foldLabel(fold, formatDuration)}</span>
      {fold.summary && <span className="min-w-0 truncate text-fg-faint">· {fold.summary}</span>}
      {!fold.answered && fold.time !== undefined && (
        <span className="shrink-0 text-caption text-fg-faint tabular">· {messageTime(fold.time)}</span>
      )}
      {fold.failedTurn && <CircleX size={12} className="shrink-0 text-danger" aria-label="The turn failed" />}
      <ChevronRight
        size={12}
        className={cx("shrink-0 text-fg-faint transition-transform duration-100", fold.open && "rotate-90")}
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
    // Two or more finished calls read as one step of the turn, and stay quieter than the prose
    // around it: no hairline, a smaller and fainter line of it, and the words and icon only come
    // up to the text colour on hover. Its own air is the row gap's alone: the transcript sets the
    // spacing between items, so a row adds none above or below it.
    <div>
      <button
        type="button"
        aria-expanded={group.open}
        onClick={() => onToggle(group.key)}
        data-kind={group.kind}
        className="group -mx-0.5 flex min-h-6 w-fit max-w-[calc(100%+4px)] items-center gap-1.5 rounded-chip px-0.5 text-left leading-chat hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          {(() => {
            const Icon = (group.kind && KIND_ICON[group.kind]) || Layers;
            return <Icon size={14} className="text-fg-faint group-hover:text-fg" />;
          })()}
        </span>
        <span className="min-w-0 truncate text-callout text-fg-muted group-hover:text-fg">{group.label}</span>
        {group.failed > 0 && (
          <span className="flex shrink-0 items-center gap-0.5 text-caption text-danger">
            <CircleX size={12} aria-hidden />
            {group.failed} failed
          </span>
        )}
        <ChevronRight
          size={12}
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
  trace,
  folded,
  timeInFold,
  threadId,
  latest,
}: {
  item: Item<"turnEnd">;
  /** The turn's work is folded into a summary row that already says how long it took. */
  folded?: boolean;
  /** The summary row above carries the time too: the turn wrote no answer. */
  timeInFold?: boolean;
  /** The turn's last reply, whose copy and fork show here rather than under the reply. */
  reply?: string;
  /** The turn's own change line: its completion card, when the harness reported one. */
  completion?: TurnCompletion | null;
  /** The turn's checkpoint chip (Changes), when the harness reported no completion. */
  changes?: ReactNode;
  /** The turn's Trace chip, when the app watched it run. */
  trace?: ReactNode;
  /** The thread, so the footer's fork can copy its conversation. */
  threadId?: string;
  latest?: boolean;
}) {
  // The turn's only action row: outcome and time, the trace and changes chips, then copy and fork. The newest
  // answer keeps its actions in view; an older turn's fade in under the pointer or focus, in room
  // the row always keeps (docs/design.md › Type).
  const row = turnActions({ text: reply, at: item.at, threadId });
  const sideChat = useApp(
    (s) => s.sessions.find((session) => session.id === threadId)?.parent?.relationship === "side_chat",
  );
  const traced = hasTimeline(item) && !!trace;
  // An empty turn that folded its work: its header already says the time, and there is no reply
  // to copy or fork, so a footer row of icons would sit over nothing.
  if (
    turnFooterHidden({
      folded,
      reply,
      completion: !!completion,
      changes: !!changes,
      trace: traced,
      outcome: item.outcome,
    })
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
  return (
    <div>
      {completion && <CompletionCard completion={completion} />}
      <div
        className={cx(
          "group/foot flex h-6 items-center gap-2 text-callout text-fg-faint",
          completion && "mt-1",
        )}
      >
        {item.outcome === "cancelled" && !folded && <SquareStop size={12} />}
        {traced && trace}
        {changes}
        {parts.length > 0 && (
          <span
            className={cx(
              "tabular",
              !latest && item.outcome !== "cancelled" && item.outcome !== "failed" && FADE_UNTIL_HOVER,
            )}
          >
            {parts.join(" · ")}
          </span>
        )}
        {(row.copy || row.fork || row.sideChat) && (
          <span
            className={cx(
              "flex items-center gap-2 [&>button]:size-6 [&>button]:rounded-chip [&>button]:text-fg-faint",
              !latest && FADE_UNTIL_HOVER,
            )}
          >
            {row.copy && <CopyButton text={row.copy} iconOnly label="Copy reply" />}
            {sideChat && row.copy && (
              <Tooltip label="Send to main thread">
                <button
                  type="button"
                  aria-label="Send to main thread"
                  className="inline-flex size-6 items-center justify-center rounded-chip text-fg-faint hover:bg-fill hover:text-fg"
                  onClick={() => sendSideChatToParent(threadId!, row.copy!)}
                >
                  <ArrowRight size={12} />
                </button>
              </Tooltip>
            )}
            {row.fork && <ReplyForkButton threadId={threadId!} />}
            {row.sideChat && row.copy && <ReplySideChatButton threadId={threadId!} reply={row.copy} />}
          </span>
        )}
      </div>
    </div>
  );
}

/** An older turn's quiet parts: drawn only under the pointer or focus, in room the row keeps. */
const FADE_UNTIL_HOVER =
  "opacity-0 transition-opacity duration-100 focus-within:opacity-100 group-hover/foot:opacity-100 pointer-coarse:opacity-100";

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
        <span>
          {waitingLabel(wait)}
          <span className="text-fg-faint"> · {waitingNext(wait)}</span>
        </span>
      </div>
    </Tooltip>
  );
}

/**
 * The running turn's header, right under the message that started it:
 */
export function WorkingRow({
  startedAt,
  now,
  done,
  elapsedMs,
  awaitingApproval,
}: {
  startedAt?: number;
  now: number;
  /** The turn finished: the line settles on a check for a moment instead of vanishing. */
  done?: boolean;
  /** The finished turn's own duration, once `turnStartedAt` is gone. */
  elapsedMs?: number;
  awaitingApproval?: boolean;
}) {
  const elapsed = elapsedMs ?? (startedAt ? Math.max(0, now - startedAt) : undefined);
  const time = elapsed !== undefined ? formatDuration(Math.floor(elapsed / 1000) * 1000) : undefined;
  const flavor = turnFlavor(startedAt);
  return (
    <div className="flex h-[34px] min-w-0 items-center gap-2 border-border/60 border-b pt-1 pb-2 text-body text-fg-muted tabular">
      {done ? (
        <>
          <CircleCheck size={13} className="shrink-0 text-success rc-dot-in" aria-hidden />
          <span>{time ? `Worked for ${time}` : "Worked"}</span>
        </>
      ) : (
        <>
          <span className="flex shrink-0 items-center gap-2" role="status">
            {!awaitingApproval && (
              <DotMatrix pattern={flavor.pattern} size={16} cycle tone="spectrum" className="opacity-70" />
            )}
            {awaitingApproval ? (
              "Waiting for approval"
            ) : (
              <WaveText
                text={time ? `Working for ${time}` : "Working"}
                className="rc-wave-quiet rc-wave-line"
              />
            )}
          </span>
        </>
      )}
    </div>
  );
}

// The diff row lives in its own file now (ui/DiffView.tsx); this keeps its importers.
export { DiffView };
