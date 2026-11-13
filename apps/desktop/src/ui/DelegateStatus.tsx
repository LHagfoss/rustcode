// Where an orchestrator thread's delegates stand (issue 3651): the pinned panel above the composer,
// refreshed while it is open, and the row a bare "status" message gets as its local reply. Both draw
// `DelegateStatusRow`s (state/delegateStatus.ts), so they say the same thing in the same words.

import { ChevronDown, Layers, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  canStop,
  type DelegateStatusRow,
  idleText,
  panelAttention,
  panelLabel,
  panelMarks,
  pullText,
  runsOn,
  statusRows,
  statusSummary,
} from "../state/delegateStatus";
import { AGENT_WORD, type AgentState } from "../state/delegates";
import { delegateStatusInputs, readDelegatePull, stopDelegate } from "../state/host";
import { selectThread, useApp } from "../state/store";
import { AgentMarks } from "./AgentStack";
import { cx, Spinner, Tooltip } from "./primitives";

const TINT: Record<AgentState, string> = {
  working: "bg-accent",
  needs: "bg-warning",
  done: "bg-success",
  failed: "bg-danger",
  stopped: "bg-fg-faint",
};

/** How often the panel looks again: the transcripts are in memory, so this costs no request. */
const REFRESH_MS = 2_000;
/** How old a pull request read may be before the panel reads it again (`gh`, one per delegate). */
const PULL_REFRESH_MS = 25_000;

/** The second line of a row: what it last ran, what it runs on, where its work is. */
function detailOf(row: DelegateStatusRow, compact: boolean): string {
  return [
    // The panel above the composer leaves the tool out: a long command is what made it tall.
    !compact && row.lastTool ? `last tool ${row.lastTool}` : "",
    runsOn(row),
    row.branch ?? "",
    row.pull ? pullText(row.pull) : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/**
 * One delegate: its state and how long it has been quiet, then what it ran and where it stands. In the
 * panel (`stop` given) work still under way has its own Stop beside the row, never inside the button
 * that opens the thread.
 */
function StatusLine({
  row,
  onOpen,
  stop,
}: {
  row: DelegateStatusRow;
  onOpen: (id: string) => void;
  stop?: (row: DelegateStatusRow) => void;
}) {
  const idle = idleText(row.idleMs);
  const detail = detailOf(row, stop !== undefined);
  return (
    <div className="flex min-w-0 items-center gap-0.5">
      <button
        type="button"
        onClick={() => onOpen(row.id)}
        title={`Open ${row.title}`}
        className="flex min-w-0 flex-1 flex-col gap-0.5 rounded-lg px-2 py-1 text-left hover:bg-fill-subtle active:bg-fill-strong"
      >
        <span className="flex min-w-0 items-center gap-2 text-callout">
          <span aria-hidden className={cx("size-1.5 shrink-0 rounded-full", TINT[row.state])} />
          <span className="min-w-0 truncate">{row.title}</span>
          <span
            className={cx(
              "ml-auto shrink-0 text-caption",
              row.state === "needs" ? "text-warning" : "text-fg-muted",
            )}
          >
            {AGENT_WORD[row.state]}
            {idle ? ` · ${idle}` : ""}
          </span>
        </span>
        {detail && <span className="truncate pl-4 text-caption text-fg-faint">{detail}</span>}
      </button>
      {stop && canStop(row) && (
        <Tooltip label="Stop this work in that thread">
          <button
            type="button"
            aria-label={`Stop ${row.title}`}
            onClick={() => stop(row)}
            className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
          >
            <X size={12} strokeWidth={2.5} />
          </button>
        </Tooltip>
      )}
    </div>
  );
}

/**
 * Pinned above the composer while a thread has delegates working or waiting, and folded to one line
 * until opened: the harness icons of what is live, how many delegates there are, and what needs the
 * reader in its own tint. Opened, it lists a row per delegate (state, idle time, account and model,
 * branch, pull request and checks) with Open and Stop, scrolled inside a bounded height; Escape folds
 * it again. Read off the app's own state every couple of seconds, and the pull requests every
 * half minute, so it moves without the thread being asked anything.
 */
export function DelegateStatusPanel({ threadId }: { threadId: string }) {
  const [rows, setRows] = useState<DelegateStatusRow[]>([]);
  const [folded, setFolded] = useState(true);
  const header = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const read = () => {
      const state = useApp.getState();
      const next = statusRows(delegateStatusInputs(state, threadId), Date.now());
      setRows((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next));
    };
    read();
    const timer = setInterval(read, REFRESH_MS);
    return () => clearInterval(timer);
  }, [threadId]);
  const live = rows.some((row) => row.state === "working" || row.state === "needs");
  // The delegates whose pull request can still change: not merged, not closed.
  const open = rows
    .filter((row) => !row.pull?.pull || (!row.pull.pull.merged && row.pull.pull.state === "open"))
    .map((row) => row.id)
    .join(",");
  useEffect(() => {
    if (!live || !open) return;
    const load = () => {
      if (document.hidden) return;
      for (const id of open.split(",")) {
        void readDelegatePull(id, PULL_REFRESH_MS).then((pull) => {
          setRows((prev) => prev.map((row) => (row.id === id ? { ...row, pull } : row)));
        });
      }
    };
    load();
    const timer = setInterval(load, PULL_REFRESH_MS);
    return () => clearInterval(timer);
  }, [live, open]);
  if (!live) return null;
  const attention = panelAttention(rows);
  const rowsId = `delegate-status-${threadId}`;
  return (
    <section
      aria-label="Delegate status"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || folded) return;
        event.stopPropagation();
        setFolded(true);
        header.current?.focus();
      }}
      className="rc-glass-overlay relative flex flex-col gap-0.5 rounded-xl p-1 rc-in"
    >
      <button
        ref={header}
        type="button"
        aria-expanded={!folded}
        aria-controls={folded ? undefined : rowsId}
        onClick={() => setFolded(!folded)}
        className="flex h-7 w-full min-w-0 items-center gap-1.5 rounded-lg px-2 text-left text-caption text-fg-muted hover:bg-fill-subtle active:bg-fill-strong"
      >
        <ChevronDown
          size={12}
          aria-hidden
          className={cx("shrink-0 text-fg-faint transition-transform duration-200", folded && "-rotate-90")}
        />
        <AgentMarks agents={panelMarks(rows)} size="sidebar" tone="surface" live={false} />
        <span
          className={cx(
            "min-w-0 truncate",
            attention === "needs" && "text-warning",
            attention === "failed" && "text-danger",
          )}
        >
          {panelLabel(rows)}
        </span>
        {rows.some((row) => row.state === "working") && <Spinner size={9} className="text-accent" />}
      </button>
      {!folded && (
        <div id={rowsId} className="flex max-h-[min(240px,35vh)] flex-col gap-0.5 overflow-y-auto">
          {rows.map((row) => (
            <StatusLine
              key={row.id}
              row={row}
              onOpen={selectThread}
              stop={(r) => void stopDelegate(threadId, r.id)}
            />
          ))}
        </div>
      )}
    </section>
  );
}

/**
 * The reply to a bare "status": the same rows, drawn as the transcript's own card with the words
 * that it came from the app and not the model, so nobody reads it as the thread's reasoning.
 */
export function StatusReplyRow({ rows }: { rows: readonly DelegateStatusRow[] }) {
  return (
    <div className="flex flex-col gap-1 rounded-xl border border-border bg-fill-subtle px-1 py-1.5">
      <div className="flex items-center gap-1.5 px-2 text-caption text-fg-muted">
        <Layers size={11} aria-hidden className="shrink-0 text-fg-faint" />
        <span className="font-medium text-fg">Status</span>
        <span className="min-w-0 truncate">· {statusSummary(rows)}</span>
        <span className="ml-auto shrink-0 text-fg-faint">Local reply · no model turn</span>
      </div>
      {rows.map((row) => (
        <StatusLine key={row.id} row={row} onOpen={selectThread} />
      ))}
    </div>
  );
}
