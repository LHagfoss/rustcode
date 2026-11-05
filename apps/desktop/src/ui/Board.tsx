// The Board: the threads as cards in four columns, Running, Needs input,
// Review and Done, as an alternative to the list. Drag a card to Review or
// Done (or use its buttons) to mark it; the engine moves merged worktree
// threads to Done by itself. Idle threads stay in the list.

import { CircleCheck, Eye, GitBranch, PanelLeft, SquarePen, Undo2 } from "lucide-react";
import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { projectName, type SessionSummary } from "../acp/types";
import { STATUS_LABEL, type ThreadStatus } from "../acp/v2";
import {
  selectThread,
  setThreadStatus,
  settleThread,
  startNewThread,
  statusOf,
  toggleSidebar,
  useApp,
  useShortcutHint,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { Cluster, ClusterButton, cx, Pill, Tooltip } from "./primitives";
import { StatusIcon } from "./Status";
import { useLightsInset } from "./Titlebar";

const COLUMNS: { status: ThreadStatus; hint: string }[] = [
  { status: "running", hint: "A turn is running" },
  { status: "needs_input", hint: "Waiting on an approval or a question" },
  { status: "review", hint: "Ready for you to look at" },
  { status: "done", hint: "Finished, or its branch merged" },
];

/** A header for views that replace the chat: clear of the traffic lights when the sidebar hides. */
export function ViewHeader({ title, children }: { title: React.ReactNode; children?: React.ReactNode }) {
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const toggleKey = useShortcutHint("toggle-sidebar");
  const newKey = useShortcutHint("new-thread");
  const lightsInset = useLightsInset();
  return (
    <header
      data-tauri-drag-region
      className={cx(
        "flex h-[42px] shrink-0 items-center gap-2.5 pt-1.5 pr-2.5 pb-2",
        !sidebarHidden && "pl-4",
      )}
      style={sidebarHidden ? { paddingLeft: lightsInset } : undefined}
    >
      {sidebarHidden && (
        <Cluster>
          <ClusterButton label="Show sidebar" tooltip={`Show sidebar (${toggleKey})`} onClick={toggleSidebar}>
            <PanelLeft size={14} />
          </ClusterButton>
          <ClusterButton
            label="New thread"
            tooltip={`New thread (${newKey})`}
            onClick={() => startNewThread()}
          >
            <SquarePen size={14} />
          </ClusterButton>
        </Cluster>
      )}
      <h1 className="min-w-0 flex-1 truncate font-semibold text-body" data-tauri-drag-region>
        {title}
      </h1>
      {children}
    </header>
  );
}

function Card({ session, status }: { session: SessionSummary; status: ThreadStatus }) {
  const harness = useApp((s) =>
    session.harness && session.harness !== "rustcode"
      ? (s.harnesses?.harnesses.find((h) => h.id === session.harness)?.name ?? session.harness)
      : undefined,
  );
  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const wt = session.worktree;
  return (
    <div
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("text/rustcode-thread", session.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      className="group rounded-lg border border-border bg-fill-subtle p-2.5 transition-[background-color,transform,box-shadow] duration-150 ease-out hover:-translate-y-px hover:bg-fill-subtle hover:shadow-1 rc-in"
    >
      <button type="button" onClick={() => selectThread(session.id)} className="block w-full text-left">
        <span className="line-clamp-2 text-body">{session.title || "New thread"}</span>
        {request && (
          <span className="mt-0.5 block truncate text-warning text-caption">
            {request.question ? `Asks: ${request.question.text}` : `Approve: ${request.title}`}
          </span>
        )}
        <span className="mt-1.5 flex flex-wrap items-center gap-1 text-caption text-fg-muted">
          {wt ? (
            <span className="flex max-w-full items-center gap-0.5 rounded-sm bg-fill-subtle px-1 font-mono text-[10px]">
              <GitBranch size={9} className="shrink-0" />
              <span className="truncate">{wt.branch}</span>
            </span>
          ) : (
            session.cwd && <span>{projectName(session.cwd)}</span>
          )}
          {harness && <span>· {harness}</span>}
          <span>· {relativeAge(session.updatedAt)}</span>
        </span>
      </button>
      {(status === "review" || status === "done") && (
        <div className="mt-2 flex gap-1 opacity-0 transition-opacity duration-150 group-focus-within:opacity-100 group-hover:opacity-100">
          {status === "review" ? (
            <Tooltip label="Mark done">
              <Pill size="sm" aria-label="Mark done" onClick={() => settleThread(session.id)}>
                <CircleCheck size={11} /> Done
              </Pill>
            </Tooltip>
          ) : (
            <Tooltip label="Back to review">
              <Pill
                size="sm"
                aria-label="Back to review"
                onClick={() => void setThreadStatus(session.id, "review")}
              >
                <Eye size={11} /> Review
              </Pill>
            </Tooltip>
          )}
          <Tooltip label="Clear the status">
            <button
              type="button"
              aria-label="Clear the status"
              onClick={() => void setThreadStatus(session.id, "idle")}
              className="flex size-6 items-center justify-center rounded-full bg-fill hover:bg-fill-strong"
            >
              <Undo2 size={11} />
            </button>
          </Tooltip>
        </div>
      )}
    </div>
  );
}

export function Board() {
  const sessions = useApp((s) => s.sessions);
  const archived = useApp((s) => s.archived);
  const statuses = useApp(
    useShallow(
      (s) =>
        Object.fromEntries(s.sessions.map((x) => [x.id, statusOf(s, x.id)])) as Record<string, ThreadStatus>,
    ),
  );
  const [over, setOver] = useState<ThreadStatus | null>(null);
  const columns = useMemo(
    () =>
      COLUMNS.map((c) => ({
        ...c,
        items: sessions.filter((x) => !(x.id in archived) && statuses[x.id] === c.status),
      })),
    [sessions, archived, statuses],
  );
  const idle = sessions.filter((x) => !(x.id in archived) && statuses[x.id] === "idle").length;
  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg" aria-label="Board">
      <ViewHeader title="Board">
        <span className="text-caption text-fg-muted">{idle} idle in the list</span>
      </ViewHeader>
      <div className="grid min-h-0 flex-1 grid-cols-4 gap-3 overflow-x-auto px-4 pb-4">
        {columns.map((c) => {
          const droppable = c.status === "review" || c.status === "done";
          return (
            <section
              key={c.status}
              aria-label={STATUS_LABEL[c.status]}
              onDragOver={(e) => {
                if (!droppable || !e.dataTransfer.types.includes("text/rustcode-thread")) return;
                e.preventDefault();
                setOver(c.status);
              }}
              onDragLeave={() => setOver(null)}
              onDrop={(e) => {
                setOver(null);
                const id = e.dataTransfer.getData("text/rustcode-thread");
                if (!id || !droppable) return;
                if (c.status === "done") settleThread(id);
                else void setThreadStatus(id, c.status as "review" | "idle");
              }}
              className={cx(
                "flex min-h-0 min-w-[200px] flex-col rounded-xl bg-fill-subtle transition-[background-color,box-shadow] duration-150",
                over === c.status && "bg-accent/10 shadow-[inset_0_0_0_1.5px_var(--rc-accent)]",
              )}
            >
              <Tooltip label={c.hint}>
                <div className="flex items-center gap-1.5 px-3 pt-3 pb-2 font-semibold text-callout">
                  <StatusIcon status={c.status} size={12} />
                  {STATUS_LABEL[c.status]}
                  <span className="font-normal text-fg-faint tabular">{c.items.length}</span>
                </div>
              </Tooltip>
              <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2">
                {c.items.map((x) => (
                  <Card key={x.id} session={x} status={c.status} />
                ))}
                {!c.items.length && (
                  <div className="px-2 py-6 text-center text-caption text-fg-faint">
                    {droppable ? "Drop a thread here" : "Nothing here"}
                  </div>
                )}
              </div>
            </section>
          );
        })}
      </div>
    </main>
  );
}
