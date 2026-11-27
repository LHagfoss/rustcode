/**
 * Job overview dialog: a roomy two-column summary of one scheduled job — its live status, quick
 * actions (run, stop, pause/resume, edit, duplicate, delete), schedule, configuration, prompt,
 * pre-check script, health metrics, and full clickable run history.
 */

import { Dialog } from "radix-ui";
import { useRef, useState } from "react";
import { projectName } from "../acp/types";
import { runThreadKey } from "../state/fleetLoops";
import {
  cancelLoopRun,
  isBlockedRun,
  type Loop,
  loopState,
  nextRunText,
  pauseReason,
  runLoopNow,
  scheduleInWords,
  setLoopPaused,
} from "../state/loops";
import { selfName, useLocalIdentity, useMachines } from "../state/machines";
import { openSettings } from "../state/store";
import { selectThread } from "../state/threadOpen";
import {
  ArrowUpRight,
  Clock,
  Copy,
  Folder,
  History,
  Pause,
  Pencil,
  Play,
  Server,
  Shield,
  ShieldAlert,
  SquareStop,
  Terminal,
  Trash2,
  Webhook,
  X,
} from "./icons";
import { averageRunDuration, FULL_RUN_FORMAT, isFailedRun, RunList, RunTimelineStrip } from "./JobRuns";
import { CopyButton } from "./Markdown";
import { Callout, cx, Pill, Segments, Spinner, Tooltip } from "./primitives";

type RunFilter = "all" | "failed";

function OverviewStatusChip({ loop }: { loop: Loop }) {
  const state = loopState(loop);
  const base = "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 font-medium text-caption";
  if (state === "running") {
    return (
      <span className={cx(base, "bg-accent/14 text-accent")}>
        <Spinner size={10} />
        Running
      </span>
    );
  }
  if (state === "failing") {
    return <span className={cx(base, "bg-danger/12 text-danger")}>Needs attention</span>;
  }
  if (state === "paused") {
    return <span className={cx(base, "bg-fill text-fg-muted")}>Paused</span>;
  }
  if (isBlockedRun(loop.runs[0])) {
    return <span className={cx(base, "bg-warning/14 text-warning")}>Blocked</span>;
  }
  if (loop.schedule.kind === "manual" && loop.webhook) {
    return (
      <span className={cx(base, "bg-fill text-fg-muted")}>
        <Webhook size={10} />
        Webhook
      </span>
    );
  }
  return (
    <span className={cx(base, "bg-success/12 text-success")}>
      <span aria-hidden className="size-1.5 rounded-full bg-success" />
      Active
    </span>
  );
}

function ConfigRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[6rem_minmax(0,1fr)] items-baseline gap-2 py-1.5 text-callout">
      <dt className="text-caption text-fg-faint">{label}</dt>
      <dd className="min-w-0 text-fg">{children}</dd>
    </div>
  );
}

export function JobDetails({
  loop,
  now,
  agent,
  machine,
  onClose,
  onEdit,
  onDuplicate,
  onDelete,
  act,
}: {
  loop: Loop;
  now: number;
  agent: string;
  machine: string;
  onClose: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  act: (what: string, run: () => Promise<void>) => void;
}) {
  const returnFocus = useRef<HTMLElement | null>(null);
  const [runFilter, setRunFilter] = useState<RunFilter>("all");
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const localName = local.displayName ?? selfName();

  const state = loopState(loop);
  const running = state === "running";
  const reason = pauseReason(loop);
  const latest = loop.runs[0];
  const latestThread = running ? undefined : runThreadKey(loop, latest, machines, localName);
  const next = !loop.paused && loop.schedule.kind !== "manual" ? loop.nextDueAt : undefined;
  const timezone = "timezone" in loop.schedule ? loop.schedule.timezone : undefined;

  const succeededCount = loop.runs.filter((run) => run.state === "succeeded").length;
  const failedCount = loop.runs.filter(isFailedRun).length;
  const avgDuration = averageRunDuration(loop.runs);

  const showFilter = failedCount > 0 && loop.runs.length > failedCount;
  const effectiveFilter: RunFilter = showFilter ? runFilter : "all";
  const visibleRuns = effectiveFilter === "failed" ? loop.runs.filter(isFailedRun) : loop.runs;

  const togglePause = () =>
    act(loop.paused ? "Resumed" : "Paused", () => setLoopPaused(loop.id, !loop.paused, loop.host));

  const triggerLabel =
    loop.schedule.kind === "manual"
      ? loop.webhook
        ? "Webhook or manual"
        : "Manual only"
      : loop.webhook
        ? "Schedule + webhook"
        : "Schedule";

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content
            onOpenAutoFocus={() => {
              returnFocus.current =
                document.activeElement instanceof HTMLElement ? document.activeElement : null;
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              if (document.activeElement === document.body) returnFocus.current?.focus();
            }}
            className="elevated pointer-events-auto flex max-h-[min(780px,calc(100vh-48px))] w-[min(900px,calc(100vw-48px))] flex-col overflow-hidden"
          >
            <header className="shrink-0 border-b border-border px-6 pt-5 pb-4">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <Dialog.Title className="break-words font-semibold text-title2 text-fg">
                      {loop.name}
                    </Dialog.Title>
                    <OverviewStatusChip loop={loop} />
                  </div>
                  <Dialog.Description className="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-callout text-fg-muted">
                    <Tooltip label={loop.workspace || undefined}>
                      <span className="inline-flex min-w-0 items-center gap-1.5">
                        <Folder size={12} className="shrink-0 text-fg-faint" />
                        <span className="truncate">{projectName(loop.workspace) || "No project"}</span>
                      </span>
                    </Tooltip>
                    <span className="inline-flex shrink-0 items-center gap-1.5">
                      <Server size={12} className="shrink-0 text-fg-faint" />
                      <span>{machine}</span>
                    </span>
                    <span className="inline-flex shrink-0 items-center gap-1.5">
                      <Clock size={12} className="shrink-0 text-fg-faint" />
                      <span>
                        {scheduleInWords(loop.schedule)}
                        {timezone ? ` (${timezone})` : ""}
                      </span>
                    </span>
                  </Dialog.Description>
                </div>
                <Dialog.Close asChild>
                  <button
                    type="button"
                    aria-label="Close"
                    className="-mt-1 -mr-2 inline-flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
                  >
                    <X size={14} />
                  </button>
                </Dialog.Close>
              </div>

              <div className="mt-4 flex flex-wrap items-center gap-2">
                {running ? (
                  <Pill
                    danger
                    size="sm"
                    onClick={() => act("Stopped run", () => cancelLoopRun(loop.id, loop.host))}
                  >
                    <SquareStop size={12} />
                    Stop run
                  </Pill>
                ) : (
                  <Pill
                    prominent
                    size="sm"
                    onClick={() => act("Started a run", () => runLoopNow(loop.id, loop.host))}
                  >
                    <Play size={12} />
                    Run now
                  </Pill>
                )}
                {(!reason || latest?.signedOut) && (
                  <Pill size="sm" onClick={togglePause}>
                    {loop.paused ? <Play size={12} /> : <Pause size={12} />}
                    {loop.paused ? "Resume" : "Pause"}
                  </Pill>
                )}
                <Pill size="sm" onClick={onEdit}>
                  <Pencil size={12} />
                  Edit
                </Pill>
                <Pill size="sm" quiet onClick={onDuplicate}>
                  <Copy size={12} />
                  Duplicate
                </Pill>
                {latestThread && (
                  <Pill
                    size="sm"
                    quiet
                    onClick={() => {
                      onClose();
                      selectThread(latestThread);
                    }}
                  >
                    <ArrowUpRight size={12} />
                    Open latest run
                  </Pill>
                )}
              </div>

              {reason && (
                <div className="mt-3">
                  <Callout
                    tone="danger"
                    action={
                      latest?.signedOut ? (
                        <Pill
                          size="sm"
                          onClick={() => {
                            onClose();
                            openSettings("agents");
                          }}
                        >
                          Sign in
                        </Pill>
                      ) : (
                        <Pill size="sm" onClick={togglePause}>
                          <Play size={12} />
                          Resume
                        </Pill>
                      )
                    }
                  >
                    {reason}
                  </Callout>
                </div>
              )}
            </header>

            <div className="grid min-h-0 flex-1 grid-cols-1 divide-y divide-border overflow-y-auto min-[760px]:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)] min-[760px]:divide-x min-[760px]:divide-y-0 min-[760px]:overflow-hidden">
              <div className="flex min-h-0 flex-col gap-4 p-6 min-[760px]:overflow-y-auto">
                <section aria-label="Next run" className="rounded-xl bg-fill-subtle p-4">
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-medium text-caption text-fg-muted">Next run</span>
                    {loop.webhook && (
                      <span className="inline-flex items-center gap-1 rounded-full bg-fill px-2 py-0.5 text-caption text-fg-muted">
                        <Webhook size={10} />
                        Webhook
                      </span>
                    )}
                  </div>
                  <p className="mt-1 font-semibold text-title3 text-fg">
                    {running
                      ? "Running now"
                      : next !== undefined
                        ? FULL_RUN_FORMAT.format(next)
                        : nextRunText(loop, now)}
                  </p>
                  <p className="mt-0.5 text-caption text-fg-muted">
                    {running
                      ? "A run is in progress."
                      : next !== undefined
                        ? `Starts ${nextRunText(loop, now)} · ${scheduleInWords(loop.schedule)}`
                        : scheduleInWords(loop.schedule)}
                  </p>
                </section>

                <section aria-label="Prompt" className="flex flex-col gap-1.5">
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="font-medium text-caption text-fg-muted">Prompt</h3>
                    {loop.prompt && <CopyButton text={loop.prompt} label="Copy prompt" />}
                  </div>
                  <div className="max-h-48 overflow-y-auto rounded-xl bg-fill-subtle p-3 text-callout text-fg whitespace-pre-wrap break-words">
                    {loop.prompt || <span className="text-fg-faint">No prompt configured.</span>}
                  </div>
                </section>

                {loop.script && (
                  <section aria-label="Pre-check script" className="flex flex-col gap-1.5">
                    <div className="flex items-center justify-between gap-2">
                      <span className="inline-flex items-center gap-1.5 font-medium text-caption text-fg-muted">
                        <Terminal size={12} />
                        Pre-check script
                      </span>
                      <CopyButton text={loop.script} label="Copy script" />
                    </div>
                    <pre className="max-h-32 overflow-y-auto rounded-xl bg-code-bg p-3 font-mono text-caption text-fg whitespace-pre-wrap break-words">
                      {loop.script}
                    </pre>
                  </section>
                )}

                <section aria-label="Configuration" className="rounded-xl bg-fill-subtle px-4 py-2">
                  <dl className="divide-y divide-border">
                    <ConfigRow label="Agent">
                      <span className="break-words">{agent || "Default harness"}</span>
                    </ConfigRow>
                    <ConfigRow label="Project">
                      <Tooltip label={loop.workspace || undefined}>
                        <span className="block truncate font-mono text-caption text-fg-muted">
                          {loop.workspace || "—"}
                        </span>
                      </Tooltip>
                    </ConfigRow>
                    <ConfigRow label="Machine">{machine}</ConfigRow>
                    <ConfigRow label="Access">
                      {loop.readOnly ? (
                        <span className="inline-flex items-center gap-1.5 text-fg-muted">
                          <Shield size={12} className="shrink-0" />
                          Read-only
                        </span>
                      ) : (
                        <span className="inline-flex items-center gap-1.5 text-warning">
                          <ShieldAlert size={12} className="shrink-0" />
                          Full access
                        </span>
                      )}
                    </ConfigRow>
                    <ConfigRow label="Runs in">
                      {loop.threadId ? "Existing thread" : "New thread each run"}
                    </ConfigRow>
                    <ConfigRow label="Triggers">{triggerLabel}</ConfigRow>
                  </dl>
                </section>
              </div>

              <div className="flex min-h-0 flex-col gap-4 p-6 min-[760px]:overflow-y-auto">
                <section
                  aria-label="Run statistics"
                  className="flex flex-col gap-3 rounded-xl bg-fill-subtle p-4"
                >
                  <dl className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                    <div className="flex flex-col-reverse">
                      <dt className="text-caption text-fg-muted">Runs</dt>
                      <dd className="tabular font-semibold text-title2 text-fg">{loop.runs.length}</dd>
                    </div>
                    <div className="flex flex-col-reverse">
                      <dt className="text-caption text-fg-muted">Succeeded</dt>
                      <dd
                        className={cx(
                          "tabular font-semibold text-title2",
                          succeededCount > 0 ? "text-success" : "text-fg",
                        )}
                      >
                        {succeededCount}
                      </dd>
                    </div>
                    <div className="flex flex-col-reverse">
                      <dt className="text-caption text-fg-muted">Failed</dt>
                      <dd
                        className={cx(
                          "tabular font-semibold text-title2",
                          failedCount > 0 ? "text-danger" : "text-fg",
                        )}
                      >
                        {failedCount}
                      </dd>
                    </div>
                    <div className="flex flex-col-reverse">
                      <dt className="text-caption text-fg-muted">Avg time</dt>
                      <dd className="tabular font-semibold text-title2 text-fg">{avgDuration ?? "—"}</dd>
                    </div>
                  </dl>
                  {loop.runs.length > 0 && (
                    <div className="border-t border-border pt-3">
                      <RunTimelineStrip runs={loop.runs} now={now} />
                    </div>
                  )}
                </section>

                <section aria-label="Run history" className="flex min-h-0 flex-1 flex-col gap-2">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <h3 className="font-semibold text-body text-fg">
                      Run history
                      {loop.runs.length > 0 && (
                        <span className="ml-1.5 font-normal text-caption text-fg-faint">
                          {visibleRuns.length}
                        </span>
                      )}
                    </h3>
                    {showFilter && (
                      <Segments
                        label="Filter runs"
                        value={effectiveFilter}
                        options={[
                          { value: "all", label: `All ${loop.runs.length}` },
                          { value: "failed", label: `Failed ${failedCount}` },
                        ]}
                        onChange={setRunFilter}
                      />
                    )}
                  </div>
                  {visibleRuns.length > 0 ? (
                    <RunList
                      detailed
                      runs={visibleRuns}
                      now={now}
                      threadOf={(run) => runThreadKey(loop, run, machines, localName)}
                      onOpenThread={onClose}
                    />
                  ) : (
                    <div className="flex flex-col items-center gap-2 rounded-xl bg-fill-subtle px-4 py-8 text-center">
                      <span className="inline-flex size-8 items-center justify-center rounded-full bg-fill text-fg-muted">
                        <History size={16} />
                      </span>
                      <p className="font-medium text-callout text-fg">No runs recorded yet</p>
                      <p className="max-w-xs text-caption text-fg-muted">
                        {next !== undefined
                          ? `Scheduled for ${FULL_RUN_FORMAT.format(next)}. Use Run now to try it right away.`
                          : "Use Run now above whenever you want to run this job."}
                      </p>
                      {!running && (
                        <div className="pt-1">
                          <Pill
                            size="sm"
                            onClick={() => act("Started a run", () => runLoopNow(loop.id, loop.host))}
                          >
                            <Play size={12} />
                            Run now
                          </Pill>
                        </div>
                      )}
                    </div>
                  )}
                </section>
              </div>
            </div>

            <footer className="flex shrink-0 items-center justify-between gap-2 border-t border-border px-6 py-3">
              <Pill danger size="sm" onClick={onDelete}>
                <Trash2 size={12} />
                Delete
              </Pill>
              <Dialog.Close asChild>
                <Pill>Done</Pill>
              </Dialog.Close>
            </footer>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
