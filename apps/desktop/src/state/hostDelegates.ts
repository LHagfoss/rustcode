// Orchestration calls about a delegate's work (issue 3651): its status, merging its pull request once
// green, cleaning up and reporting on its worktree, and handing that work off. Split out of
// state/host.ts.

import { bool, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { cleanupRefusal, parseMergeOutcome, parsePullError } from "../acp/rustcode";
import { Method } from "../acp/types";
import {
  type HandoffRefusal,
  handoffRefusal,
  parseHandoff,
  parseWorktreeStates,
  V2Method,
  type WorktreeState,
} from "../acp/v2";
import { isStatusQuery, type StatusInput, type StatusPull, statusRows } from "./delegateStatus";
import { LINK_AGENT_STATE } from "./delegates";
import { selectOption } from "./harness";
import {
  allThreads,
  book,
  type Droppable,
  deliver,
  droppable,
  droppedFailure,
  localMachine,
  type MachineRef,
  manageTarget,
  permitted,
  reachRefused,
  requestOn,
  ruleThread,
  setBook,
  stateOf,
  trackCall,
} from "./host";
import { delegatePull, pullByNumber, pullFailure } from "./hostSchedule";
import {
  activityOf,
  failure,
  failureJson,
  isFailure,
  MAX_WORKTREE_PULLS,
  MAX_WORKTREES,
  noteLinkEvent,
  type parseCall,
  projectOf,
  type RuleThread,
  reachesThread,
  retainDelegateCleanup,
  retireCleanedLinks,
} from "./orchestration";
import {
  type AppState,
  errorText,
  request,
  sessionById,
  setThreadStatus,
  updateThread,
  useApp,
} from "./store";
import { localId, withPullMark } from "./thread";
import { isRemoteThread, splitThreadKey } from "./threadKeys";

/** How long a read pull request stands before the next read; the panel and "status" share it. */
const PULL_READ_TTL_MS = 30_000;
const pullReads = new Map<string, { at: number; value: StatusPull }>();
const pullFlights = new Map<string, Promise<StatusPull>>();

/** The pull request last read for a delegate, however old; undefined before the first read. */
export function cachedDelegatePull(id: string): StatusPull | undefined {
  return pullReads.get(id)?.value;
}

/**
 * A delegate's pull request and checks, the same read `thread_status` makes (`delegatePull`), kept
 * for {@link PULL_READ_TTL_MS} and shared by every caller in flight, so a panel that redraws and a
 * "status" typed meanwhile cost one `gh` read. A delegate outside a worktree has no branch of its
 * own to look up, so it reads as having none without asking.
 */
export function readDelegatePull(id: string, maxAgeMs = PULL_READ_TTL_MS): Promise<StatusPull> {
  const retained = [...book().links].reverse().find((link) => link.target === id && link.cleanup)?.cleanup;
  if (retained && !ruleThread(useApp.getState(), id)?.worktree)
    return Promise.resolve({ pull: retained.pull ?? null });
  const hit = pullReads.get(id);
  if (hit && Date.now() - hit.at < maxAgeMs) return Promise.resolve(hit.value);
  const flying = pullFlights.get(id);
  if (flying) return flying;
  const entry = ruleThread(useApp.getState(), id);
  if (!entry?.worktree) {
    const retained = [...book().links].reverse().find((link) => link.target === id && link.cleanup)?.cleanup;
    return Promise.resolve({ pull: retained?.pull ?? null });
  }
  const read = delegatePull(entry)
    .then((state): StatusPull => {
      const value: StatusPull = state.error
        ? { pull: null, note: state.error.message }
        : { pull: state.pull ?? null, ...(state.note ? { note: state.note } : {}) };
      pullReads.set(id, { at: Date.now(), value });
      return value;
    })
    .finally(() => pullFlights.delete(id));
  pullFlights.set(id, read);
  return read;
}

/** The delegates a thread has, newest first and one per thread, for the status panel and reply. */
export function delegateStatusInputs(s: AppState, orchestrator: string, max = Infinity): StatusInput[] {
  const seen = new Set<string>();
  const out: StatusInput[] = [];
  const links = s.links.links;
  for (let i = links.length - 1; i >= 0 && out.length < max; i--) {
    const link = links[i]!;
    if (link.orchestrator !== orchestrator || link.target === orchestrator || seen.has(link.target)) continue;
    seen.add(link.target);
    const thread = s.threads[link.target];
    const session = sessionById(s, link.target);
    const activity = activityOf(thread?.items ?? []);
    out.push({
      id: link.target,
      title: session?.title || link.targetTitle,
      state: LINK_AGENT_STATE[stateOf(s, link)],
      lastAt: activity.lastAt ?? session?.updatedAt,
      lastTool: activity.lastTool,
      harness: link.harness ?? thread?.harness?.id ?? session?.harness,
      account: thread?.harness?.account ?? session?.account,
      model:
        (thread?.configOptions && selectOption(thread.configOptions, "model")?.current) || session?.model,
      branch: thread?.worktree?.branch ?? session?.worktree?.branch,
      cleaned: !!link.cleanup,
      pull: link.cleanup ? { pull: link.cleanup.pull ?? null } : cachedDelegatePull(link.target),
    });
  }
  return out;
}

/**
 * `/status` in a thread that has delegates adds their status here, from what the app already
 * knows: a local reply row goes into the transcript, and no model turn runs (each one resent the
 * whole context and cost as much as any other). With `withMessage` the user's message goes in
 * before it; without, the harness's own `/status` report answers the message in its panel. The
 * pull requests are read after the row is drawn and fill in as they arrive. False for a thread
 * with no delegates, where `/status` goes to the harness as it always did.
 */
export function answerStatusLocally(threadId: string, text: string, withMessage = true): boolean {
  if (!isStatusQuery(text)) return false;
  const s = useApp.getState();
  if (!s.threads[threadId]) return false;
  const inputs = delegateStatusInputs(s, threadId);
  if (!inputs.length) return false;
  const now = Date.now();
  const rows = statusRows(inputs, now);
  const statusId = localId("status");
  updateThread(threadId, (t) => ({
    ...t,
    items: [
      ...t.items,
      ...(withMessage ? [{ kind: "user" as const, id: localId("user"), text: text.trim(), at: now }] : []),
      { kind: "status" as const, id: statusId, at: now, rows },
    ],
  }));
  for (const row of rows) {
    if (row.pull && Date.now() - now < PULL_READ_TTL_MS) continue;
    void readDelegatePull(row.id).then((pull) =>
      updateThread(threadId, (t) => ({
        ...t,
        items: t.items.map((item) =>
          item.kind === "status" && item.id === statusId
            ? { ...item, rows: item.rows.map((r) => (r.id === row.id ? { ...r, pull } : r)) }
            : item,
        ),
      })),
    );
  }
  return true;
}

/**
 * `merge_when_green`: waits for a delegate's checks and squash-merges, with the
 * semantics of scripts/pr-merge.sh (advisory jobs ignored, two quiet rounds,
 * a failing check stops it). The engine waits; the app asks the user first.
 */
export async function mergeWhenGreen(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "merge_when_green" }>,
): Promise<JsonObject> {
  const tracked = call.callId ? trackCall(call.callId, caller.id) : undefined;
  try {
    return await mergeTracked(caller, call, tracked);
  } finally {
    if (call.callId && droppable.get(call.callId) === tracked) droppable.delete(call.callId);
  }
}

async function mergeTracked(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "merge_when_green" }>,
  tracked: Droppable | undefined,
): Promise<JsonObject> {
  const target = call.number ? caller : manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  let state = await delegatePull(target);
  if (tracked?.dropped) return failureJson(droppedFailure());
  if (call.number && !state.note && state.pull?.number !== call.number) {
    state = await pullByNumber(target, call.number);
    if (tracked?.dropped) return failureJson(droppedFailure());
  }
  if (state.error) return failureJson(state.error);
  if (state.note) return failureJson(failure("not_found", state.note));
  const pull = state.pull;
  if (!pull) {
    return failureJson(
      call.number
        ? failure("not_found", `pull request #${call.number} was not found in this repository`)
        : failure(
            "not_found",
            `“${target.title}” has no pull request on ${state.branch ?? "its branch"} yet`,
          ),
    );
  }
  if (pull.merged) {
    return {
      session_id: target.id,
      number: pull.number,
      merged: true,
      state: "merged",
      ...(pull.url ? { url: pull.url } : {}),
    };
  }
  const no = await permitted(
    caller,
    `merge:${target.id}:${pull.number}`,
    {
      kind: "merge",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `It would wait for the checks of pull request #${pull.number}${pull.title ? ` “${pull.title}”` : ""}${state.branch ? ` on ${state.branch}` : ""} and squash-merge it once every one passes, then delete its branch. A failing check stops it and reports the names.`,
    },
    tracked,
  );
  if (no) return failureJson(no);
  // Nothing merges for a call that was dropped while it waited for the approval.
  if (tracked?.dropped) return failureJson(droppedFailure());
  const finishMerge = async (): Promise<JsonObject> => {
    // The engine's merge loop runs under the call's id, so a drop ends it before it merges.
    const mergeId = call.number ? undefined : call.callId;
    const host = splitThreadKey(target.id).host;
    const stopMerge = () => {
      if (mergeId) void requestOn(host, Method.pullMergeCancel, { mergeId }, 10_000).catch(() => {});
    };
    if (tracked) tracked.onDrop.add(stopMerge);
    let result: Json;
    try {
      result = await requestOn(
        host,
        Method.pullMerge,
        {
          cwd: projectOf(target),
          number: pull.number,
          ...(state.branch ? { branch: state.branch } : {}),
          timeoutMs: call.timeoutMs,
          ...(mergeId ? { mergeId } : {}),
          // The engine claims the caller's link news for this merge, so it hears once.
          sessionId: caller.id,
        },
        call.timeoutMs + 120_000,
      );
    } catch (error) {
      // The app gives up on the answer, so the loop must not go on to merge unseen.
      stopMerge();
      return failureJson(failure("failed", `the merge did not answer: ${errorText(error)}`));
    } finally {
      tracked?.onDrop.delete(stopMerge);
    }
    const error = parsePullError(result);
    if (error) return failureJson(pullFailure(error));
    const outcome = parseMergeOutcome(result);
    if (!outcome) return failureJson(failure("failed", "the engine answered without a merge result"));
    if (outcome.merged) {
      // The branch's story is part of the link's: the chip's list says it merged.
      setBook((b) => noteLinkEvent(b, caller.id, target.id, "merged"));
    }
    return {
      session_id: target.id,
      number: outcome.number,
      state: outcome.state,
      merged: outcome.merged,
      ...(outcome.url ? { url: outcome.url } : {}),
      ...(outcome.failing.length ? { failing: outcome.failing } : {}),
      waited_secs: outcome.waitedSecs,
      detail: outcome.message,
      ...(outcome.announced ? { announced: true } : {}),
    };
  };
  if (call.number) {
    void finishMerge().then((result) => {
      // The link already woke the thread with "Merged"; a second notice is the same news (issue 3851).
      if (result.announced !== true) deliver(caller.id, mergeResultNotice(pull.number, result), "", "");
    });
    return {
      number: pull.number,
      state: "watching",
      detail: "Merge approved and handed to RustCode. End the turn; the result will wake this thread.",
    };
  }
  return finishMerge();
}

/** Keep the full result for the agent beneath the persistent, human-readable notice line. */
export function mergeResultNotice(number: number, result: JsonObject): string {
  const summary =
    str(result.detail) || str(obj(result.error)?.message) || str(result.state) || "Merge failed";
  return withPullMark(`Pull request #${number}: ${summary}\n\n${JSON.stringify(result)}`);
}

/**
 * `cleanup_worktree`: removes a merged delegate's worktree and branch, then
 * settles it, after the engine confirms nothing unique is lost (git cherry,
 * allowing a squash merge's content). A branch the delegate renamed after this
 * read is accepted by the engine when that branch's pull request merged. Refuses
 * a dirty worktree or one still running. The user approves it.
 */
export async function cleanupWorktree(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "cleanup_worktree" }>,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) {
    return failureJson(failure("not_allowed", "a thread cannot clean up the checkout it runs in"));
  }
  if (!target.worktree) {
    return failureJson(failure("not_allowed", `“${target.title}” does not run in a worktree`));
  }
  if (target.running) {
    return failureJson(failure("conflict", `“${target.title}” is working; wait for its turn to finish`));
  }
  // A pull request read that fails leaves `merged` false, so the engine falls
  // back to the worktree's own `git cherry` check, which is the stricter one.
  const state = await delegatePull(target);
  const merged = state.pull?.merged ?? false;
  const branch = target.worktree.branch;
  const no = await permitted(caller, `cleanup:${target.id}`, {
    kind: "cleanup",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `It would remove the worktree at ${target.worktree.path} and delete the branch ${branch}${merged ? " (its pull request merged)" : ""}, then ${call.settle ? "settle the thread" : "leave the thread as it is"}. It refuses when anything there would be lost.`,
  });
  if (no) return failureJson(no);
  let result: Json;
  try {
    result = await requestOn(
      splitThreadKey(target.id).host,
      V2Method.worktreeCleanup,
      {
        path: target.worktree.path,
        branch,
        headSha: state.pull?.headSha ?? "",
        merged,
      },
      120_000,
    );
  } catch (error) {
    if (error instanceof RpcError) {
      const reason = cleanupRefusal(error.data);
      if (reason) {
        // "unknown" is not a refusal to fix but a git that could not answer.
        const code = reason === "unknown" ? "target_unavailable" : "conflict";
        // The engine's detail (which commits or files, which branch) is in
        // `error.data.message`; `error.message` alone is "Invalid params".
        return failureJson(failure(code, `${reason}: ${errorText(error)}`));
      }
    }
    return failureJson(failure("failed", `the worktree could not be cleaned up: ${errorText(error)}`));
  }
  setBook((b) =>
    retainDelegateCleanup(b, target.id, {
      at: Date.now(),
      branch: state.branch ?? branch,
      ...(state.pull ? { pull: state.pull } : {}),
    }),
  );
  const cleanup = obj(result)?.cleanup;
  // The worktree is gone and the engine moved the thread back to the
  // repository; the store must not keep offering the old folder.
  const repo = projectOf(target);
  useApp.setState((s) => ({
    sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: undefined, cwd: repo } : x)),
  }));
  updateThread(target.id, (t) => ({ ...t, worktree: undefined, cwd: repo }));
  const answer: JsonObject = {
    session_id: target.id,
    removed: bool(obj(cleanup)?.removed) ?? true,
    branch: str(obj(cleanup)?.branch) ?? branch,
    branch_removed: bool(obj(cleanup)?.branch_removed) ?? false,
  };
  // A branch the delegate renamed: the engine removes the recorded one too
  // when it holds nothing unique, and says so.
  const recorded = str(obj(cleanup)?.recorded_branch);
  if (recorded) {
    answer.recorded_branch = recorded;
    answer.recorded_branch_removed = bool(obj(cleanup)?.recorded_branch_removed) ?? false;
  }
  setBook((b) => noteLinkEvent(b, caller.id, target.id, "cleaned"));
  if (call.settle) {
    setBook((b) => retireCleanedLinks(b, target.id));
    await setThreadStatus(target.id, "done");
    answer.settled = true;
  }
  return answer;
}

/**
 * `worktree_status`: where the worktree threads stand — the branch each one is
 * on, its distance from the ref it tracks, how many files are uncommitted, its
 * last commit, and the pull request its branch heads. The git reads are one
 * engine call; the `gh` reads are bounded like `thread_status`'s. A read: no
 * approval.
 */
/** The worktree threads this computer runs, and so can read git for. */
function localWorktrees(): RuleThread[] {
  return allThreads().filter((t) => t.worktree && !isRemoteThread(t.id));
}

export async function worktreeStatus(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "worktree_status" }>,
  machine: MachineRef = localMachine(),
): Promise<JsonObject> {
  const host = machine.host;
  const where = useApp.getState().readAcrossProjects;
  if (call.allProjects && !where && !host) return reachRefused();
  const entries: RuleThread[] = [];
  // A named machine's worktrees are read by the engine there, every one it has:
  // its folders are in no project of this computer's.
  const all = host
    ? allThreads().filter((t) => t.worktree && splitThreadKey(t.id).host === host)
    : localWorktrees();
  if (call.target) {
    const target = manageTarget(caller, call.target, !!host);
    if (isFailure(target)) return failureJson(target);
    if (!target.worktree) {
      return { worktrees: [], note: `“${target.title}” does not run in a worktree` };
    }
    entries.push(target);
  } else {
    const rows = all.filter((t) => host || call.allProjects || reachesThread(caller, t));
    entries.push(...rows.slice(0, MAX_WORKTREES));
  }
  const omitted = call.target ? 0 : Math.max(0, all.length - entries.length);
  if (!entries.length) {
    return {
      worktrees: [],
      note: "no worktree threads here; start one with start_session and worktree",
    };
  }
  // One engine call reads every worktree's branch, distance and changed files:
  // a git process per path, not a `git` command from the agent.
  const paths = entries.flatMap((t) => (t.worktree ? [t.worktree.path] : []));
  let states: WorktreeState[] = [];
  let stateError: string | undefined;
  try {
    states = parseWorktreeStates(await requestOn(host, V2Method.worktreeState, { paths }, 60_000));
  } catch (error) {
    stateError = errorText(error);
  }
  // A pull request is one `gh` read, run together and capped like `thread_status`.
  const asking = entries.slice(0, MAX_WORKTREE_PULLS);
  const pulls = await Promise.allSettled(asking.map((entry) => delegatePull(entry, host)));
  const worktrees = entries.map((entry, index) => {
    const state = states.find((row) => row.path === entry.worktree?.path);
    const read = index < pulls.length ? pulls[index] : undefined;
    const pull = read?.status === "fulfilled" ? read.value : undefined;
    return {
      session_id: entry.id,
      title: entry.title,
      ...(call.allProjects || host ? { project: projectOf(entry) } : {}),
      status: entry.status ?? (entry.running ? "running" : "idle"),
      branch: state?.branch ?? entry.worktree?.branch ?? "",
      path: state?.path ?? entry.worktree?.path ?? "",
      ...(state ? { exists: state.exists } : {}),
      // The ref is named wherever a count is given, so `ahead: 3` is never
      // ambiguous about what it is ahead of.
      ...(state?.against ? { against: state.against } : {}),
      ...(state?.ahead === undefined ? {} : { ahead: state.ahead }),
      ...(state?.behind === undefined ? {} : { behind: state.behind }),
      ...(state?.dirty === undefined ? {} : { dirty: state.dirty }),
      ...(state?.lastCommit ? { last_commit: state.lastCommit } : {}),
      ...(state?.lastCommitSubject ? { last_commit_subject: state.lastCommitSubject } : {}),
      ...(state?.error ? { error: { code: "unavailable", message: state.error } } : {}),
      ...(pull?.error
        ? { pull_error: { code: pull.error.code, message: pull.error.message } }
        : pull
          ? {
              ...(pull.note ? { pull_note: pull.note } : {}),
              pull: pull.pull
                ? {
                    number: pull.pull.number,
                    title: pull.pull.title,
                    state: pull.pull.state,
                    merged: pull.pull.merged,
                    draft: pull.pull.draft,
                    ...(pull.pull.branch !== (state?.branch ?? entry.worktree?.branch)
                      ? { branch: pull.pull.branch }
                      : {}),
                    ...(pull.pull.url ? { url: pull.pull.url } : {}),
                  }
                : null,
            }
          : {}),
    };
  });
  const pullsOmitted = entries.length - asking.length;
  return {
    worktrees,
    ...(omitted ? { omitted_worktrees: omitted } : {}),
    ...(pullsOmitted ? { pulls_omitted: pullsOmitted } : {}),
    ...(stateError ? { note: `each worktree's git state could not be read: ${stateError}` } : {}),
  };
}

/** The failure code each handoff refusal answers with: the reason itself says what to fix. */
const HANDOFF_CODES: Record<HandoffRefusal, string> = {
  no_worktree: "not_allowed",
  has_worktree: "not_allowed",
  not_checkout: "not_allowed",
  dirty: "conflict",
  dirty_checkout: "conflict",
  unpushed_checkout: "conflict",
  ignored_files: "conflict",
  detached_checkout: "conflict",
  not_git: "target_unavailable",
  no_folder: "target_unavailable",
  unknown: "target_unavailable",
  failed: "failed",
};

/**
 * `handoff_worktree`: move a delegate's work into the user's checkout (so they
 * continue locally), or a thread out of the checkout into a worktree of its
 * own. The engine refuses rather than force — a dirty worktree or checkout, a
 * checkout on another branch with commits no remote has, a running thread —
 * and nothing is stashed, rebased, reset or deleted. The user approves it.
 */
export async function handoffWorktree(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "handoff_worktree" }>,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  const toCheckout = call.to === "checkout";
  if (toCheckout && !target.worktree) {
    return failureJson(
      failure(
        "not_allowed",
        `“${target.title}” does not run in a worktree, so there is no branch to hand over`,
      ),
    );
  }
  if (!toCheckout && target.worktree) {
    return failureJson(failure("not_allowed", `“${target.title}” already runs in a worktree`));
  }
  // A thread mid-turn would keep writing in a folder it is losing.
  if (target.running) {
    return failureJson(failure("conflict", `“${target.title}” is working; wait for its turn to finish`));
  }
  const checkout = projectOf(target);
  const worktree = target.worktree;
  const no = await permitted(caller, `handoff:${target.id}:${call.to}`, {
    kind: "handoff",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: toCheckout
      ? `It would remove the worktree at ${worktree?.path}, switch your checkout ${checkout} to ${worktree?.branch}, and move “${target.title}” there so you can continue locally. It refuses when your checkout has uncommitted changes or holds commits no remote has, or the worktree has uncommitted or ignored files. Nothing is stashed, rebased or reset, and the branch keeps every commit.`
      : `It would create a worktree on a new branch from your checkout's HEAD and move “${target.title}” into it, leaving your checkout untouched. It refuses when your checkout has uncommitted changes.`,
  });
  if (no) return failureJson(no);
  let result: Json;
  try {
    result = await request(
      V2Method.worktreeHandoff,
      { sessionId: target.id, to: call.to, title: target.title },
      120_000,
    );
  } catch (error) {
    if (error instanceof RpcError) {
      const reason = handoffRefusal(error.data);
      if (reason) {
        // The reason names what to fix, and the engine's detail (which files,
        // which commits, which branch) follows it; `error.message` alone is
        // "Invalid params".
        return failureJson(failure(HANDOFF_CODES[reason], `${reason}: ${errorText(error)}`));
      }
    }
    return failureJson(failure("failed", `the handoff did not happen: ${errorText(error)}`));
  }
  const moved = parseHandoff(result);
  if (!moved?.moved) {
    return failureJson(failure("failed", "the engine answered without a handoff"));
  }
  // The thread's folder follows the move; the store must not keep the old one.
  if (moved.to === "checkout") {
    const repo = moved.path ?? checkout;
    useApp.setState((s) => ({
      sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: undefined, cwd: repo } : x)),
    }));
    updateThread(target.id, (t) => ({ ...t, worktree: undefined, cwd: repo }));
  } else {
    const path = moved.path ?? target.folder;
    const at = { path, branch: moved.branch ?? "", repo: moved.repo ?? checkout };
    useApp.setState((s) => ({
      sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: at, cwd: path } : x)),
    }));
    updateThread(target.id, (t) => ({ ...t, worktree: at, cwd: path }));
  }
  setBook((b) => noteLinkEvent(b, caller.id, target.id, "handed_off"));
  return {
    session_id: target.id,
    to: moved.to,
    moved: true,
    ...(moved.worktreeRemoved === undefined ? {} : { worktree_removed: moved.worktreeRemoved }),
    ...(moved.switched === undefined ? {} : { switched: moved.switched }),
    ...(moved.branch ? { branch: moved.branch } : {}),
    ...(moved.path ? { path: moved.path } : {}),
    ...(moved.detail ? { detail: moved.detail } : {}),
  };
}
