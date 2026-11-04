// The Details view of the right panel: one place for what a thread is. Which
// harness and model run it, where it works, what it changed, how it relates
// to other threads, and how far along it is. A section with nothing to say
// is left out.

import { CircleCheck, CircleX, Copy, CornerDownRight, FolderOpen, GitBranch, ListChecks } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import type { SubagentInfo } from "../acp/rustcode";
import { displayPath, effortLabel, modelName } from "../acp/types";
import { type GitChanges, gitChanges, revealPath } from "../lib/native";
import { contextPercent, planProgress, type SubagentLine, subagentLines } from "../state/details";
import { linksOf, stateOf } from "../state/host";
import { LINK_LABEL, type Link } from "../state/orchestration";
import { selectThread, showPanel, useApp, worktreeOf } from "../state/store";
import { formatDuration, formatTokens } from "../state/thread";
import { useNow } from "./AgentViews";
import { HarnessIcon } from "./HarnessIcon";
import { LinkStateIcon } from "./Links";
import { copyText, cx, Orb, Tooltip } from "./primitives";

const NO_AGENTS: SubagentInfo[] = [];

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="px-1.5 pt-2 pb-2.5 [&+&]:border-border [&+&]:border-t">
      <h3 className="px-2 pb-1 font-medium text-fg-faint text-sub">{title}</h3>
      {children}
    </section>
  );
}

/** One fact: its label, its value, and a click when there is somewhere to go. */
function Line({
  label,
  children,
  onClick,
  title,
}: {
  label: ReactNode;
  children?: ReactNode;
  onClick?: () => void;
  title?: string;
}) {
  const body = (
    <>
      <span className="flex min-w-0 shrink-0 items-center gap-1.5 text-fg-muted">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right">{children}</span>
    </>
  );
  const shape = "flex min-h-7 w-full items-center gap-3 rounded-lg px-2 text-callout";
  return onClick ? (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={cx(shape, "text-left hover:bg-fill-subtle")}
    >
      {body}
    </button>
  ) : (
    <div title={title} className={shape}>
      {body}
    </div>
  );
}

function LinkLine({ link, id }: { link: Link; id: string }) {
  const state = useApp((s) => stateOf(s, link));
  return (
    <Line
      onClick={() => selectThread(id)}
      label={
        <span className="flex min-w-0 items-center gap-1.5">
          <LinkStateIcon state={state} />
          <span className="max-w-[220px] truncate text-fg">
            {id === link.target ? link.targetTitle : link.orchestratorTitle}
          </span>
        </span>
      }
    >
      <span className="text-fg-muted">{LINK_LABEL[state]}</span>
    </Line>
  );
}

function SubagentRowLine({ line, now }: { line: SubagentLine; now: number }) {
  const end = line.state === "working" ? now : line.endedAt;
  const elapsed = line.startedAt && end ? end - line.startedAt : undefined;
  return (
    <div className="flex min-h-7 items-center gap-2 rounded-lg px-2 text-callout">
      <span className="flex w-3.5 shrink-0 justify-center">
        {line.state === "working" ? (
          <Orb />
        ) : line.state === "failed" ? (
          <CircleX size={12} className="text-danger" />
        ) : (
          <CircleCheck size={12} className="text-success" />
        )}
      </span>
      <span className="shrink-0 font-medium">{line.name}</span>
      <span className="min-w-0 flex-1 truncate text-fg-muted">{line.task}</span>
      {elapsed !== undefined && elapsed >= 1000 && (
        <span className="shrink-0 text-caption text-fg-faint tabular-nums">
          {formatDuration(Math.floor(elapsed / 1000) * 1000)}
        </span>
      )}
    </div>
  );
}

function useChanges(cwd: string | undefined, running: boolean) {
  const [changes, setChanges] = useState<GitChanges | null>(null);
  // Runs again when `running` flips, so a finished turn counts its edits.
  useEffect(() => {
    if (!cwd || running) return;
    let alive = true;
    void gitChanges(cwd).then(
      (result) => alive && setChanges(result),
      () => alive && setChanges(null),
    );
    return () => {
      alive = false;
    };
  }, [cwd, running]);
  return changes;
}

export function ThreadDetails() {
  const id = useApp((s) => s.selectedId);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const worktree = useApp((s) => (s.selectedId ? worktreeOf(s, s.selectedId) : undefined));
  const harnessName = useApp((s) =>
    thread?.harness
      ? (s.harnesses?.harnesses.find((h) => h.id === thread.harness?.id)?.name ?? thread.harness.id)
      : undefined,
  );
  const agents = useApp((s) => (s.selectedId ? (s.agents[s.selectedId] ?? NO_AGENTS) : NO_AGENTS));
  const book = useApp((s) => s.links);
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const changes = useChanges(thread?.cwd, !!thread?.running);

  const subagents = useMemo(() => subagentLines(thread?.items ?? [], agents), [thread?.items, agents]);
  const working = subagents.some((s) => s.state === "working");
  const now = useNow(working);

  if (!id || !thread) {
    return (
      <div className="flex flex-1 items-center justify-center px-6 text-center text-callout text-fg-muted">
        Open a thread to see its details.
      </div>
    );
  }

  const { driving, drivenBy } = linksOf(book, id);
  const choice = (option: string) => {
    const o = thread.configOptions.find((x) => x.id === option);
    const name = o?.choices.find((c) => c.value === o.current)?.name;
    return { name: name !== o?.current ? name : undefined, value: o?.current };
  };
  const picked = choice("model");
  const model = picked.name ?? modelName(picked.value);
  const pickedEffort = choice("effort");
  const effort = pickedEffort.name ?? (pickedEffort.value ? effortLabel(pickedEffort.value) : undefined);
  const percent = contextPercent(thread.usage);
  const plan = planProgress(thread.plan);
  const totals = changes?.files.reduce(
    (t, f) => ({ added: t.added + f.added, removed: t.removed + f.removed }),
    { added: 0, removed: 0 },
  );

  return (
    <div className="min-h-0 flex-1 overflow-y-auto pb-2">
      {(harnessName || model) && (
        <Section title="Harness">
          {harnessName && (
            <Line
              label={
                <>
                  <HarnessIcon id={thread.harness!.id} size={13} /> Runs on
                </>
              }
            >
              {harnessName}
            </Line>
          )}
          {model && <Line label="Model">{model}</Line>}
          {effort && <Line label="Effort">{effort}</Line>}
        </Section>
      )}

      <Section title="Workspace">
        <Line
          label={
            <>
              <FolderOpen size={13} /> Folder
            </>
          }
          title={`Show ${thread.cwd} in the file manager`}
          onClick={() => revealPath(thread.cwd)}
        >
          {displayPath(thread.cwd)}
        </Line>
        {worktree && (
          <Line
            label={
              <>
                <GitBranch size={13} /> Worktree branch
              </>
            }
            title="Copy the branch name"
            onClick={() => void copyText(worktree.branch)}
          >
            <span className="inline-flex max-w-full items-center gap-1.5">
              <span className="truncate">{worktree.branch}</span>
              <Copy size={11} className="shrink-0 text-fg-faint" />
            </span>
          </Line>
        )}
        {worktree?.base && <Line label="Based on">{worktree.base}</Line>}
        {totals && changes && (
          <Line label="Uncommitted" onClick={() => showPanel("changes")} title="Open the changes">
            {changes.files.length === 0 ? (
              <span className="text-fg-muted">None</span>
            ) : (
              <span className="tabular-nums">
                {changes.files.length} file{changes.files.length === 1 ? "" : "s"}{" "}
                <span className="text-success">+{totals.added}</span>{" "}
                <span className="text-danger">−{totals.removed}</span>
              </span>
            )}
          </Line>
        )}
        {pullsOn && (
          <Line label="Pull requests" onClick={() => showPanel("pulls")} title="Open the pull requests" />
        )}
      </Section>

      {(drivenBy || driving.length > 0) && (
        <Section title="Threads">
          {drivenBy && (
            <>
              <div className="flex items-center gap-1.5 px-2 pt-0.5 text-caption text-fg-faint">
                <CornerDownRight size={11} /> Working for
              </div>
              <LinkLine link={drivenBy} id={drivenBy.orchestrator} />
            </>
          )}
          {driving.length > 0 && (
            <>
              <div className="px-2 pt-0.5 text-caption text-fg-faint">Started or messaged</div>
              {driving.map((l) => (
                <LinkLine key={l.id} link={l} id={l.target} />
              ))}
            </>
          )}
        </Section>
      )}

      {subagents.length > 0 && (
        <Section title={`Subagents · ${subagents.length}`}>
          {subagents.map((line) => (
            <SubagentRowLine key={line.key} line={line} now={now} />
          ))}
        </Section>
      )}

      {plan && (
        <Section title="Plan">
          <Tooltip label={thread.plan.find((p) => p.status === "in_progress")?.content ?? plan}>
            <div>
              <Line
                label={
                  <>
                    <ListChecks size={13} /> Steps
                  </>
                }
              >
                {plan}
              </Line>
            </div>
          </Tooltip>
        </Section>
      )}

      {thread.usage && (
        <Section title="Usage">
          {percent !== undefined && (
            <Line label="Context window">
              <span className="tabular-nums">
                {percent}% of {formatTokens(thread.usage.size)}
              </span>
            </Line>
          )}
          {thread.usage.inputTokens !== undefined && (
            <Line label="Tokens in, out">
              <span className="tabular-nums">
                {formatTokens(thread.usage.inputTokens)}, {formatTokens(thread.usage.outputTokens ?? 0)}
              </span>
            </Line>
          )}
          {thread.usage.cost !== undefined && (
            <Line label="Cost">
              <span className="tabular-nums">
                {thread.usage.cost.toFixed(2)} {thread.usage.currency ?? "USD"}
              </span>
            </Line>
          )}
        </Section>
      )}
    </div>
  );
}
