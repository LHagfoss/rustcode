// Orchestration calls about scheduled work: scheduling a task, listing, deleting and running one now,
// and the pull requests a thread opened. Split out of state/host.ts.

import { type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import {
  type DelegatePull,
  parsePullError,
  parsePullList,
  parsePullState,
  pullErrorTitle,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import {
  activityFrom,
  book,
  localMachine,
  type MachineRef,
  machineFields,
  manageTarget,
  permitted,
  projectOnMachine,
  requestOn,
  ruleThread,
  targetMachine,
} from "./host";
import { projectPath } from "./hostProjects";
import {
  activityOf,
  type Failure,
  failure,
  failureJson,
  isFailure,
  MAX_DELEGATES,
  MAX_PULL_REQUESTS,
  type PullStateChoice,
  projectOf,
  type RuleThread,
} from "./orchestration";
import { rememberPullSummaries } from "./pullQueries";
import { errorText, request, useApp } from "./store";
import { splitThreadKey } from "./threadKeys";

/** The user's IANA time zone, which a cron with none is read in. */
function userTimeZone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

/** How a schedule reads in the approval card. */
function scheduleText(schedule: {
  kind: "cron" | "at";
  expression?: string;
  timezone?: string;
  at?: string;
}): string {
  if (schedule.kind === "cron") {
    return `every cron ${schedule.expression}${schedule.timezone ? ` (${schedule.timezone})` : ""}`;
  }
  return `once at ${schedule.at}`;
}

/** The engine's scheduler call, answered as a failure the agent can read. */
async function askScheduler(method: string, params: JsonObject, timeoutMs = 30_000): Promise<JsonObject> {
  try {
    return await request<JsonObject>(method, params, timeoutMs);
  } catch (error) {
    if (error instanceof RpcError) {
      return failureJson(failure("target_unavailable", error.message));
    }
    return failureJson(failure("target_unavailable", `the scheduler did not answer: ${errorText(error)}`));
  }
}

/** `schedule_task`: a prompt the daemon runs on its schedule, with no thread open. */
export async function scheduleTask(
  caller: RuleThread,
  call: {
    prompt: string;
    schedule: { kind: "cron"; expression: string; timezone?: string } | { kind: "at"; at: string };
    project?: string;
    name?: string;
    modelProfile?: string;
  },
): Promise<JsonObject> {
  const workspace = projectPath(call.project || projectOf(caller) || useApp.getState().projectCwd);
  if (!workspace) {
    return failureJson(failure("failed", "this thread has no project folder; pass project"));
  }
  // A cron is read in the user's own zone unless the call named one: the app knows it, the engine only guesses.
  const timezone = call.schedule.kind === "cron" ? call.schedule.timezone || userTimeZone() : undefined;
  const schedule =
    call.schedule.kind === "cron"
      ? {
          kind: "cron" as const,
          expression: call.schedule.expression,
          ...(timezone ? { timezone } : {}),
        }
      : call.schedule;
  const title = call.name || call.prompt.trim().split("\n")[0]?.slice(0, 60) || "this task";
  const no = await permitted(caller, "schedule:new", {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: `It would add “${title}” to the scheduler: ${scheduleText(schedule)}, running in ${workspace} with no thread open. The daemon must be running for it to fire.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledCreate, {
    prompt: call.prompt,
    workspace,
    schedule,
    ...(call.name ? { name: call.name } : {}),
    ...(call.modelProfile ? { modelProfile: call.modelProfile } : {}),
  });
}

/** `list_scheduled_tasks`: what the daemon holds, and whether it is running. */
export async function listScheduledTasks(): Promise<JsonObject> {
  return askScheduler(Method.scheduledList, {}, 20_000);
}

/**
 * What the list says a task is called and where it runs, so a card names it
 * rather than a `task-1723…` id. The daemon may not answer: the changing call
 * then reads as it did before, still naming the id it was given.
 */
async function scheduledTaskName(id: string): Promise<string> {
  const listed = await askScheduler(Method.scheduledList, {}, 20_000);
  const tasks = Array.isArray(listed.tasks) ? listed.tasks : [];
  const task = tasks.find((entry) => obj(entry)?.id === id);
  const name = str(obj(task)?.name);
  const workspace = str(obj(task)?.workspace);
  if (!name && !workspace) return "that scheduled task";
  return `“${name ?? id}”${workspace ? `, which runs in ${workspace}` : ""}`;
}

/** `delete_scheduled_task`: the task leaves the scheduler at once. */
export async function deleteScheduledTask(caller: RuleThread, id: string): Promise<JsonObject> {
  const task = await scheduledTaskName(id);
  const no = await permitted(caller, `schedule:delete:${id}`, {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: id,
    detail: `It would delete ${task}. It stops at once, and its run history goes with it.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledDelete, { id });
}

/** `run_scheduled_task_now`: one run now, with the schedule left as it is. */
export async function runScheduledTaskNow(caller: RuleThread, id: string): Promise<JsonObject> {
  const task = await scheduledTaskName(id);
  const no = await permitted(caller, `schedule:run:${id}`, {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: id,
    detail: `It would run ${task} now, and keep its schedule. Its prompt runs with no thread open.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledRun, { id });
}

/** Why a pull request read failed, in the shape the agent reads: `{ error: { code, message } }`. */
export function pullFailure(error: { code: string; message: string }): Failure {
  return failure(
    error.code === "failed" ? "failed" : "target_unavailable",
    `${pullErrorTitle(error.code)}: ${error.message}`,
  );
}

/** `list_pull_requests`: the caller's project's pull requests, through the panel's own call. */
/**
 * `list_pull_requests`: this project's, read by `gh` on this computer, or on a
 * named machine in its checkout of this project's repository (found as
 * `start_session` finds it), with that machine's own `gh` login.
 */
export async function listPullRequests(
  caller: RuleThread,
  state: PullStateChoice,
  machine: MachineRef = localMachine(),
): Promise<JsonObject> {
  const cwd = machine.host ? await projectOnMachine(machine, caller, undefined) : projectOf(caller);
  if (isFailure(cwd)) return failureJson(cwd);
  if (!cwd) return failureJson(failure("failed", "this thread has no project folder"));
  let result: Json;
  try {
    result = await requestOn(machine.host, Method.pulls, { cwd, state, limit: MAX_PULL_REQUESTS }, 60_000);
  } catch (error) {
    return failureJson(failure("target_unavailable", `pull requests could not be read: ${errorText(error)}`));
  }
  const error = parsePullError(result);
  const unsupported = noProviderPullList(
    error,
    machine.host ? { ...machineFields(machine), project: cwd } : {},
  );
  if (unsupported) return unsupported;
  if (error) return failureJson(pullFailure(error));
  const pulls = parsePullList(result).pulls;
  // The answer also seeds the panel's header: a pull opened after the agent's
  // read draws its title, state and checks before its own detail is read.
  if (!machine.host) rememberPullSummaries(cwd, pulls);
  return {
    ...(machine.host ? { ...machineFields(machine), project: cwd } : {}),
    pull_requests: pulls.map((p) => ({
      number: p.number,
      title: p.title,
      state: p.state,
      branch: p.headRef,
      ...(p.url ? { url: p.url } : {}),
    })),
  };
}
/** An unsupported remote is an empty list with an explanation, not a failed read. */
export function noProviderPullList(
  error: { code: string } | null | undefined,
  machine: JsonObject,
): JsonObject | null {
  return error?.code === "no_provider" ? { ...machine, pull_requests: [], note: NO_PROVIDER_NOTE } : null;
}
/** What a read says when the project's remote is not on GitHub: no pull requests exist to read. */
const NO_PROVIDER_NOTE = "no pull request provider for this remote";

/** A thread's pull request and checks, read through the engine's `gh` call. */
export async function delegatePull(
  target: RuleThread,
  host = splitThreadKey(target.id).host,
): Promise<{ branch?: string; pull?: DelegatePull; error?: Failure; note?: string }> {
  const retained = [...book().links]
    .reverse()
    .find((link) => link.target === target.id && link.cleanup)?.cleanup;
  if (!target.worktree && retained) return { branch: retained.branch, pull: retained.pull };
  const cwd = projectOf(target);
  if (!cwd) return { error: failure("failed", "this thread has no project folder") };
  const worktree = target.worktree;
  try {
    // Ask for the branch the worktree is on now, not the recorded one: a
    // delegate may have renamed it, and the engine echoes the branch it was
    // asked for, so naming the recorded one would hide the rename (and its
    // pull request) from status, merge and cleanup.
    // A thread on another machine is read by `gh` there, in its own folders.
    let answer = parsePullState(
      await requestOn(host, Method.pullState, { cwd: worktree?.path ?? cwd }, 60_000),
    );
    // A remote on another forge has no pull requests to read: that is an answer, not a failure.
    if (answer.error?.code === "no_provider") return { note: NO_PROVIDER_NOTE };
    // A detached HEAD has no branch to look up, so fall back to the recorded
    // one rather than reporting no pull request at all.
    if (answer.error && worktree?.branch) {
      answer = parsePullState(
        await requestOn(host, Method.pullState, { cwd, branch: worktree.branch }, 60_000),
      );
    }
    if (answer.error?.code === "no_provider") return { note: NO_PROVIDER_NOTE };
    if (answer.error) return { error: pullFailure(answer.error) };
    return { branch: answer.branch, pull: answer.pull };
  } catch (error) {
    return {
      error: failure("target_unavailable", `the pull request could not be read: ${errorText(error)}`),
    };
  }
}

/**
 * The delegates a thread has on `host` ("" is this computer): the targets of
 * its links, newest first, one row each, capped so the gh reads stay well
 * inside the request timeout.
 */
function delegateTargets(caller: RuleThread, host: string): { targets: RuleThread[]; omitted: number } {
  const ids = new Set(
    book()
      .links.filter((link) => link.orchestrator === caller.id && splitThreadKey(link.target).host === host)
      .map((l) => l.target),
  );
  ids.delete(caller.id);
  const all = [...ids].reverse().flatMap((id) => ruleThread(useApp.getState(), id) ?? []);
  return { targets: all.slice(0, MAX_DELEGATES), omitted: Math.max(0, all.length - MAX_DELEGATES) };
}

/**
 * `thread_status`: where each delegate stands — worktree, branch, pull request
 * and checks — in one call, instead of combining read_session, list_pull_requests
 * and a shell poll of `gh pr checks`. A read: no approval, same project. The
 * delegates are read concurrently, so a dozen do not add up to a timeout.
 */
export async function threadStatus(
  caller: RuleThread,
  target: string | undefined,
  host: string,
): Promise<JsonObject> {
  const asked = target ? undefined : delegateTargets(caller, host);
  const targets = target ? [manageTarget(caller, target, true)] : (asked?.targets ?? []);
  const entries: RuleThread[] = [];
  for (const entry of targets) {
    if (isFailure(entry)) return failureJson(entry);
    if (entry.id === caller.id) {
      return failureJson(
        failure("not_allowed", "a thread reports its own pull request through list_pull_requests"),
      );
    }
    entries.push(entry);
  }
  const settled = await Promise.allSettled(
    entries.map(async (entry) => {
      // Another machine's branch and pull request are read by `gh` in its own
      // folders, over its connection, and its recorded activity is asked of
      // it too: that carries the provider error a failed remote turn left
      // (issue 3525).
      const [state, activity] = await Promise.all([
        delegatePull(entry),
        entry.running
          ? activityFrom(entry.id, { order: "newest", limit: 1 }).then((read) => read.answer ?? null)
          : null,
      ]);
      return { entry, state, activity };
    }),
  );
  const delegates: JsonObject[] = [];
  for (const result of settled) {
    if (result.status === "rejected") {
      delegates.push({ error: { code: "failed", message: errorText(result.reason) } });
      continue;
    }
    const { entry, state, activity } = result.value;
    const recorded = Array.isArray(activity?.items) ? obj(activity.items[0]) : undefined;
    const recordedAt = typeof recorded?.at === "string" ? Date.parse(recorded.at) : NaN;
    const transcript = useApp.getState().threads[entry.id];
    const transcriptAt = activityOf(transcript?.items ?? [], transcript?.turn?.children).lastAt;
    const times = [transcriptAt, Number.isFinite(recordedAt) ? recordedAt : undefined].filter(
      (time): time is number => time !== undefined,
    );
    const lastAt = times.length ? Math.max(...times) : entry.updatedAt;
    delegates.push({
      session_id: entry.id,
      title: entry.title,
      ...(targetMachine(entry.id) ? { machine: targetMachine(entry.id) } : {}),
      status: entry.status ?? (entry.running ? "running" : "idle"),
      ...(entry.running && lastAt !== undefined ? { idle_ms: Math.max(0, Date.now() - lastAt) } : {}),
      ...(entry.running && activity?.last_provider_error
        ? { last_provider_error: activity.last_provider_error }
        : {}),
      project: projectOf(entry),
      // The worktree's and the pull request's branch are named only where they differ from `branch`.
      ...(entry.worktree
        ? {
            worktree: {
              path: entry.worktree.path,
              ...(entry.worktree.branch !== state.branch ? { branch: entry.worktree.branch } : {}),
            },
          }
        : {}),
      ...(state.branch ? { branch: state.branch } : {}),
      pull: state.pull
        ? {
            number: state.pull.number,
            title: state.pull.title,
            state: state.pull.state,
            merged: state.pull.merged,
            draft: state.pull.draft,
            ...(state.pull.branch !== state.branch ? { branch: state.pull.branch } : {}),
            head_sha: state.pull.headSha,
            ci: state.pull.ci,
            failing: state.pull.failing,
            pending: state.pull.pending,
            passed: state.pull.passed,
            ...(state.pull.url ? { url: state.pull.url } : {}),
          }
        : null,
      ...(state.note ? { pull_note: state.note } : {}),
      ...(state.error ? { error: { code: state.error.code, message: state.error.message } } : {}),
    });
  }
  return {
    delegates,
    ...(asked?.omitted ? { omitted_delegates: asked.omitted } : {}),
    ...(delegates.length === 0 ? { note: "this thread has no delegates; start one with start_session" } : {}),
  };
}
