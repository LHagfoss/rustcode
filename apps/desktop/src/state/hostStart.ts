// Orchestration calls that start threads: one (start_session) or several at once (fan_out), the
// harness, account and model each runs on, and waiting on them and reading their activity. Split out
// of state/hostManage.ts.

import type { JsonObject } from "../acp/jsonrpc";
import { type Harness, Method } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { hasShell, shellCall } from "../lib/shell";
import { childTitle, titleFrom } from "../lib/title";
import { selectOption, unavailableReason, usableHarnesses } from "./harness";
import {
  book,
  calls,
  delegateLimits,
  droppable,
  filledWith,
  HANDED_KEPT,
  handedBy,
  holdKey,
  holds,
  markHeard,
  permitted,
  setBook,
  subtreeQuiet,
  WAIT_CHECK_MS,
  type Wait,
  waiting,
  wakeTick,
  withLinkGate,
} from "./host";
import { addLink, deliver, type FanOutCall, type StartCall } from "./hostManage";
import { accountLabelOf, accountNameOf, childAccountFor } from "./logins";
import {
  activityOf,
  checkStart,
  cutForAll,
  delegateAccountKey,
  delegated,
  type Failure,
  type FanOut,
  failure,
  failureJson,
  isActive,
  isFailure,
  type Link,
  type LinkState,
  latestReply,
  linkResult,
  linksLeft,
  liveState,
  projectOf,
  type RuleThread,
  sameProject,
  unheard,
  updateLink,
} from "./orchestration";
import { effectiveMode, type PermissionMode } from "./permissions";
import {
  type AppState,
  connectionFor,
  createSession,
  defaultModeFor,
  ensureHarnessModels,
  errorText,
  isKeptUnlisted,
  permissionModeOf,
  runPrompt,
  sessionById,
  setPermissionMode,
  statusOf,
  useApp,
} from "./store";
import { isRunning, localId } from "./thread";
import { splitThreadKey } from "./threadKeys";
import { orchestrationTool } from "./toolNames";

/** Checks a harness id against the engine's list; the default when none is named. */
/**
 * The harness a delegated thread runs on: it must be installed, turned on and
 * signed in (the same rule as the picker), else the error names the ones that
 * can run now, so the agent can pick again (issue 2799).
 */
function harnessFor(name: string | undefined): Harness | Failure {
  const snapshot = useApp.getState().harnesses;
  const id = name ?? snapshot?.default.harness ?? "rustcode";
  const h = snapshot?.harnesses.find((x) => x.id === id || x.name.toLowerCase() === id.toLowerCase());
  if (!h) return failure("invalid", `no harness “${id}”; the ones that can run now are ${usableIds()}`);
  const reason = unavailableReason(h);
  if (reason)
    return failure("not_allowed", `${h.name} cannot run a thread: ${reason}. Usable now: ${usableIds()}`);
  return h;
}

/**
 * The account a delegated thread runs on: the named login the agent asked for
 * (`start_session`'s or a `fan_out` entry's `account`), checked against that
 * harness's accounts, else the parent's own on the same harness, else the one
 * a new thread would start on (`state/logins.ts`, `docs/harnesses.md`).
 */
function accountFor(caller: RuleThread, harness: Harness, asked?: string): string | null | Failure {
  const s = useApp.getState();
  const pick = childAccountFor(
    harness,
    asked,
    {
      harness: caller.harness,
      account: s.threads[caller.id]?.harness?.account ?? s.sessions.find((x) => x.id === caller.id)?.account,
    },
    s.signedOutLogins,
  );
  if (!("refused" in pick)) return pick.account;
  const refusal = pick.refused;
  switch (refusal.kind) {
    case "ambiguous":
      return failure(
        "invalid",
        `${harness.name} has more than one account matching “${refusal.asked}” (${refusal.matches.join(", ")}); name one of those ids`,
      );
    case "signedOut":
      return failure(
        "not_allowed",
        `${harness.name} account “${refusal.name}” (${refusal.account}) is signed out; sign it in in Settings › Harnesses or pick another login (${refusal.available.join(", ")})`,
      );
    case "unknown":
      return failure(
        "invalid",
        refusal.available.length
          ? `${harness.name} has no account “${refusal.asked}”; its named accounts are ${refusal.available.join(", ")}`
          : `${harness.name} has no named accounts; it runs on its own sign-in, so leave account out`,
      );
  }
}

const usableIds = () => {
  const snapshot = useApp.getState().harnesses;
  return (
    (snapshot
      ? usableHarnesses(snapshot)
          .map((h) => h.id)
          .join(", ")
      : "") || "rustcode"
  );
};

/**
 * The model a delegated thread asks for, checked against what the harness
 * lists (asked for once if it has not listed them yet). An unknown id would
 * be ignored by the harness and the child would quietly run on its default,
 * so it is refused with the closest matches instead.
 */
async function modelFor(harness: string, model: string | undefined): Promise<string | undefined | Failure> {
  if (!model) return undefined;
  await ensureHarnessModels(harness);
  const h = useApp.getState().harnesses?.harnesses.find((x) => x.id === harness);
  const models = h?.models ?? [];
  if (!models.length) return model;
  const wanted = model.toLowerCase();
  const exact = models.find((m) => m.id.toLowerCase() === wanted || m.name.toLowerCase() === wanted);
  if (exact) return exact.id;
  // "deepseek-v4-pro" for "deepseek/deepseek-v4-pro": a provider prefix left off, when only one fits.
  const suffixed = models.filter((m) => m.id.toLowerCase().endsWith(`/${wanted}`));
  if (suffixed.length === 1) return suffixed[0]!.id;
  const close = models
    .filter(
      (m) =>
        m.id.toLowerCase().includes(wanted) || wanted.includes(m.id.toLowerCase().split("/").pop() ?? ""),
    )
    .slice(0, 5)
    .map((m) => m.id);
  const offer = (close.length ? close : models.slice(0, 8).map((m) => m.id)).join(", ");
  return failure(
    "invalid",
    `${h?.name ?? harness} has no model “${model}”; ${close.length ? "closest" : "some it lists"}: ${offer}`,
  );
}

/** "Claude Code · Work · opus · high" for the approval card: what will actually run. */
export const choiceLabel = (name: string, account?: string, model?: string, effort?: string) =>
  [name, account, model, effort].filter(Boolean).join(" · ");

/** The model, effort and account the new thread reports it runs on, for the caller's answer. */
export function ranWith(id: string): JsonObject {
  const options = useApp.getState().threads[id]?.configOptions ?? [];
  const model = selectOption(options, "model")?.current;
  const effort = selectOption(options, "effort")?.current;
  const account = useApp.getState().threads[id]?.harness?.account;
  return {
    ...(model ? { model } : {}),
    ...(effort ? { effort } : {}),
    ...(account ? { account } : {}),
  };
}

/** The project a new thread starts in: the caller's, or one the user has opened before. */
function projectFor(caller: RuleThread, asked: string | undefined): string | Failure {
  const own = projectOf(caller);
  if (!asked || sameProject(asked, own)) return own;
  const known = useApp.getState().sessions.some((x) => sameProject(x.worktree?.repo || x.cwd, asked));
  if (!known) return failure("not_allowed", "that folder is not a project the user has opened in RustCode");
  return asked.replace(/\/+$/, "");
}

/** `session/new` for a link, in a worktree when asked (or by default in a git project). */
async function newThread(
  project: string,
  harness: string,
  options: {
    model?: string;
    effort?: string;
    account?: string | null;
    title?: string;
    task: string;
    worktree?: boolean;
    /** The thread starting this one, recorded in the child's metadata. */
    parent?: string;
    /** What the new thread may do without asking (`childMode`), fixed before the user is asked. */
    access: PermissionMode;
  },
): Promise<string> {
  // The orchestrator names it when it gave a title, and the task itself does otherwise: a session
  // made without one takes the message it is sent as its title, and the sidebar then shows
  // "From thread “parent” (agent)…" for good.
  const title = childTitle(options.title, options.task);
  const { access, ...rest } = options;
  const id = await createThreadFor(project, harness, { ...rest, title });
  // Before its first turn, so nothing it does runs broader than the thread that started it.
  setPermissionMode(id, access);
  // The orchestrator named it; without this the first message ("From thread …") would.
  if (hasShell()) await shellCall("thread_rename", { id, title }).catch(() => undefined);
  return id;
}

/**
 * What a delegated thread runs at: the lower of its parent's access and the default access of the
 * project it starts in (Settings › Permissions), which is what a new thread there would get anyway.
 * The parent's own limit is the one that counts, so an Ask or Auto-edit parent never hands out Full
 * access. Applied at every depth, a grandchild can only be narrower still.
 */
export function childMode(caller: RuleThread, project: string): PermissionMode {
  const s = useApp.getState();
  return effectiveMode(permissionModeOf(s, caller.id), defaultModeFor(s, project));
}

/** `mode`, lowered to what the call asked for (a delegate preset's access, issue 3391); never raised. */
export const narrowed = (mode: PermissionMode, asked: PermissionMode | undefined): PermissionMode =>
  asked ? effectiveMode(mode, asked) : mode;

/** What a thread may do without asking, as the orchestration answers report it. */
export const accessOf = (id: string): PermissionMode => permissionModeOf(useApp.getState(), id);

/**
 * What a standalone thread runs at: its project's default access (Settings › Permissions), the
 * same as a thread the user starts by hand. It is the user's own thread, not a
 * delegate, so the caller's own limit is not what bounds it.
 */
export const ownMode = (project: string): PermissionMode => defaultModeFor(useApp.getState(), project);

/**
 * The first message of a standalone thread: the prompt as the user's own, with
 * no "From thread" header and no delegate report to end with (issue 3443).
 */
export function deliverOwn(target: string, text: string) {
  // The thread was created a moment ago, so it is not running: the prompt is
  // its first turn, as if the user had typed it.
  void runPrompt(target, text);
}

async function createThreadFor(
  project: string,
  harness: string,
  options: {
    model?: string;
    effort?: string;
    account?: string | null;
    title: string;
    worktree?: boolean;
    parent?: string;
  },
): Promise<string> {
  const wantWorktree = (options.worktree ?? true) && !!useApp.getState().features.worktrees;
  try {
    return await createSession({
      cwd: project,
      harness,
      account: options.account ?? undefined,
      model: options.model,
      effort: options.effort,
      title: options.title,
      parent: options.parent,
      worktree: wantWorktree ? { repo: project, title: options.title } : undefined,
    });
  } catch (error) {
    // The default (a worktree) does not apply outside a git repository: start in the folder.
    if (wantWorktree && options.worktree === undefined && /git|repositor/i.test(errorText(error))) {
      return createSession({
        cwd: project,
        harness,
        account: options.account ?? undefined,
        model: options.model,
        effort: options.effort,
        title: options.title,
        parent: options.parent,
      });
    }
    throw error;
  }
}

export function start(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  return withLinkGate(() => startNow(caller, call));
}

async function startNow(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  // A standalone thread is not a delegate: it takes no delegation slot, so the
  // depth and slot rules (which bound how much work one thread hands out) do
  // not apply to it.
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), 1, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const project = projectFor(caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const harness = harnessFor(call.harness);
  if (isFailure(harness)) return failureJson(harness);
  const model = await modelFor(harness.id, call.model);
  if (isFailure(model)) return failureJson(model);
  // Resolved before the user is asked, so the card names the login and the thread gets exactly it.
  const account = accountFor(caller, harness, call.account);
  if (isFailure(account)) return failureJson(account);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const other = !sameProject(project, projectOf(caller));
  // Fixed before the user is asked, so the card says what the thread runs at and the thread gets
  // exactly that, however long the card waits.
  const access = narrowed(standalone ? ownMode(project) : childMode(caller, project), call.access);
  const no = await permitted(
    caller,
    other ? `start:${project}` : "start",
    {
      kind: "start",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: [choiceLabel(harness.name, accountLabelOf(harness, account), model, call.effort)],
      access,
      worktree: call.worktree ?? true,
      standalone: standalone || undefined,
    },
    undefined,
    harness.accounts?.length ? [delegateAccountKey("local", harness.id, account)] : [],
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), 1, delegateLimits());
    if (again) return failureJson(again);
  }
  let id: string;
  try {
    // The task is the title's fallback: whatever the prompt starts with, a child never goes
    // unnamed and so never takes the delivered message as its title.
    id = await newThread(project, harness.id, {
      ...call,
      model,
      title,
      task: call.prompt,
      access,
      parent: standalone ? undefined : caller.id,
      account,
    });
  } catch (error) {
    return failureJson(failure("failed", `the thread could not start: ${errorText(error)}`));
  }
  if (standalone) {
    deliverOwn(id, call.prompt);
    return {
      session_id: id,
      receipt: "started",
      title,
      harness: harness.id,
      access: accessOf(id),
      standalone: true,
      ...ranWith(id),
    };
  }
  const link = addLink({
    orchestrator: caller.id,
    target: id,
    orchestratorTitle: caller.title,
    targetTitle: title,
    message: call.prompt,
    state: "working",
    started: true,
    harness: harness.id,
    harnessName: harness.name,
    model,
    account: account ?? undefined,
  });
  deliver(id, delegated(call.prompt, caller.title), link, caller.id);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    access: accessOf(id),
    ...ranWith(id),
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
}

export function fanOut(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  return withLinkGate(() => fanOutNow(caller, call));
}

async function fanOutNow(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), call.harnesses.length, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const chosen: { harness: Harness; model?: string; effort?: string; account: string | null }[] = [];
  const targets = new Set<string>();
  for (const entry of call.harnesses) {
    const h = harnessFor(entry.harness);
    if (isFailure(h)) return failureJson(h);
    const model = await modelFor(h.id, entry.model);
    if (isFailure(model)) return failureJson(model);
    const account = accountFor(caller, h, entry.account);
    if (isFailure(account)) return failureJson(account);
    // Two entries that resolve to the same target would run one task twice, and the engine
    // has only compared the arguments as they came in ('Work' and 'work' are one login).
    const key = [h.id, model ?? "", entry.effort ?? "", account ?? ""].join("\u0000");
    if (targets.has(key)) {
      return failureJson(
        failure(
          "invalid",
          `harnesses resolves to the same target twice: ${choiceLabel(h.name, accountLabelOf(h, account), model, entry.effort)}; give each entry a different harness, model, effort or account`,
        ),
      );
    }
    targets.add(key);
    chosen.push({ harness: h, model, effort: entry.effort, account });
  }
  const project = projectFor(caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode(project) : childMode(caller, project), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    other ? `start:${project}` : "start",
    {
      kind: "fan_out",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: chosen.map((h) =>
        choiceLabel(h.harness.name, accountLabelOf(h.harness, h.account), h.model, h.effort),
      ),
      access,
      worktree: true,
      standalone: standalone || undefined,
    },
    undefined,
    chosen
      .filter((h) => h.harness.accounts?.length)
      .map((h) => delegateAccountKey("local", h.harness.id, h.account)),
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), chosen.length, delegateLimits());
    if (again) return failureJson(again);
  }
  const fanId = localId("fan");
  const out: JsonObject[] = [];
  const linkIds: string[] = [];
  for (const h of chosen) {
    // The account names the thread too when it has one, so two logins of one harness read apart.
    const threadTitle = [title, h.harness.name, accountNameOf(h.harness, h.account)]
      .filter(Boolean)
      .join(" · ");
    try {
      const id = await newThread(project, h.harness.id, {
        model: h.model,
        effort: h.effort,
        account: h.account,
        title: threadTitle,
        task: call.prompt,
        parent: standalone ? undefined : caller.id,
        access,
      });
      if (standalone) {
        deliverOwn(id, call.prompt);
        out.push({ session_id: id, harness: h.harness.id, access: accessOf(id), ...ranWith(id) });
        continue;
      }
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.harness.id,
        harnessName: h.harness.name,
        model: h.model,
        account: h.account ?? undefined,
        fanOut: fanId,
      });
      linkIds.push(link);
      out.push({
        session_id: id,
        link_id: link,
        harness: h.harness.id,
        access: accessOf(id),
        ...ranWith(id),
      });
      deliver(id, delegated(call.prompt, caller.title), link, caller.id);
    } catch (error) {
      // The reason, not the name of the error class: the harness says why it could not start.
      out.push({ harness: h.harness.id, ...failureJson(failure("failed", errorText(error))) });
    }
  }
  if (linkIds.length) {
    const group: FanOut = {
      id: fanId,
      orchestrator: caller.id,
      title,
      prompt: call.prompt,
      links: linkIds,
      createdAt: Date.now(),
    };
    setBook((b) => ({ ...b, fanOuts: [...b.fanOuts, group] }));
  }
  if (standalone) return { links: out, standalone: true };
  return { links: out, fan_out_id: fanId, links_left: linksLeft(book(), caller.id, delegateLimits()) };
}

/**
 * A link's state now: what its target thread does, folded onto the record
 * ({@link liveState}). A running turn that waits on an approval or a question
 * needs the user, and a link a restart could not resume reads live again while
 * its thread keeps working.
 */
export function stateOf(s: AppState, link: Link) {
  return liveState(link, activityOfThread(s, link.target));
}

/** What a thread the app may not have open does now: its live turn, else what the list reports. */
function activityOfThread(s: AppState, id: string): ThreadStatus | undefined {
  const open = s.threads[id];
  if (open?.permissions.length) return "needs_input";
  if (isRunning(open)) return "running";
  // The list's index, not a scan: `stateOf` runs in selectors on each store change (state/store.ts).
  return sessionById(s, id)?.status;
}

/**
 * One thread's activity from the data store of the computer it runs on: this
 * computer's, or the paired machine's own, over that machine's connection. No
 * answer (an older engine, a channel that went away) is said by its callers,
 * never passed on as an empty list that reads as a thread that did nothing
 * (issue 3525).
 */
export async function activityFrom(
  thread: string,
  params: JsonObject,
  timeoutMs = 20_000,
): Promise<{ answer?: JsonObject; error?: string }> {
  const conn = connectionFor(splitThreadKey(thread).host);
  if (!conn) return { error: "it is not connected" };
  try {
    const answer = await conn.request<JsonObject>(
      Method.dataActivity,
      { sessionId: splitThreadKey(thread).id, ...params },
      timeoutMs,
    );
    return { answer };
  } catch (error) {
    return { error: errorText(error) };
  }
}

/** Why a link to a thread the app can no longer find failed: its row and its tip say the same. */
const REASON_THREAD_GONE = "That thread is gone.";

/**
 * Whether a link's thread has gone: no list names it and the app keeps no row for it. A thread
 * this window made there, or another client announced, is listed only once a `session/list`
 * catches up, which on a first turn is after the message that started it: the keep-unlisted set
 * (`state/store.ts` `isKeptUnlisted`) says which unlisted rows are still due. A thread with a
 * `threads` copy is no exception — every link target made in this window has one — so the copy
 * is not what tells an unlisted thread from a gone one (issue 3523).
 */
const vanished = (s: AppState, target: string) =>
  !s.sessions.some((x) => x.id === target) && !isKeptUnlisted(target);

/**
 * `wait_session`: blocks until one of the links settles or asks the user, or
 * the timeout passes. Each answer carries what the app already knows about the
 * child (idle time, last tool, turns), so a timeout says whether it is
 * thinking or stuck. While it runs, this parent is registered as waiting, so
 * no wake repeats what the wait itself returns.
 *
 * A wait ends without an answer when nobody can read one any more: its call was
 * dropped by the harness (`callId`, see {@link orchestrationCallDropped}), its
 * own transcript row finished, or a newer wait took its links over. Its links
 * are then free at once, and anything that settles comes as the ordinary wake.
 */
export async function wait(
  callerId: string,
  ids: string[],
  timeoutMs: number,
  callId?: string,
  all = false,
): Promise<JsonObject> {
  const mine = [...new Set(ids)].filter(
    (id) => book().links.find((l) => l.id === id)?.orchestrator === callerId,
  );
  if (!mine.length) return failureJson(failure("not_found", "no such link from this thread"));
  const deadline = Date.now() + timeoutMs;
  const until = new Date(deadline).toISOString();
  const me: Wait = { caller: callerId, links: new Set(mine), nudge: () => {}, handed: [] };
  // The newest wait on a link takes it over: two waits holding one link could each hand its
  // answer over, and only one of them is being read.
  for (const id of mine) {
    const held = holds.get(holdKey(callerId, id));
    if (held && held !== me) {
      held.ended = "taken";
      held.nudge();
    }
    holds.set(holdKey(callerId, id), me);
    const set = waiting.get(callerId) ?? new Set<string>();
    set.add(id);
    waiting.set(callerId, set);
  }
  if (callId) calls.set(callId, me);
  // Some harnesses drop a call without the engine hearing of it (a tool timeout
  // inside the harness): the caller's own row for the call then ends while the
  // wait runs. Only a caller with one wait row in flight is certainly looking at
  // its own, and a wrong guess costs no answer: a released link comes as a wake.
  const row = waitRowOf(callerId);
  const endEarly = (why: "dropped" | "taken"): JsonObject => {
    me.ended = why;
    return {
      status: why === "taken" ? "taken_over" : "dropped",
      waited_until: until,
      links: [],
      note:
        why === "taken"
          ? "A newer wait_session took these links over, so this one stopped: read that answer. Anything that settles still comes as a wake; do not poll."
          : "This call was dropped (the harness stopped waiting for it), so the wait stopped and its links are free. Anything that settles comes as a wake; do not poll.",
    };
  };
  /**
   * What the parent is told the link is now. A child that settled but whose own subtree still works
   * has not answered yet: it says it works, with how long it has been quiet, rather than hand back
   * the answer of a turn that only started the work below it.
   */
  const shownOf = (link: Link): LinkState => {
    const state = stateOf(useApp.getState(), link);
    return (state === "done" || state === "failed") && !subtreeQuiet(link.target) ? "working" : state;
  };
  const result = (link: Link, shown = shownOf(link)) => {
    const s = useApp.getState();
    const now = Date.now();
    const activity = activityOf(s.threads[link.target]?.items ?? [], s.threads[link.target]?.turn?.children);
    // A settled link with no answer still has one: the target's last reply, read now.
    const reply = link.answer?.trim()
      ? link
      : filledWith(link, latestReply(s.threads[link.target]?.items ?? []));
    return linkResult(reply, shown, statusOf(s, link.target), { now, activity });
  };
  // The link's answer is out with this reply: no wake may repeat it. A link whose state goes out
  // with a timeout is handed over the same way, so a child that settles just as the wait gives up
  // does not come back a second time as a wake.
  const handOne = (link: Link, shown = shownOf(link)) => {
    if (shown === "done" || shown === "failed" || shown === "needs_user") {
      me.handed.push(markHeard(link, shown, Date.now()));
    }
    return result(link, shown);
  };
  const hand = (link: Link, shown = shownOf(link)) => ({ links: [handOne(link, shown)] });
  /** A wave's answer: every ready link in one reply, each answer cut to {@link ALL_ANSWER_CHARS}. */
  const handAll = (ready: Link[]) => ({ links: ready.map((link) => cutForAll(handOne(link))) });
  /** The pause before looking again, cut short when the wait ends early. */
  const rest = () => {
    if (me.ended) return Promise.resolve();
    return new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const done = () => {
        clearTimeout(timer);
        me.nudge = () => {};
        resolve();
      };
      timer = setTimeout(done, WAIT_CHECK_MS);
      me.nudge = done;
    });
  };
  try {
    for (;;) {
      if (me.ended) return endEarly(me.ended);
      if (row && !waitRowRuns(callerId, row)) return endEarly("dropped");
      const s = useApp.getState();
      // The turn that asked is over (the user stopped it, or the harness dropped the call): nobody
      // will read this answer, and holding the links back would keep their wakes from arriving.
      // A thread the app holds no state for keeps waiting: it cannot tell whether a turn runs.
      const caller = s.threads[callerId];
      const ready: Link[] = [];
      let pending = false;
      let asking = false;
      if (caller && !isRunning(caller)) {
        return {
          status: "timeout",
          waited_until: until,
          links: mine.flatMap((id) => {
            const link = book().links.find((l) => l.id === id);
            return link ? [result(link)] : [];
          }),
          note: "The turn that waited has ended, so the wait stopped. Anything that settles still comes as a wake; do not poll.",
        };
      }
      for (const id of mine) {
        const link = book().links.find((l) => l.id === id);
        if (!link) continue;
        if (isActive(link.state) && vanished(s, link.target)) {
          setBook((b) =>
            updateLink(b, id, {
              state: "failed",
              answer: REASON_THREAD_GONE,
              error: REASON_THREAD_GONE,
            }),
          );
          // Failed now: the next look hands it over with the rest.
          pending = true;
          continue;
        }
        const now = stateOf(s, link);
        if (now === "done" || now === "failed") {
          // Settled, but it works on: the answer is the one after its subtree is quiet.
          if (subtreeQuiet(link.target)) {
            if (!all) return hand(link);
            ready.push(link);
          } else pending = true;
          continue;
        }
        if (!isActive(now) || now === "needs_user") {
          if (!all) return hand(link);
          ready.push(link);
          asking ||= now === "needs_user";
        } else pending = true;
      }
      // With `all`, one answer for the whole wave: once every link is ready, or sooner when one
      // waits for the user, who may be needed before the rest can finish.
      if (all && ready.length && (asking || !pending)) return handAll(ready);
      if (Date.now() >= deadline) {
        return {
          status: "timeout",
          waited_until: until,
          links: mine.flatMap((id) => {
            const link = book().links.find((l) => l.id === id);
            return link ? [hand(link)] : [];
          }),
          note: "Still running. You are woken when it finishes or asks; do not poll with sleeps.",
        };
      }
      await rest();
    }
  } finally {
    if (callId && calls.get(callId) === me) calls.delete(callId);
    // What this wait handed over, for a call whose answer turns out to be dropped unread.
    if (callId && me.handed.length) {
      handedBy.set(callId, me.handed);
      while (handedBy.size > HANDED_KEPT) handedBy.delete(handedBy.keys().next().value!);
    }
    for (const id of mine) {
      if (holds.get(holdKey(callerId, id)) !== me) continue;
      holds.delete(holdKey(callerId, id));
      const set = waiting.get(callerId);
      set?.delete(id);
      if (set && !set.size) waiting.delete(callerId);
    }
    wakeTick();
  }
}

/**
 * The caller's own row for a call the app can watch: its `wait_session` row
 * still in flight. Only a lone one is certainly this wait's, so a second call in
 * parallel leaves this off rather than watching another call's row.
 */
function waitRowOf(callerId: string): string | undefined {
  const rows = (useApp.getState().threads[callerId]?.items ?? []).filter(
    (item) =>
      item.kind === "tool" &&
      (item.status === "pending" || item.status === "in_progress") &&
      orchestrationTool(item.title) === "wait_session",
  );
  return rows.length === 1 ? rows[0]!.id : undefined;
}

/** Whether that row still runs: one that ended, or that a reload took away, says the call is gone. */
function waitRowRuns(callerId: string, row: string): boolean {
  const item = (useApp.getState().threads[callerId]?.items ?? []).find((i) => i.id === row);
  return !!item && item.kind === "tool" && (item.status === "pending" || item.status === "in_progress");
}

/**
 * The engine says a call was dropped before it answered (a harness's own tool
 * timeout, a cancel, the MCP server closing): its wait ends where it is, without
 * marking anything heard, so a child that settles comes as the ordinary wake and
 * an answer already sent — which the engine drops unread — is handed again.
 * `_rustcode/orchestration/cancelled`.
 */
export function orchestrationCallDropped(params: JsonObject): void {
  const callId = typeof params.callId === "string" ? params.callId : "";
  if (!callId) return;
  const session = typeof params.sessionId === "string" ? params.sessionId : undefined;
  const pending = droppable.get(callId);
  if (pending && (session === undefined || pending.caller === session)) {
    pending.dropped = true;
    for (const stop of [...pending.onDrop]) stop();
    return;
  }
  const live = calls.get(callId);
  if (live && (session === undefined || live.caller === session)) {
    live.ended = "dropped";
    live.nudge();
    return;
  }
  const marks = handedBy.get(callId);
  if (!marks?.length) return;
  handedBy.delete(callId);
  setBook((b) => unheard(b, marks));
  wakeTick();
}
