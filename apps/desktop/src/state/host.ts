// The app's side of threads starting and messaging threads: it answers the
// engine's `_rustcode/orchestration` requests by checking the rules
// (state/orchestration.ts), asking the user, creating threads and
// delivering messages through the ordinary turn queue, and reporting each
// link turn's answer. After ThreadMessaging.swift (M1), plus v2's
// `read_session`, `start_session` and `fan_out` (docs/orchestrator.md).
//
// The store imports this module and this module imports the store; nothing
// here touches the store at load time.

import type { JsonObject } from "../acp/jsonrpc";
import { projectName } from "../acp/types";
import { hasShell, shellCall } from "../lib/shell";
import { titleFrom } from "../lib/title";
import { selectOption, unavailableReason, usableHarnesses } from "./harness";
import {
  capped,
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
  linkResult,
  listSessions,
  MAX_ACTIVE_LINKS,
  nextId,
  parseCall,
  projectOf,
  prune,
  READ_BYTES,
  type RuleThread,
  sameProject,
  shownState,
  updateLink,
} from "./orchestration";
import { delegationAllowed, modeLabel } from "./permissions";
import {
  type AppState,
  cancel,
  createSession,
  ensureHarnessModels,
  errorText,
  openThread,
  permissionModeOf,
  runPrompt,
  setThreadStatus,
  statusOf,
  toast,
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
};

/** What the app did on the thread's behalf, when its call's row is not in flight to say it. */
const ALLOWED_NOTICE: Record<GrantAsk["kind"], string> = {
  read: "Read a thread",
  message: "Messaged a thread",
  start: "Started a thread",
  fan_out: "Fanned out across harnesses",
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
  const threads = () => {
    const st = useApp.getState();
    return st.sessions.flatMap((x) => ruleThread(st, x.id) ?? []);
  };
  switch (call.tool) {
    case "list_sessions":
      return listSessions(threads(), caller, call.query);
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
  }
}

async function read(caller: RuleThread, targetId: string, lastTurns: number): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  const refused = checkMessage(caller, target, book(), true);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const no = await permitted(caller, target.id, {
    kind: "read",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `Its last ${lastTurns} turn${lastTurns === 1 ? "" : "s"}: your messages and its replies, no tool output.`,
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
      out.push({ harness: h.id, error: { code: "failed", message: errorText(error) } });
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
  return { links: out, fan_out_id: fanId };
}

/** A link's state now: a running turn that waits on an approval or a question needs the user. */
export function stateOf(s: AppState, link: Link) {
  return shownState(link, !!s.threads[link.target]?.permissions.length);
}

async function wait(callerId: string, ids: string[], timeoutMs: number): Promise<JsonObject> {
  const mine = ids.filter((id) => book().links.find((l) => l.id === id)?.orchestrator === callerId);
  if (!mine.length) return failureJson(failure("not_found", "no such link from this thread"));
  const deadline = Date.now() + timeoutMs;
  const result = (link: Link) => {
    const s = useApp.getState();
    return linkResult(link, stateOf(s, link), statusOf(s, link.target));
  };
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
      if (!isActive(now) || now === "needs_user") return { links: [result(link)] };
    }
    if (Date.now() >= deadline) {
      return {
        status: "timeout",
        links: mine.flatMap((id) => {
          const link = book().links.find((l) => l.id === id);
          return link ? [result(link)] : [];
        }),
      };
    }
    await new Promise((r) => setTimeout(r, 300));
  }
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
  setBook((b) =>
    updateLink(b, id, {
      state: outcome === "cancelled" ? "cancelled" : outcome === "failed" ? "failed" : "done",
      answer: failed ? (text ? `${text}\n\n${failed}` : failed) : text,
      truncated,
      filesChanged: files,
    }),
  );
}

/** A queued link message was removed from the target's queue. */
export function queuedLinkDropped(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (link?.state === "queued") setBook((b) => updateLink(b, id, { state: "cancelled" }));
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
