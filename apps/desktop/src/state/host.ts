// The app's side of threads starting and messaging threads: it answers the
// engine's `_rustcode/orchestration` requests by checking the rules
// (state/orchestration.ts), asking the user, creating threads and
// delivering messages through the ordinary turn queue, and reporting each
// link turn's answer. After ThreadMessaging.swift (M1), plus v2's
// `read_session`, `start_session` and `fan_out` (docs/orchestrator.md).
//
// The store imports this module and this module imports the store; nothing
// here touches the store at load time.

import type { Json, JsonObject } from "../acp/jsonrpc";
import { parsePullError, parsePullList, pullErrorTitle } from "../acp/rustcode";
import { Method, projectName } from "../acp/types";
import { hasShell, shellCall } from "../lib/shell";
import { titleFrom } from "../lib/title";
import { selectOption, unavailableReason, usableHarnesses } from "./harness";
import {
  activityOf,
  capped,
  checkManage,
  checkMessage,
  checkStart,
  delivered,
  emptyBook,
  type Failure,
  type FanOut,
  failure,
  failureJson,
  type GrantAsk,
  type GrantChoice,
  grant,
  hasGrant,
  interruptActive,
  isActive,
  isFailure,
  type Link,
  type LinkBook,
  type LinkState,
  linkResult,
  linksLeft,
  listSessions,
  MAX_ACTIVE_LINKS,
  MAX_PROJECTS,
  MAX_PULL_REQUESTS,
  markWoken,
  nextId,
  nextWakeAt,
  orphanedLinks,
  type PullStateChoice,
  parseCall,
  projectOf,
  prune,
  READ_BYTES,
  type RuleThread,
  sameProject,
  shownState,
  updateLink,
  type Wake,
  type WakeContext,
  wakeMessage,
  wakesDue,
} from "./orchestration";
import { delegationAllowed, modeLabel } from "./permissions";
import {
  type AppState,
  cancel,
  createSession,
  ensureHarnessModels,
  errorText,
  markUnread,
  openThread,
  permissionModeOf,
  renameThread,
  request,
  runPrompt,
  searchThreads,
  setConfigOption,
  setThreadStatus,
  snoozeThread,
  statusOf,
  switchThreadHarness,
  toast,
  togglePin,
  updateThread,
  useApp,
} from "./store";
import { appendNotice, attachAutoAllowed, localId, type TranscriptItem, turnCompletion } from "./thread";

const BOOK_KEY = "rustcode.threadLinks";

export function loadBook(): LinkBook {
  try {
    const raw = localStorage.getItem(BOOK_KEY);
    if (!raw) return emptyBook();
    const parsed = JSON.parse(raw) as Partial<LinkBook>;
    return interruptActive({ ...emptyBook(), ...parsed });
  } catch {
    return emptyBook();
  }
}

function setBook(fn: (book: LinkBook) => LinkBook) {
  const book = prune(fn(useApp.getState().links));
  useApp.setState({ links: book });
  try {
    localStorage.setItem(BOOK_KEY, JSON.stringify(book));
  } catch {
    // Not persisted in a private window.
  }
}

const book = () => useApp.getState().links;

/**
 * The links each parent is blocked on in `wait_session` right now: a wake for
 * one of them would be a duplicate, so it is held back while the wait runs.
 */
const waiting = new Map<string, Set<string>>();

/** The timer that re-checks a coalescing window; one at a time. */
let wakeTimer: ReturnType<typeof setTimeout> | undefined;

function ruleThread(s: AppState, id: string): RuleThread | undefined {
  const summary = s.sessions.find((x) => x.id === id);
  const thread = s.threads[id];
  if (!summary && !thread) return undefined;
  return {
    id,
    title: summary?.title ?? thread?.title ?? "Untitled thread",
    folder: thread?.cwd ?? summary?.cwd ?? "",
    worktree: thread?.worktree ?? summary?.worktree,
    updatedAt: summary?.updatedAt,
    running: thread?.running ?? false,
    harness: thread?.harness?.id ?? summary?.harness,
    status: statusOf(s, id),
  };
}

// ---------------------------------------------------------------- asking the user

let resolveAsk: ((choice: GrantChoice) => void) | null = null;

/** Ask once; a second request while one waits is refused, and the agent can try again. */
function ask(question: GrantAsk): Promise<GrantChoice> | Failure {
  if (useApp.getState().grant) return failure("busy", "another thread's request is waiting for the user");
  toast(`“${question.callerTitle}” asks to ${verb(question)}`, "info");
  return new Promise((resolve) => {
    resolveAsk = resolve;
    useApp.setState({ grant: { ...question, id: localId("grant") } });
  });
}

export function verb(q: GrantAsk): string {
  switch (q.kind) {
    case "message":
      return `message “${q.target}”`;
    case "read":
      return `read “${q.target}”`;
    case "start":
      return "start a thread";
    case "fan_out":
      return `run one task on ${q.harnesses?.length ?? 2} harnesses`;
    case "organize":
      return `change “${q.target}”`;
    case "interrupt":
      return `stop “${q.target}”`;
    case "configure":
      return `change the model of “${q.target}”`;
    case "transfer":
      return `move “${q.target}” to another harness`;
  }
}

/** The user's answer from the grant prompt. */
export function resolveGrant(choice: GrantChoice) {
  const resolve = resolveAsk;
  resolveAsk = null;
  useApp.setState({ grant: null });
  resolve?.(choice);
}

/** The orchestration tool each grant kind answers for, on the caller's own transcript row. */
const TOOL_OF_KIND: Record<GrantAsk["kind"], string> = {
  read: "read_session",
  message: "message_session",
  start: "start_session",
  fan_out: "fan_out",
  organize: "update_session",
  interrupt: "interrupt_session",
  configure: "configure_session",
  transfer: "transfer_session",
};

/** What the app did on the thread's behalf, when its call's row is not in flight to say it. */
const ALLOWED_NOTICE: Record<GrantAsk["kind"], string> = {
  read: "Read a thread",
  message: "Messaged a thread",
  start: "Started a thread",
  fan_out: "Fanned out across harnesses",
  organize: "Changed a thread",
  interrupt: "Stopped a thread",
  configure: "Changed a thread's model",
  transfer: "Moved a thread to another harness",
};

const toasted = new Set<string>();

async function permitted(caller: RuleThread, key: string, question: GrantAsk): Promise<Failure | null> {
  if (hasGrant(book(), caller.id, key)) return null;
  // The thread's access level answers for the user where it already allows the work: the same
  // policy that answers the harness's own approvals, so a Full-access thread is not re-asked here.
  const mode = permissionModeOf(useApp.getState(), caller.id);
  if (delegationAllowed(mode, question.kind)) {
    // Not silent: the call's own row says the thread did it on its own, and why. A notice would
    // both name the caller (the reader is already in that thread) and split the run of calls.
    const why = modeLabel(mode);
    let attached = false;
    updateThread(caller.id, (t) => {
      const marked = attachAutoAllowed(t, TOOL_OF_KIND[question.kind], why);
      attached = marked.attached;
      return marked.attached ? marked.state : t;
    });
    if (!attached) {
      updateThread(caller.id, (t) =>
        appendNotice(t, `${ALLOWED_NOTICE[question.kind]} · allowed by ${why}`, "info"),
      );
    }
    // The transcript keeps every one; the toast comes once per thread and kind, so an orchestrator
    // reading threads in a loop does not flood the screen.
    const once = `${caller.id}:${question.kind}`;
    if (!toasted.has(once)) {
      toasted.add(once);
      toast(`“${caller.title}” ${verb(question)}, allowed by ${why}`, "info");
    }
    return null;
  }
  const asked = ask(question);
  if (isFailure(asked)) return asked;
  const choice = await asked;
  if (choice === "deny") return failure("denied", `the user did not allow this thread to ${verb(question)}`);
  if (choice === "always") setBook((b) => grant(b, caller.id, key));
  return null;
}

// ---------------------------------------------------------------- the calls

/** Answer one `_rustcode/orchestration` request: a result, or `{ error }` the agent reads. */
export async function answerOrchestration(params: JsonObject): Promise<JsonObject> {
  try {
    return await answer(params);
  } catch (error) {
    return failureJson(failure("failed", errorText(error)));
  }
}

async function answer(params: JsonObject): Promise<JsonObject> {
  const s = useApp.getState();
  const callerId = typeof params.sessionId === "string" ? params.sessionId : "";
  const caller = ruleThread(s, callerId);
  if (!caller) return failureJson(failure("not_found", "the calling thread is not listed"));
  const call = parseCall(params);
  if (isFailure(call)) return failureJson(call);
  const acrossProjects = () => useApp.getState().readAcrossProjects;
  /** A read that asked for every project and may not have it: say so, rather than answer half. */
  const reachRefused = () =>
    failureJson(
      failure(
        "not_allowed",
        "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Agents",
      ),
    );
  const threads = () => {
    const st = useApp.getState();
    return st.sessions.flatMap((x) => ruleThread(st, x.id) ?? []);
  };
  switch (call.tool) {
    case "list_sessions":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listSessions(threads(), caller, call.query, call.allProjects);
    case "read_session":
      return read(caller, call.target, call.lastTurns);
    case "message_session":
      return message(caller, call.target, call.text);
    case "wait_session":
      return wait(caller.id, call.links, call.timeoutMs);
    case "start_session":
      return start(caller, call);
    case "fan_out":
      return fanOut(caller, call);
    case "list_harnesses":
      return listHarnesses(caller);
    case "search_sessions":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return searchSessions(threads(), caller, call.query, call.allProjects);
    case "list_worktrees":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listWorktrees(threads(), caller, call.allProjects);
    case "list_projects":
      return listProjects(s);
    case "list_pull_requests":
      return listPullRequests(caller, call.state);
    case "update_session":
      return updateSession(caller, call);
    case "interrupt_session":
      return interruptSession(caller, call.target);
    case "configure_session":
      return configureSession(caller, call);
    case "transfer_session":
      return transferSession(caller, call.target, call.harness);
    case "list_pending_requests":
      return listPendingRequests(caller);
  }
}

// ---------------------------------------------------------------- reads: no approval

/** Models listed per harness: an agent's context pays for each one. */
const LISTED_MODELS = 25;

async function listHarnesses(caller: RuleThread): Promise<JsonObject> {
  const snapshot = useApp.getState().harnesses;
  const harnesses = snapshot?.harnesses ?? [];
  await Promise.all(harnesses.filter((h) => !unavailableReason(h)).map((h) => ensureHarnessModels(h.id)));
  const fresh = useApp.getState().harnesses?.harnesses ?? harnesses;
  return {
    current: { harness: caller.harness ?? "rustcode", ...ranWith(caller.id) },
    harnesses: fresh.map((h) => {
      const reason = unavailableReason(h);
      const models = h.models.slice(0, LISTED_MODELS).map((m) => ({
        id: m.id,
        name: m.name,
        ...(m.efforts.length ? { efforts: m.efforts } : {}),
      }));
      return {
        id: h.id,
        name: h.name,
        available: !reason,
        ...(reason ? { reason } : { models }),
        ...(!reason && h.models.length > LISTED_MODELS
          ? { omitted_models: h.models.length - LISTED_MODELS }
          : {}),
      };
    }),
  };
}

async function searchSessions(
  all: RuleThread[],
  caller: RuleThread,
  query: string,
  allProjects = false,
): Promise<JsonObject> {
  const said = await searchThreads(query);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = all
    .filter((t) => allProjects || sameProject(projectOf(t), projectOf(caller)))
    .filter((t) => t.id in said || words.every((w) => t.title.toLowerCase().includes(w)))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 30);
  return {
    sessions: matches.map((t) => ({
      session_id: t.id,
      title: t.title,
      status: t.status ?? (t.running ? "running" : "idle"),
      ...(allProjects ? { project: projectOf(t) } : {}),
      ...(said[t.id] ? { snippet: said[t.id] } : {}),
    })),
  };
}

function listWorktrees(all: RuleThread[], caller: RuleThread, allProjects = false): JsonObject {
  return {
    worktrees: all.flatMap((t) =>
      t.worktree && (allProjects || sameProject(projectOf(t), projectOf(caller)))
        ? [
            {
              session_id: t.id,
              title: t.title,
              ...(allProjects ? { project: projectOf(t) } : {}),
              branch: t.worktree.branch,
              path: t.worktree.path,
            },
          ]
        : [],
    ),
  };
}

/** `list_projects`: every project the user has a thread in, most recently updated first. */
function listProjects(s: AppState): JsonObject {
  const byPath = new Map<string, { path: string; threads: number; working: boolean; updatedAt: number }>();
  for (const x of s.sessions) {
    const path = projectOf({ folder: x.cwd, worktree: x.worktree });
    if (!path) continue;
    const entry = byPath.get(path) ?? { path, threads: 0, working: false, updatedAt: 0 };
    entry.threads += 1;
    entry.working ||= !!s.threads[x.id]?.running || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    byPath.set(path, entry);
  }
  const projects = [...byPath.values()]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, MAX_PROJECTS)
    .map((p) => ({ path: p.path, name: projectName(p.path), threads: p.threads, working: p.working }));
  return { projects };
}

/** Why a pull request read failed, in the shape the agent reads: `{ error: { code, message } }`. */
function pullFailure(error: { code: string; message: string }): Failure {
  return failure(
    error.code === "failed" ? "failed" : "target_unavailable",
    `${pullErrorTitle(error.code)}: ${error.message}`,
  );
}

/** `list_pull_requests`: the caller's project's pull requests, through the panel's own call. */
async function listPullRequests(caller: RuleThread, state: PullStateChoice): Promise<JsonObject> {
  const cwd = projectOf(caller);
  if (!cwd) return failureJson(failure("failed", "this thread has no project folder"));
  let result: Json;
  try {
    result = await request(Method.pulls, { cwd, state, limit: MAX_PULL_REQUESTS }, 60_000);
  } catch (error) {
    return failureJson(failure("target_unavailable", `pull requests could not be read: ${errorText(error)}`));
  }
  const error = parsePullError(result);
  if (error) return failureJson(pullFailure(error));
  return {
    pull_requests: parsePullList(result).pulls.map((p) => ({
      number: p.number,
      title: p.title,
      state: p.state,
      branch: p.headRef,
      ...(p.url ? { url: p.url } : {}),
    })),
  };
}

// ---------------------------------------------------------------- managing threads

type UpdateCall = Extract<ReturnType<typeof parseCall>, { tool: "update_session" }>;
type ConfigureCall = Extract<ReturnType<typeof parseCall>, { tool: "configure_session" }>;

const STATUS_OF_CHOICE = { active: "idle", review: "review", settled: "done" } as const;

/** The thread a management tool acts on: the caller's own when none is named. */
function manageTarget(caller: RuleThread, id: string | undefined): RuleThread | Failure {
  const target = id ? ruleThread(useApp.getState(), id) : caller;
  return checkManage(caller, target) ?? (target as RuleThread);
}

async function updateSession(caller: RuleThread, call: UpdateCall): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  if (target.id !== caller.id) {
    const changes = [
      call.title && `rename it to “${call.title}”`,
      call.pinned !== undefined && (call.pinned ? "pin it" : "unpin it"),
      call.unread && "mark it unread",
      call.status && `set its status to ${call.status}`,
      call.snoozedUntil ? "snooze it" : call.snoozedUntil === null && "bring it back from snooze",
    ].filter(Boolean);
    const no = await permitted(caller, `organize:${target.id}`, {
      kind: "organize",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `It would ${changes.join(", ")}.`,
    });
    if (no) return failureJson(no);
  }
  if (call.title && !(await renameThread(target.id, call.title))) {
    return failureJson(failure("failed", "the thread could not be renamed"));
  }
  if (call.pinned !== undefined && useApp.getState().pinned.includes(target.id) !== call.pinned) {
    togglePin(target.id);
  }
  if (call.unread) markUnread(target.id);
  if (call.status) await setThreadStatus(target.id, STATUS_OF_CHOICE[call.status]);
  if (call.snoozedUntil !== undefined) snoozeThread(target.id, call.snoozedUntil);
  const after = ruleThread(useApp.getState(), target.id);
  const until = useApp.getState().snoozed[target.id];
  return {
    session_id: target.id,
    title: after?.title ?? target.title,
    pinned: useApp.getState().pinned.includes(target.id),
    status: after?.status ?? "idle",
    snoozed_until: until ? new Date(until).toISOString() : null,
  };
}

async function interruptSession(caller: RuleThread, id: string): Promise<JsonObject> {
  const target = manageTarget(caller, id);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) return failureJson(failure("not_allowed", "a thread cannot stop itself"));
  if (!target.running) return { session_id: target.id, interrupted: false, status: target.status ?? "idle" };
  const no = await permitted(caller, `interrupt:${target.id}`, {
    kind: "interrupt",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: "Its current turn stops. The thread stays and can be messaged again.",
  });
  if (no) return failureJson(no);
  await cancel(target.id);
  return { session_id: target.id, interrupted: true };
}

async function configureSession(caller: RuleThread, call: ConfigureCall): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  await openThread(target.id);
  const options = () => useApp.getState().threads[target.id]?.configOptions ?? [];
  const changes: [string, string][] = [];
  if (call.model) changes.push(["model", call.model]);
  if (call.effort) changes.push(["effort", call.effort]);
  for (const [id, value] of changes) {
    const option = selectOption(options(), id);
    if (!option) return failureJson(failure("not_allowed", `${target.title} has no ${id} setting`));
    const known = option.choices.map((c) => c.value);
    if (!known.includes(value)) {
      return failureJson(
        failure("invalid", `no ${id} “${value}”; choose from ${known.slice(0, 25).join(", ")}`),
      );
    }
  }
  if (changes.length && target.id !== caller.id) {
    const no = await permitted(caller, `configure:${target.id}`, {
      kind: "configure",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `Its next turn would run on ${changes.map(([, value]) => value).join(" · ")}.`,
    });
    if (no) return failureJson(no);
  }
  for (const [id, value] of changes) await setConfigOption(target.id, id, value);
  return {
    session_id: target.id,
    harness: target.harness ?? "rustcode",
    access: permissionModeOf(useApp.getState(), target.id),
    ...ranWith(target.id),
  };
}

async function transferSession(caller: RuleThread, id: string, harness: string): Promise<JsonObject> {
  const target = manageTarget(caller, id);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) return failureJson(failure("not_allowed", "a thread cannot move itself"));
  if (target.running)
    return failureJson(failure("busy", `${target.title} is working; stop it or wait for it first`));
  const listed = useApp.getState().harnesses?.harnesses ?? [];
  const to = listed.find((h) => h.id === harness);
  if (!to) {
    return failureJson(
      failure("invalid", `no harness “${harness}”; choose from ${listed.map((h) => h.id).join(", ")}`),
    );
  }
  const reason = unavailableReason(to);
  if (reason) return failureJson(failure("not_allowed", `${to.name} is not available: ${reason}`));
  if (target.harness === harness) return { session_id: target.id, harness, moved: false };
  const no = await permitted(caller, `transfer:${target.id}`, {
    kind: "transfer",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `It would continue on ${to.name}, which gets the conversation so far with its next message.`,
  });
  if (no) return failureJson(no);
  await openThread(target.id);
  if (!(await switchThreadHarness(target.id, harness))) {
    return failureJson(failure("failed", "the thread could not be moved"));
  }
  return { session_id: target.id, harness, moved: true, ...ranWith(target.id) };
}

/** What this project's open threads wait on. Only what they ask: answering stays with the user. */
function listPendingRequests(caller: RuleThread): JsonObject {
  const s = useApp.getState();
  const requests = Object.values(s.threads)
    .filter((t) => t.permissions.length)
    .map((t) => ({ thread: t, rule: ruleThread(s, t.id) }))
    .filter(({ rule }) => rule && sameProject(projectOf(rule), projectOf(caller)))
    .flatMap(({ thread, rule }) =>
      thread.permissions.map((p) => ({
        session_id: thread.id,
        title: rule?.title ?? "",
        kind: p.question ? "question" : "approval",
        asks: p.question?.text ?? p.title,
      })),
    );
  return { requests };
}

async function read(caller: RuleThread, targetId: string, lastTurns: number): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  const refused = checkMessage(caller, target, book(), true, useApp.getState().readAcrossProjects);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  // Its project only matters to the user when it is not the caller's.
  const elsewhere = sameProject(projectOf(target), projectOf(caller))
    ? ""
    : ` It is in ${projectOf(target)}, another project.`;
  const no = await permitted(caller, target.id, {
    kind: "read",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `Its last ${lastTurns} turn${lastTurns === 1 ? "" : "s"}: your messages and its replies, no tool output.${elsewhere}`,
  });
  if (no) return failureJson(no);
  await openThread(target.id);
  const items = useApp.getState().threads[target.id]?.items ?? [];
  const turns = turnsOf(items).slice(-lastTurns);
  // Bounded: drop the oldest turns until the answer fits.
  let shown = turns;
  while (shown.length > 1 && new TextEncoder().encode(JSON.stringify(shown)).length > READ_BYTES) {
    shown = shown.slice(1);
  }
  const body = JSON.stringify(shown);
  const fits = new TextEncoder().encode(body).length <= READ_BYTES;
  return {
    title: target.title,
    status: target.status ?? "idle",
    project: projectOf(target),
    turns: fits
      ? shown
      : shown.map((t) => ({
          ...t,
          user: capped(String(t.user), READ_BYTES / 4).text,
          assistant: capped(String(t.assistant), READ_BYTES / 2).text,
        })),
    ...(shown.length < turns.length || !fits ? { truncated: true } : {}),
  };
}

/** A thread's turns as `read_session` gives them: the message, the last reply and how many files changed. */
export function turnsOf(items: TranscriptItem[]): JsonObject[] {
  const out: JsonObject[] = [];
  let current: { user: string; assistant: string; start: number } | null = null;
  const close = (end: number) => {
    if (!current) return;
    const files = turnCompletion(items, end)?.files.length ?? 0;
    out.push({ user: current.user, assistant: current.assistant, files_changed: files });
  };
  items.forEach((item, i) => {
    if (item.kind === "user") {
      close(i);
      current = { user: item.text, assistant: "", start: i };
    } else if (item.kind === "assistant" && current) {
      current.assistant = item.text;
    }
  });
  close(items.length);
  return out;
}

async function message(caller: RuleThread, targetId: string, text: string): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  const refused = checkMessage(caller, target, book());
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: text,
  });
  if (no) return failureJson(no);
  // The user took their time: look again.
  const again = checkMessage(caller, ruleThread(useApp.getState(), targetId), book());
  if (again) return failureJson(again);
  await openThread(target.id);
  const thread = useApp.getState().threads[target.id];
  if (!thread || thread.error) {
    return failureJson(failure("target_unavailable", "that thread could not be opened"));
  }
  const busy = thread.running;
  const id = addLink({
    orchestrator: caller.id,
    target: target.id,
    orchestratorTitle: caller.title,
    targetTitle: target.title,
    message: text,
    state: busy ? "queued" : "working",
  });
  deliver(target.id, delivered(text, caller.title), id);
  return { link_id: id, receipt: busy ? "queued" : "started", session_id: target.id, title: target.title };
}

function addLink(link: Omit<Link, "id" | "createdAt">): string {
  let id = "";
  setBook((b) => {
    const next = nextId(b, link.orchestrator);
    id = next.id;
    return { ...next.book, links: [...next.book.links, { ...link, id, createdAt: Date.now() }] };
  });
  return id;
}

/** After the turn the target runs, never into it: the same queue a typed message uses. */
function deliver(target: string, text: string, link: string) {
  const thread = useApp.getState().threads[target];
  if (thread?.running) {
    updateThread(target, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text, link }] }));
  } else {
    void runPrompt(target, text, undefined, { link });
  }
}

type StartCall = Extract<ReturnType<typeof parseCall>, { tool: "start_session" }>;
type FanOutCall = Extract<ReturnType<typeof parseCall>, { tool: "fan_out" }>;

/** Checks a harness id against the engine's list; the default when none is named. */
/**
 * The harness a delegated thread runs on: it must be installed, turned on and
 * signed in (the same rule as the picker), else the error names the ones that
 * can run now, so the agent can pick again (issue 2799).
 */
function harnessFor(name: string | undefined): { id: string; name: string } | Failure {
  const snapshot = useApp.getState().harnesses;
  const id = name ?? snapshot?.default.harness ?? "rustcode";
  const h = snapshot?.harnesses.find((x) => x.id === id || x.name.toLowerCase() === id.toLowerCase());
  if (!h) return failure("invalid", `no harness “${id}”; the ones that can run now are ${usableIds()}`);
  const reason = unavailableReason(h);
  if (reason)
    return failure("not_allowed", `${h.name} cannot run a thread: ${reason}. Usable now: ${usableIds()}`);
  return { id: h.id, name: h.name };
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

/** "Claude Code · opus · high" for the approval card: what will actually run. */
const choiceLabel = (name: string, model?: string, effort?: string) =>
  [name, model, effort].filter(Boolean).join(" · ");

/** The model and effort the new thread reports it runs on, for the caller's answer. */
function ranWith(id: string): JsonObject {
  const options = useApp.getState().threads[id]?.configOptions ?? [];
  const model = selectOption(options, "model")?.current;
  const effort = selectOption(options, "effort")?.current;
  return { ...(model ? { model } : {}), ...(effort ? { effort } : {}) };
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
  options: { model?: string; effort?: string; title: string; worktree?: boolean },
): Promise<string> {
  const id = await createThreadFor(project, harness, options);
  // The orchestrator named it; without this the first message ("From thread …") would.
  if (hasShell()) await shellCall("thread_rename", { id, title: options.title }).catch(() => undefined);
  return id;
}

async function createThreadFor(
  project: string,
  harness: string,
  options: { model?: string; effort?: string; title: string; worktree?: boolean },
): Promise<string> {
  const wantWorktree = (options.worktree ?? true) && !!useApp.getState().features.worktrees;
  try {
    return await createSession({
      cwd: project,
      harness,
      model: options.model,
      effort: options.effort,
      title: options.title,
      worktree: wantWorktree ? { repo: project, title: options.title } : undefined,
    });
  } catch (error) {
    // The default (a worktree) does not apply outside a git repository: start in the folder.
    if (wantWorktree && options.worktree === undefined && /git|repositor/i.test(errorText(error))) {
      return createSession({
        cwd: project,
        harness,
        model: options.model,
        effort: options.effort,
        title: options.title,
      });
    }
    throw error;
  }
}

async function start(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  const refused = checkStart(caller, book(), 1);
  if (refused) return failureJson(refused);
  const project = projectFor(caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const harness = harnessFor(call.harness);
  if (isFailure(harness)) return failureJson(harness);
  const model = await modelFor(harness.id, call.model);
  if (isFailure(model)) return failureJson(model);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(caller, other ? `start:${project}` : "start", {
    kind: "start",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: call.prompt,
    project: other ? project : undefined,
    harnesses: [choiceLabel(harness.name, model, call.effort)],
    worktree: call.worktree ?? true,
  });
  if (no) return failureJson(no);
  const again = checkStart(caller, book(), 1);
  if (again) return failureJson(again);
  let id: string;
  try {
    id = await newThread(project, harness.id, { ...call, model, title });
  } catch (error) {
    return failureJson(failure("failed", `the thread could not start: ${errorText(error)}`));
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
  });
  deliver(id, delivered(call.prompt, caller.title), link);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    ...ranWith(id),
    links_left: linksLeft(book(), caller.id),
  };
}

async function fanOut(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  const refused = checkStart(caller, book(), call.harnesses.length);
  if (refused) return failureJson(refused);
  const chosen: { id: string; name: string; model?: string; effort?: string }[] = [];
  for (const entry of call.harnesses) {
    const h = harnessFor(entry.harness);
    if (isFailure(h)) return failureJson(h);
    const model = await modelFor(h.id, entry.model);
    if (isFailure(model)) return failureJson(model);
    chosen.push({ ...h, model, effort: entry.effort });
  }
  const project = projectOf(caller);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const no = await permitted(caller, "start", {
    kind: "fan_out",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: call.prompt,
    harnesses: chosen.map((h) => choiceLabel(h.name, h.model, h.effort)),
    worktree: true,
  });
  if (no) return failureJson(no);
  const again = checkStart(caller, book(), chosen.length);
  if (again) return failureJson(again);
  const fanId = localId("fan");
  const out: JsonObject[] = [];
  const linkIds: string[] = [];
  for (const h of chosen) {
    const threadTitle = `${title} · ${h.name}`;
    try {
      const id = await newThread(project, h.id, { model: h.model, effort: h.effort, title: threadTitle });
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.id,
        fanOut: fanId,
      });
      linkIds.push(link);
      out.push({ session_id: id, link_id: link, harness: h.id, ...ranWith(id) });
      deliver(id, delivered(call.prompt, caller.title), link);
    } catch (error) {
      // The reason, not the name of the error class: the harness says why it could not start.
      out.push({ harness: h.id, ...failureJson(failure("failed", errorText(error))) });
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
  return { links: out, fan_out_id: fanId, links_left: linksLeft(book(), caller.id) };
}

/** A link's state now: a running turn that waits on an approval or a question needs the user. */
export function stateOf(s: AppState, link: Link) {
  return shownState(link, !!s.threads[link.target]?.permissions.length);
}

/**
 * `wait_session`: blocks until one of the links settles or asks the user, or
 * the timeout passes. Each answer carries what the app already knows about the
 * child (idle time, last tool, turns), so a timeout says whether it is
 * thinking or stuck. While it runs, this parent is registered as waiting, so
 * no wake repeats what the wait itself returns.
 */
async function wait(callerId: string, ids: string[], timeoutMs: number): Promise<JsonObject> {
  const mine = ids.filter((id) => book().links.find((l) => l.id === id)?.orchestrator === callerId);
  if (!mine.length) return failureJson(failure("not_found", "no such link from this thread"));
  const deadline = Date.now() + timeoutMs;
  const set = waiting.get(callerId) ?? new Set<string>();
  for (const id of mine) set.add(id);
  waiting.set(callerId, set);
  const until = new Date(deadline).toISOString();
  const result = (link: Link) => {
    const s = useApp.getState();
    const now = Date.now();
    const activity = activityOf(s.threads[link.target]?.items ?? []);
    return linkResult(link, stateOf(s, link), statusOf(s, link.target), { now, activity });
  };
  // The parent has this answer now: no wake may repeat it.
  const hand = (link: Link) => {
    markHeard(link, stateOf(useApp.getState(), link), Date.now());
    return { links: [result(link)] };
  };
  try {
    for (;;) {
      const s = useApp.getState();
      for (const id of mine) {
        const link = book().links.find((l) => l.id === id);
        if (!link) continue;
        if (isActive(link.state) && !s.sessions.some((x) => x.id === link.target)) {
          setBook((b) => updateLink(b, id, { state: "failed", answer: "That thread is gone." }));
          continue;
        }
        const now = stateOf(s, link);
        if (!isActive(now) || now === "needs_user") return hand(link);
      }
      if (Date.now() >= deadline) {
        return {
          status: "timeout",
          waited_until: until,
          links: mine.flatMap((id) => {
            const link = book().links.find((l) => l.id === id);
            return link ? [result(link)] : [];
          }),
          note: "Still running. You are woken when it finishes or asks; do not poll with sleeps.",
        };
      }
      await new Promise((r) => setTimeout(r, 300));
    }
  } finally {
    for (const id of mine) set.delete(id);
    if (!set.size) waiting.delete(callerId);
    wakeTick();
  }
}

// ---------------------------------------------------------------- waking a parent

/** What the wake rules need of the app, read once: no polling, no engine calls. */
function wakeContext(now: number): WakeContext {
  const s = useApp.getState();
  const states: Record<string, LinkState> = {};
  const pending: Record<string, string> = {};
  const parents: Record<string, { title: string; running: boolean }> = {};
  const said: Record<string, readonly string[]> = {};
  for (const link of s.links.links) {
    const thread = s.threads[link.target];
    states[link.id] = shownState(link, !!thread?.permissions.length);
    const request = thread?.permissions[0];
    if (request) pending[link.id] = String(request.rpcId);
    const parent = ruleThread(s, link.orchestrator);
    if (parent)
      parents[link.orchestrator] = { title: parent.title, running: !!s.threads[link.orchestrator]?.running };
  }
  for (const [id, ids] of waiting) said[id] = [...ids];
  return { states, pending, waiting: said, parents, now };
}

/**
 * Delivers what each parent should hear — children settling, children waiting
 * on the user — once per child, and schedules the re-check a coalescing window
 * needs. Called when a link settles, when a child asks the user, and when a
 * wait returns; nothing here polls.
 */
export function wakeTick(): void {
  clearTimeout(wakeTimer);
  wakeTimer = undefined;
  const now = Date.now();
  if (!useApp.getState().orchestration) return;
  const ctx = wakeContext(now);
  // A parent that is gone for good: its children's results are never delivered.
  const orphaned = orphanedLinks(book(), ctx);
  if (orphaned.length) {
    setBook((b) => orphaned.reduce((acc, l) => updateLink(acc, l.id, { notifiedAt: now }), b));
  }
  for (const wake of wakesDue(book(), ctx)) sendWake(wake, ctx, now);
  const at = nextWakeAt(book(), ctx);
  if (at !== null) scheduleWake(Math.max(250, at - now + 50));
}

/** Re-checks once, later: a coalescing window ending, or a parent still loading. */
function scheduleWake(ms: number): void {
  clearTimeout(wakeTimer);
  wakeTimer = setTimeout(() => wakeTick(), ms);
}

/**
 * One message per parent, in a turn of its own: queued behind a running turn
 * (never interrupting its tools), started at once when the thread is idle.
 * Links are marked before it is sent, so a wake turn's own activity cannot
 * deliver the same result twice.
 */
function sendWake(wake: Wake, ctx: WakeContext, now: number): void {
  const parent = useApp.getState().threads[wake.parent];
  if (parent?.loading) {
    // Its own history is still replaying: try again once it is there, unmarked.
    scheduleWake(1_000);
    return;
  }
  const text = wakeMessage(wake, now);
  setBook((b) => markWoken(b, wake, ctx, now));
  if (!parent) return;
  if (parent.running) {
    updateThread(wake.parent, (t) => ({
      ...t,
      queue: [...t.queue, { id: localId("queued"), text, wake: true }],
    }));
    return;
  }
  void runPrompt(wake.parent, text, undefined, { wake: true });
}

/**
 * A link the parent was just handed (a `wait_session` answer, or a pending
 * question it was told about): mark it heard, so no wake repeats it.
 */
function markHeard(link: Link, state: LinkState, now: number): void {
  const request = useApp.getState().threads[link.target]?.permissions[0]?.rpcId;
  const asked = request === undefined ? undefined : String(request);
  setBook((b) =>
    updateLink(
      b,
      link.id,
      state === "needs_user" && asked !== undefined ? { askedRequest: asked } : { notifiedAt: now },
    ),
  );
}

// ---------------------------------------------------------------- link turns

export function linkStarted(id: string) {
  setBook((b) => updateLink(b, id, { state: "working" }));
}

/** The link's turn ended: its answer is that turn's last reply, never an earlier one. */
export function linkFinished(
  id: string,
  _thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  const reply = [...items].reverse().find((i) => i.kind === "assistant");
  const { text, truncated } = capped(reply?.kind === "assistant" ? reply.text : "");
  const files = turnCompletion(items, items.length)?.files.length ?? 0;
  const now = Date.now();
  setBook((b) =>
    updateLink(b, id, {
      state: outcome === "cancelled" ? "cancelled" : outcome === "failed" ? "failed" : "done",
      answer: failed ? (text ? `${text}\n\n${failed}` : failed) : text,
      truncated,
      filesChanged: files,
      settledAt: now,
    }),
  );
  wakeTick();
}

/** A queued link message was removed from the target's queue. */
export function queuedLinkDropped(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (link?.state !== "queued") return;
  setBook((b) => updateLink(b, id, { state: "cancelled", settledAt: Date.now() }));
  // Its siblings can now be the last ones working: look again at once.
  wakeTick();
}

/** Stop a link: its queued message goes, or its running turn is cancelled. The user's own turns there go on. */
export async function stopLink(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (!link || !isActive(link.state)) return;
  if (link.state === "queued") {
    updateThread(link.target, (t) => ({ ...t, queue: t.queue.filter((q) => q.link !== id) }));
    setBook((b) => updateLink(b, id, { state: "cancelled" }));
    return;
  }
  await cancel(link.target);
}

export async function stopLinksFrom(orchestrator: string) {
  for (const link of book().links) {
    if (link.orchestrator === orchestrator && isActive(link.state)) await stopLink(link.id);
  }
}

/** Forget a finished link's chip. */
export function dismissLink(id: string) {
  setBook((b) => ({ ...b, links: b.links.filter((l) => l.id !== id || isActive(l.state)) }));
}

/** "Keep this": the fan-out's other threads are done, and the kept one goes to review. */
export async function keepFanOut(fanId: string, keep: string) {
  const group = book().fanOuts.find((f) => f.id === fanId);
  if (!group) return;
  const targets = group.links.flatMap((id) => book().links.find((l) => l.id === id)?.target ?? []);
  for (const target of targets) {
    if (target !== keep) await setThreadStatus(target, "done");
  }
  await setThreadStatus(keep, "review");
  setBook((b) => ({ ...b, fanOuts: b.fanOuts.map((f) => (f.id === fanId ? { ...f, kept: keep } : f)) }));
  toast("Kept. The other threads are marked done.", "success");
}

/** The links of a thread, both ways, for chips, banners and badges. */
export function linksOf(b: LinkBook, id: string) {
  return {
    driving: b.links.filter((l) => l.orchestrator === id).slice(-MAX_ACTIVE_LINKS * 2),
    drivenBy: [...b.links].reverse().find((l) => l.target === id && isActive(l.state)),
  };
}

export const projectLabel = (path: string) => projectName(path);
