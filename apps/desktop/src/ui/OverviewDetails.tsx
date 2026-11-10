// The overview's Details section, folded away by default: the facts and links
// that are worth keeping but rarely the reason the panel was opened — the
// commands allowed without asking, the threads this one came from and started,
// its subagents, its plan, a terminal on its folder, and the changes it made
// (ui/ThreadDetails.tsx draws the sections; the rows here are the old details
// card's, in the order the overview puts them).

import {
  Clock,
  CornerDownRight,
  FileDiff,
  GitFork,
  ListChecks,
  MessageSquareQuote,
  SquareTerminal,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo } from "react";
import type { SubagentInfo } from "../acp/rustcode";
import { delegateParent, displayPath, modelName } from "../acp/types";
import { type GitChanges, openTerminalAt } from "../lib/native";
import { branchesOf, branchLine, originLine, originOf } from "../state/branches";
import { contextPercent, planProgress, type SubagentLine, subagentLines } from "../state/details";
import { activeGrants, grantRemaining } from "../state/grants";
import { stateOf } from "../state/host";
import { threadLineage } from "../state/lineage";
import { isActive, LINK_LABEL, type Link } from "../state/orchestration";
import {
  peekThread,
  refreshGrants,
  revokeGrant,
  selectThread,
  showPanel,
  statusOf,
  toast,
  useApp,
} from "../state/store";
import { formatDuration, formatTokens, shortAge, type ThreadState } from "../state/thread";
import { AgentRow, AgentTip, LINK_AGENT_STATE, useNow } from "./AgentViews";
import { OverviewSection } from "./OverviewSection";
import { cx, Pill } from "./primitives";
import { DiffCount } from "./Rows";
import { StatusChip } from "./Status";

const NO_AGENTS: SubagentInfo[] = [];

/** One thing the section offers: an icon, what it is, and what is on its right. */
function CardRow({
  icon,
  label,
  right,
  onClick,
  title,
}: {
  icon?: ReactNode;
  label: ReactNode;
  right?: ReactNode;
  onClick?: () => void;
  title?: string;
}) {
  const body = (
    <>
      {icon}
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {right}
    </>
  );
  const shape = "flex h-8 w-full items-center gap-2 rounded-md px-2 text-body";
  return onClick ? (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={cx(shape, "font-medium text-fg enabled:hover:bg-fill-subtle")}
    >
      {body}
    </button>
  ) : (
    <div title={title} className={cx(shape, "text-fg-muted")}>
      {body}
    </div>
  );
}

/** A fact the section only states: its name on the left, its value at the right. */
function Fact({ label, value, title }: { label: ReactNode; value: ReactNode; title?: string }) {
  return (
    <div title={title} className="flex h-8 w-full items-center gap-2 px-2 text-body">
      <span className="min-w-0 shrink-0 text-fg-muted">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right text-fg tabular">{value}</span>
    </div>
  );
}

/** A group of rows inside the section, with its own quiet heading. */
function Group({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) {
  return (
    <div className="pt-1.5 pb-0.5">
      <h3 className="px-2 pt-1.5 pb-1 font-medium text-caption text-fg-faint">{title}</h3>
      {hint && <p className="px-2 pb-1 text-caption text-fg-faint">{hint}</p>}
      {children}
    </div>
  );
}

/**
 * What a group of the section's threads holds, in the words the app settles on
 * (docs/terminology.md): a thread this one *started* had a task handed to it (the
 * Delegates section lists those), a thread it *messaged* was already running. The
 * hint says which, once per group, so the heading stays short at a narrow pane.
 */
function GroupHeading({ label, count, hint }: { label: string; count: number; hint: string }) {
  return (
    <div className="px-2 pt-1.5 pb-1">
      <div className="flex items-baseline gap-1.5 text-caption">
        <span className="font-medium text-fg-muted">{label}</span>
        <span className="text-fg-faint tabular">
          {count} {count === 1 ? "thread" : "threads"}
        </span>
      </div>
      <p className="text-caption text-fg-faint">{hint}</p>
    </div>
  );
}

function LinkRow({
  link,
  id,
  now,
  relationship,
  count = 1,
}: {
  link: Link;
  /** The thread the row opens: the other end of the link. */
  id: string;
  now: number;
  /** `Thread it started`, `Thread that started this one`. */
  relationship: string;
  /** How many links this one row stands for: more than one message to the same thread. */
  count?: number;
}) {
  const state = useApp((s) => stateOf(s, link));
  const opened = useApp((s) => s.sessions.find((x) => x.id === id));
  const threadStatus = useApp((s) => statusOf(s, id));
  // A deleted parent or target is a fact with a tip, not a dead click.
  const openable = useApp((s) => s.sessions.some((x) => x.id === id));
  const harness = id === link.target ? (link.harness ?? opened?.harness) : opened?.harness;
  const harnessName = useApp((s) =>
    harness ? (s.harnesses?.harnesses.find((h) => h.id === harness)?.name ?? harness) : undefined,
  );
  const active = isActive(state);
  const started = id === link.target;
  const title = (started && opened?.title) || (started ? link.targetTitle : link.orchestratorTitle);
  // The elapsed time belongs to the row's own thread: on the parent row it would be the child's age.
  const end = active ? now : link.settledAt;
  const elapsedMs =
    started && end !== undefined && end - link.createdAt >= 1000 ? end - link.createdAt : undefined;
  const elapsed = elapsedMs === undefined ? undefined : formatDuration(elapsedMs);
  // The row ends the way the sidebar's row for that thread ends: a review mark when it waits for
  // one, then its age in the same short form. How long the link has run is in the hover tip.
  const age = shortAge(opened?.updatedAt, now);
  const review = threadStatus === "review";
  const meta = [harnessName, modelName(opened?.model), count > 1 ? `${count} messages` : undefined]
    .filter(Boolean)
    .join(" · ");
  const status = LINK_LABEL[state];
  // A row with nowhere to open exposes its relation and prompt in its accessible name instead.
  const description = openable
    ? undefined
    : `${title}. ${relationship}. ${status}${elapsed ? `, ${elapsed}` : ""}. ${link.message.split("\n")[0]?.slice(0, 200) ?? ""}`;
  return (
    <AgentRow
      harness={harness}
      state={LINK_AGENT_STATE[state]}
      status={status}
      description={description}
      title={title}
      elapsed={
        (review || age) && (
          <span className="flex items-center gap-1.5">
            {review && <StatusChip status={threadStatus} compact />}
            {age && <span className="tabular">{age}</span>}
          </span>
        )
      }
      label={`Open ${title}`}
      tooltip={
        <AgentTip
          title={title}
          meta={meta || undefined}
          relationship={relationship}
          state={LINK_AGENT_STATE[state]}
          status={status}
          elapsed={elapsed}
          task={link.message}
        />
      }
      onClick={openable ? () => selectThread(id) : undefined}
    />
  );
}

function SubagentRow({ line, now, harness }: { line: SubagentLine; now: number; harness?: string }) {
  const end = line.state === "working" ? now : line.endedAt;
  const elapsedMs = line.startedAt && end ? end - line.startedAt : undefined;
  const elapsed =
    elapsedMs !== undefined && elapsedMs >= 1000
      ? formatDuration(Math.floor(elapsedMs / 1000) * 1000)
      : undefined;
  const description = `${line.name}, subagent, ${line.state}${elapsed ? `, ${elapsed}` : ""}. ${line.task}`;
  return (
    <AgentRow
      harness={harness}
      state={line.state}
      title={line.name}
      description={description}
      detail={line.task}
      elapsed={elapsed}
      tooltip={
        <AgentTip
          title={line.name}
          relationship="Subagent"
          state={line.state}
          elapsed={elapsed}
          task={line.task}
        />
      }
    />
  );
}

export function DetailsSection({
  thread,
  changes,
  folded,
  onToggle,
}: {
  thread: ThreadState;
  /** The thread's working tree against HEAD, read when its turn ended. */
  changes?: GitChanges | null;
  folded: boolean;
  onToggle: () => void;
}) {
  const id = thread.id;
  const agents = useApp((s) => s.agents[id] ?? NO_AGENTS);
  const book = useApp((s) => s.links);
  // Only an agent's parent is lineage here; a fork's or side chat's has its own rows below.
  const parentMeta = useApp((s) => delegateParent(s.sessions.find((x) => x.id === id)?.parent));
  // The metadata can name a parent whose thread is gone: then the row is a fact, not a link.
  const parentListed = useApp((s) => {
    const parent = delegateParent(s.sessions.find((x) => x.id === id)?.parent);
    return !!parent?.id && s.sessions.some((x) => x.id === parent.id);
  });
  const sessions = useApp((s) => s.sessions);
  const origin = useMemo(() => originOf(sessions, id), [sessions, id]);
  const branches = useMemo(() => branchesOf(sessions, id), [sessions, id]);
  const grants = useApp((s) => s.grants);
  // "Allow for 1 hour" grants are the engine's user config, so this panel lists
  // what is still in force; it is read on open and after a revoke.
  // The grants are the engine's own config, so they are read when the section is drawn.
  useEffect(() => {
    void refreshGrants();
  }, []);

  const subagents = useMemo(() => subagentLines(thread.items, agents), [thread.items, agents]);
  const working = subagents.some((s) => s.state === "working");
  // A working link's elapsed time ticks too, not only a subagent's.
  const linkActive = useMemo(
    () => book.links.some((l) => (l.orchestrator === id || l.target === id) && isActive(l.state)),
    [book.links, id],
  );
  const now = useNow(working || linkActive || grants.length > 0);
  const allowed = activeGrants(grants, now);

  const lineage = threadLineage(id, book.links, parentMeta);
  const percent = contextPercent(thread.usage);
  const plan = planProgress(thread.plan);
  const step = thread.plan.find((p) => p.status === "in_progress")?.content;
  const totals = changes?.files.reduce(
    (t, f) => ({ added: t.added + f.added, removed: t.removed + f.removed }),
    { added: 0, removed: 0 },
  );

  return (
    <OverviewSection id="details" title="Details" folded={folded} onToggle={onToggle}>
      {allowed.length > 0 && (
        <Group title="Allowed" hint="Commands run without asking, until their hour is up.">
          {allowed.map((grant) => (
            <div key={grant.id} className="flex h-8 w-full items-center gap-2 rounded-md px-2">
              <Clock size={16} className="shrink-0 text-fg-muted" />
              <span className="min-w-0 flex-1 truncate font-mono text-callout text-fg" title={grant.prefix}>
                {grant.prefix}
              </span>
              <span className="shrink-0 text-caption text-fg-muted tabular">
                for {grantRemaining(grant, now)}
              </span>
              <Pill size="sm" quiet onClick={() => void revokeGrant(grant.id)}>
                Revoke
              </Pill>
            </div>
          ))}
        </Group>
      )}

      {(origin || branches.length > 0) && (
        <Group title="Forks and side chats" hint="Threads opened from a thread, not handed to an agent.">
          {origin && (
            <CardRow
              icon={<GitFork size={16} className="shrink-0 text-fg-muted" />}
              label={`${originLine(origin).label} ${origin.title}`}
              title={origin.listed ? "Open the thread it came from" : "That thread is no longer in the list"}
              onClick={origin.listed && origin.id ? () => selectThread(origin.id!) : undefined}
            />
          )}
          {branches.map((branch) => (
            <CardRow
              key={branch.id}
              icon={
                branch.kind === "fork" ? (
                  <GitFork size={16} className="shrink-0 text-fg-muted" />
                ) : (
                  <MessageSquareQuote size={16} className="shrink-0 text-fg-muted" />
                )
              }
              label={`${branchLine(branch).label} ${branch.title}`}
              title={branch.kind === "fork" ? "Open the fork" : "Open the side chat beside this thread"}
              onClick={() => (branch.kind === "fork" ? selectThread(branch.id) : peekThread(branch.id))}
            />
          ))}
        </Group>
      )}

      {(lineage.parent || lineage.messagedBy || lineage.messaged.length > 0) && (
        <Group title="Threads">
          {lineage.parent && (
            <>
              <div className="flex items-center gap-1.5 px-2 pt-1.5 pb-1 text-caption text-fg-faint">
                <CornerDownRight size={11} /> {lineage.parent.live ? "Working for" : "Started from"}
              </div>
              {lineage.parent.link ? (
                <LinkRow
                  link={lineage.parent.link}
                  id={lineage.parent.id ?? lineage.parent.link.orchestrator}
                  now={now}
                  relationship={
                    lineage.parent.started ? "Thread that started this one" : "Thread that messaged this one"
                  }
                />
              ) : (
                <CardRow
                  icon={<CornerDownRight size={16} className="shrink-0 text-fg-muted" />}
                  label={lineage.parent.title}
                  title="The thread that started this one"
                  onClick={
                    parentListed && lineage.parent.id ? () => selectThread(lineage.parent!.id!) : undefined
                  }
                />
              )}
            </>
          )}
          {lineage.messagedBy?.link && (
            <>
              <div className="flex items-center gap-1.5 px-2 pt-1.5 pb-1 text-caption text-fg-faint">
                <CornerDownRight size={11} /> Messaged by
              </div>
              <LinkRow
                link={lineage.messagedBy.link}
                id={lineage.messagedBy.id ?? lineage.messagedBy.link.orchestrator}
                now={now}
                relationship="Thread that messaged this one"
              />
            </>
          )}
          {lineage.messaged.length > 0 && (
            <>
              <GroupHeading
                label="Messaged"
                count={lineage.messaged.length}
                hint="A message to a thread already running."
              />
              {lineage.messaged.map((group) => (
                <LinkRow
                  key={group.links[0]!.id}
                  link={group.link}
                  id={group.link.target}
                  now={now}
                  relationship="Thread it messaged"
                  count={group.links.length}
                />
              ))}
            </>
          )}
        </Group>
      )}

      {subagents.length > 0 && (
        <Group title={`Subagents · ${subagents.length}`} hint="Helpers inside this thread, not threads.">
          {subagents.map((line) => (
            <SubagentRow key={line.key} line={line} now={now} harness={thread.harness?.id} />
          ))}
        </Group>
      )}
      <div className="pt-1">
        <CardRow
          icon={<SquareTerminal size={16} className="shrink-0 text-fg-muted" />}
          label="Open in Terminal"
          title={`A terminal in ${displayPath(thread.cwd)}`}
          onClick={() => void openTerminalAt(thread.cwd).catch((e) => toast(String(e), "error"))}
        />
        {changes && (
          <CardRow
            icon={<FileDiff size={16} className="shrink-0 text-fg-muted" />}
            label="Changes"
            title="Open the changes"
            onClick={() => showPanel("changes")}
            right={
              changes.files.length === 0 ? (
                <span className="shrink-0 text-callout text-fg-muted">None</span>
              ) : (
                totals && (
                  <span className="shrink-0">
                    <DiffCount added={totals.added} removed={totals.removed} />
                  </span>
                )
              )
            }
          />
        )}
        {plan && (
          <Fact
            title={step ?? plan}
            label={
              <span className="flex items-center gap-1.5">
                <ListChecks size={16} className="shrink-0" /> Plan
              </span>
            }
            value={plan}
          />
        )}
        {percent !== undefined && thread.usage && (
          <Fact label="Context" value={`${percent}% of ${formatTokens(thread.usage.size)}`} />
        )}
      </div>
      <div className="h-1" />
    </OverviewSection>
  );
}
