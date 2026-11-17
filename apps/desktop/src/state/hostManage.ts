import { openError as threadOpenError } from "./thread";
// Orchestration calls that manage threads: renaming, settling, organizing, interrupting,
// configuring and moving them, and answering a request one of them waits on. Split out of
// state/host.ts.

import type { JsonObject } from "../acp/jsonrpc";
import type { PermissionOption, PermissionRequest } from "../acp/types";
import { optionScope } from "../lib/approval";
import { hasShell, shellCall } from "../lib/shell";
import {
  type AttachedFile,
  attachedExtras,
  attachmentRefusal,
  attachmentRoots,
  MAX_ATTACHMENT_FILES,
} from "./attachments";
import type { ComposeExtras } from "./compose";
import { selectOption, unavailableReason } from "./harness";
import {
  book,
  delegateLimits,
  linkSteered,
  type MachineRef,
  permitted,
  queueBook,
  ruleThread,
  setBook,
  storedQueues,
  withLinkGate,
} from "./host";
import { ranWith } from "./hostStart";
import { newThreadAccountOn } from "./logins";
import {
  type ApprovalChoice,
  activeDescendants,
  ancestorsOf,
  capped,
  checkManage,
  checkMessage,
  delivered,
  type Failure,
  failure,
  failureJson,
  isFailure,
  type Link,
  MAX_QUEUE_ITEMS,
  nextId,
  noteEvent,
  pageOf,
  type parseCall,
  parseDelivered,
  projectOf,
  QUEUE_PREVIEW_CHARS,
  type RuleThread,
  reachesThread,
  renderRead,
  sameProject,
  sameReach,
  updateLink,
} from "./orchestration";
import { QUEUE_REASON } from "./queue";
import { parseQueueBook } from "./queues";
import {
  type AppState,
  answerPermission,
  answerQuestionOptions,
  answerQuestionText,
  cancel,
  clearFinished,
  errorText,
  lastAccountFor,
  leaveSettledThreads,
  markUnread,
  openThread,
  permissionModeOf,
  removeQueued,
  renameThread,
  runPrompt,
  sendDecisionFor,
  sendQueuedNow,
  setConfigOption,
  setSnoozed,
  setThreadStatus,
  setThreadsStatus,
  statusOf,
  steer,
  switchThreadHarness,
  togglePin,
  updateQueued,
  updateThread,
  useApp,
} from "./store";
import {
  isContinueMessage,
  isWakeMessage,
  localId,
  type QueuedPrompt,
  type TranscriptItem,
  turnCompletion,
} from "./thread";
import { isRemoteThread, splitThreadKey } from "./threadKeys";
import { sessionById } from "./threadStatus";

type UpdateCall = Extract<ReturnType<typeof parseCall>, { tool: "update_session" }>;
type ConfigureCall = Extract<ReturnType<typeof parseCall>, { tool: "configure_session" }>;

const STATUS_OF_CHOICE = { active: "idle", review: "review", settled: "done" } as const;

/** The thread a management tool acts on: the caller's own when none is named. */
/**
 * The thread a management call acts on. A thread on another machine is only
 * reachable when the call named that machine (`machine`): the app refuses to
 * act there by a bare guess, and the tools that cannot work across machines
 * say where the thread runs instead (`targetMachine`).
 */
export function manageTarget(
  caller: RuleThread,
  id: string | undefined,
  machine = false,
  acrossProjects = false,
): RuleThread | Failure {
  const target = id ? ruleThread(useApp.getState(), id) : caller;
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (isRemoteThread(target.id) && !machine) {
    return failure(
      "not_allowed",
      `“${target.title}” runs on ${targetMachine(target.id) ?? "another computer"}: do that there`,
    );
  }
  if (isRemoteThread(target.id) || acrossProjects) return target;
  return checkManage(caller, target) ?? target;
}

/** The machine a thread key names, as people say it; undefined for this computer. */
export function targetMachine(id: string): string | undefined {
  const { host } = splitThreadKey(id);
  return host ? useApp.getState().machineNames[host] : undefined;
}

/** The changes `update_session` and `organize_threads` share, as one list. */
interface ManagedChange {
  title?: string;
  pinned?: boolean;
  unread?: boolean;
}

/** Whether a thread is the list's unread one: it finished something the user has not seen. */
const isUnread = (id: string) => {
  const s = useApp.getState();
  return id in s.finished || id in s.failed;
};

/**
 * Applies a thread's title, pin and unread flag with the store calls the app's own
 * controls use, and counts the ones that did something: a value the thread already
 * has is not a change, and `unread` is a two-way flag (`true` marks it unread,
 * `false` reads it, as opening the thread does).
 */
async function applyManagedChange(
  target: RuleThread,
  change: ManagedChange,
): Promise<{ applied: number; renameFailed: boolean }> {
  let applied = 0;
  const live = () => ruleThread(useApp.getState(), target.id) ?? target;
  if (change.title !== undefined && change.title !== live().title) {
    if (!(await renameThread(target.id, change.title))) return { applied, renameFailed: true };
    applied += 1;
  }
  if (change.pinned !== undefined && useApp.getState().pinned.includes(target.id) !== change.pinned) {
    togglePin(target.id);
    applied += 1;
  }
  if (change.unread !== undefined && isUnread(target.id) !== change.unread) {
    if (change.unread) markUnread(target.id);
    else clearFinished(target.id);
    applied += 1;
  }
  return { applied, renameFailed: false };
}

export async function updateSession(
  caller: RuleThread,
  call: UpdateCall,
  machine = false,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target, machine);
  if (isFailure(target)) return failureJson(target);
  // A thread's name lives on the computer that owns it (`renameThread`).
  if (call.title && isRemoteThread(target.id)) {
    return failureJson(
      failure(
        "not_allowed",
        `a thread on ${targetMachine(target.id) ?? "another computer"} is renamed there`,
      ),
    );
  }
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
      machine: targetMachine(target.id),
    });
    if (no) return failureJson(no);
  }
  // An agent naming its own thread leaves a name the user chose alone (issue 3599); renaming
  // another thread is the user's own approval above.
  const titleStatus = call.title && target.id === caller.id ? await userTitled(target.id) : false;
  const keepTitle = titleStatus !== false;
  const applied = await applyManagedChange(target, {
    title: keepTitle ? undefined : call.title,
    pinned: call.pinned,
    unread: call.unread,
  });
  if (applied.renameFailed) return failureJson(failure("failed", "the thread could not be renamed"));
  if (call.status) {
    const onScreen = useApp.getState().selectedId;
    const failed = await setThreadStatus(target.id, STATUS_OF_CHOICE[call.status]);
    // Settling is what the user asked for ("settle this"), so the thread leaves the screen as their
    // own settle does, unless they moved to another thread while the status was saved.
    if (call.status === "settled" && !failed && useApp.getState().selectedId === onScreen) {
      leaveSettledThreads([target.id]);
    }
  }
  if (call.snoozedUntil !== undefined) await setSnoozed(target.id, call.snoozedUntil);
  const after = ruleThread(useApp.getState(), target.id);
  const until = useApp.getState().snoozed[target.id];
  return {
    session_id: target.id,
    title: after?.title ?? target.title,
    pinned: useApp.getState().pinned.includes(target.id),
    // The status this call set, not the live one: a thread settled in the middle of its turn still
    // runs, and answering "running" read as if the settle had been refused (issue 3529). It is
    // settled now and leaves the active list when its turn ends; `running` says it still works.
    status: call.status ? STATUS_OF_CHOICE[call.status] : (after?.status ?? "idle"),
    running: !!useApp.getState().threads[target.id]?.running,
    snoozed_until: until ? new Date(until).toISOString() : null,
    ...(keepTitle && {
      title_kept:
        titleStatus === null
          ? "Could not check whether the user named this thread; its title stays as it is."
          : "The user named this thread, so its title stays as it is.",
    }),
  };
}

/** Null means the shell could not determine whether the user named the thread. */
async function userTitled(id: string): Promise<boolean | null> {
  if (!hasShell()) return null;
  return shellCall<boolean>("thread_user_titled", { id }).catch(() => null);
}

type AnswerCall = Extract<ReturnType<typeof parseCall>, { tool: "answer_request" }>;
type OrganizeCall = Extract<ReturnType<typeof parseCall>, { tool: "organize_threads" }>;

type Scope = ReturnType<typeof optionScope>;

/** What each scope is called when a choice cannot be answered with it. */
const SCOPE_WORD: Record<Scope, string> = {
  once: "allow once",
  hour: "allow for an hour",
  always: "allow always",
  decline: "decline",
  other: "another answer",
};

/**
 * The option a `choice` answers with: the scope it names, and no other. A request
 * that offers only a standing answer is refused rather than answered once, because
 * the engine records the grant the option carries, not the word the caller used.
 */
function optionFor(request: PermissionRequest, choice: ApprovalChoice): PermissionOption | null {
  const wanted = choice === "deny" ? "decline" : choice === "allow_always" ? "always" : "once";
  return request.options.find((o) => optionScope(o) === wanted) ?? null;
}

/** Why a choice has no option here, naming the scopes the request does offer. */
function scopeRefusal(request: PermissionRequest, choice: ApprovalChoice): Failure {
  const offers = [...new Set(request.options.map(optionScope))].map((scope) => SCOPE_WORD[scope]);
  return failure("invalid", `that request offers ${offers.join(", ")}, not “${ANSWER_LABEL[choice]}”`);
}

/** The options a question offers as answers: the allow and plain ones its card lists, not a reject. */
const questionAnswers = (request: PermissionRequest) =>
  request.options.filter((o) => !o.kind || o.kind === "allow_once" || o.kind === "allow_always");

/** One approval's answer, in the words the grant card and the notice use. */
const ANSWER_LABEL: Record<ApprovalChoice, string> = {
  allow_once: "allow it once",
  allow_always: "always allow it",
  deny: "decline it",
};

/** How one call answers one request; a question answered by words has `text` instead of a pick. */
interface Answering {
  option: PermissionOption | null;
  chosen: PermissionOption | null;
  text: string | null;
}

/**
 * The answer one call gives one request as it is now: an approval's option, a
 * question's option by id or its text, or a question's own Decline. Checked again
 * after the user answered the card, since that request may be gone by then.
 */
function answeringFor(request: PermissionRequest, call: AnswerCall): Answering | Failure {
  if (request.secret)
    return failure("not_allowed", "Secret requests can only be answered privately by the user");
  const question = request.question;
  if (!question) {
    if (call.answer) return failure("invalid", "that request is an approval: give choice");
    const option = optionFor(request, call.choice ?? "allow_once");
    return option ? { option, chosen: null, text: null } : scopeRefusal(request, call.choice ?? "allow_once");
  }
  // A question's card offers the same Decline an approval's does.
  if (call.choice === "deny") {
    const option = optionFor(request, "deny");
    return option ? { option, chosen: null, text: null } : scopeRefusal(request, "deny");
  }
  if (call.answer === undefined) {
    return failure("invalid", "that request is a question: give answer, or choice deny to decline it");
  }
  const answers = questionAnswers(request);
  const chosen = answers.find((o) => o.id === call.answer) ?? null;
  if (chosen) return { option: chosen, chosen, text: null };
  if (!answers.length) return failure("invalid", "that question offers no answer this call can give");
  // The store picks the carrier option, as the card's text field does.
  return { option: null, chosen: null, text: call.answer };
}

/**
 * `answer_request`: answer one approval or question a thread waits on. The
 * caller answers a thread below it in its own delegation tree, never itself,
 * and never one in another project; the card the user would have clicked is
 * the one this answers (`state/store.ts` `answerPermission`, `answerQuestionText`).
 */
export async function answerRequest(caller: RuleThread, call: AnswerCall): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), call.target);
  // Another machine's thread is in no project of this computer's: the tree rule below
  // (a thread this one delegated to) is what lets it be answered from here.
  const refused = target && isRemoteThread(target.id) ? null : checkManage(caller, target);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  if (target.id === caller.id) {
    return failureJson(failure("not_allowed", "a thread cannot answer its own request"));
  }
  if (!ancestorsOf(book(), target.id).has(caller.id)) {
    return failureJson(
      failure(
        "not_allowed",
        `“${target.title}” is not a thread this one delegated to; only a thread below this one in its delegation tree can be answered`,
      ),
    );
  }
  const waiting = () =>
    useApp.getState().threads[target.id]?.permissions.find((p) => String(p.rpcId) === call.requestId);
  const request = waiting();
  if (!request) {
    return failureJson(
      failure("not_found", "that request is not waiting any more; list_pending_requests again"),
    );
  }
  const answered = answeringFor(request, call);
  if (isFailure(answered)) return failureJson(answered);
  const kind = request.question ? "question" : "approval";
  const asked = capped(request.question?.text || request.title, 200).text;
  const choice = call.answer === undefined ? (call.choice ?? "allow_once") : undefined;
  const detail =
    choice === "deny"
      ? `It would decline “${asked}”`
      : choice
        ? `It would ${ANSWER_LABEL[choice]} “${asked}”`
        : `It would answer “${asked}” with: ${call.answer}`;
  // The grant names the request, not only the thread: "always" for one question or
  // command must not answer a later, unrelated one without a card of its own.
  const key = `answer:${target.id}:${kind}:${asked}`;
  const no = await permitted(caller, key, {
    kind: "answer",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail,
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // The user took their time: the card may have been answered in its own chat meanwhile.
  const current = waiting();
  if (!current) {
    return failureJson(failure("conflict", "that request was answered before this call reached it"));
  }
  const now = answeringFor(current, call);
  if (isFailure(now)) return failureJson(now);
  if (!current.question) answerPermission(current, now.option);
  else if (now.text !== null) answerQuestionText(current, now.text);
  else if (now.chosen) answerQuestionOptions(current, [now.chosen]);
  else answerPermission(current, now.option);
  const picked = now.option ?? now.chosen;
  return {
    session_id: target.id,
    request_id: call.requestId,
    thread: target.title,
    kind,
    // A question answered in words or by option id reports that answer; a declined
    // question and every approval report the choice, and the option it picked.
    ...(call.answer !== undefined
      ? { answer: call.answer, ...(picked ? { option: picked.name } : {}) }
      : { choice: choice ?? "allow_once", option: picked?.name ?? "" }),
  };
}

/**
 * `organize_threads`: the batch form of `update_session`. Every listed thread
 * of the caller's project gets the same changes, the user is asked once, and a
 * thread this caller cannot manage is skipped with the reason instead of
 * failing the batch. A change a thread already has is not made and not counted.
 */
export async function organizeThreads(caller: RuleThread, call: OrganizeCall): Promise<JsonObject> {
  const decided: RuleThread[] = [];
  const skipped: JsonObject[] = [];
  for (const id of call.targets) {
    const target = manageTarget(caller, id);
    if (isFailure(target)) {
      skipped.push({ session_id: id, reason: target.message });
      continue;
    }
    decided.push(target);
  }
  if (!decided.length) {
    return failureJson(
      failure("not_allowed", skipped[0]?.reason?.toString() ?? "no thread of that list can be changed"),
    );
  }
  const state = () => useApp.getState();
  /** Which of the call's changes this thread does not have yet. */
  const missing = (target: RuleThread) => ({
    title: call.title !== undefined && call.title !== (ruleThread(state(), target.id) ?? target).title,
    pinned: call.pinned !== undefined && state().pinned.includes(target.id) !== call.pinned,
    unread: call.unread !== undefined && isUnread(target.id) !== call.unread,
    archived: call.archived !== undefined && (statusOf(state(), target.id) === "done") !== call.archived,
  });
  const wanted = decided.filter((target) => Object.values(missing(target)).some(Boolean));
  // Nothing to do is not a change to ask the user about: say so instead.
  if (!wanted.length) return { changed: 0, threads: [], skipped };
  const others = wanted.filter((target) => target.id !== caller.id);
  if (others.length) {
    const any = (key: keyof ReturnType<typeof missing>) => wanted.some((target) => missing(target)[key]);
    const changes = [
      any("title") && `rename ${others.length === 1 ? "it" : "them"} to “${call.title}”`,
      any("pinned") && (call.pinned ? "pin" : "unpin"),
      any("unread") && (call.unread ? "mark unread" : "mark read"),
      any("archived") && (call.archived ? "settle" : "bring back"),
    ].filter(Boolean);
    const names = others.map((target) => `“${target.title}”`).join(", ");
    const key = `organize:${decided
      .map((target) => target.id)
      .sort()
      .join(",")}`;
    const no = await permitted(caller, key, {
      kind: "organize",
      caller: caller.id,
      callerTitle: caller.title,
      target: names,
      detail: `It would ${changes.join(", ")} ${names}.`,
    });
    if (no) return failureJson(no);
  }
  // The status of `archived` goes in one write for the batch, as the sidebar's own
  // multi-select does it; the rest goes one thread at a time, and only where it changes.
  const settled = wanted.filter((target) => missing(target).archived).map((target) => target.id);
  if (settled.length) await setThreadsStatus(settled, call.archived ? "done" : "idle");
  const threads: JsonObject[] = [];
  for (const target of decided) {
    const applied = await applyManagedChange(target, {
      title: call.title,
      pinned: call.pinned,
      unread: call.unread,
    });
    if (applied.renameFailed) {
      skipped.push({ session_id: target.id, reason: "the thread could not be renamed" });
      continue;
    }
    if (!applied.applied && !settled.includes(target.id)) continue;
    threads.push({
      session_id: target.id,
      title: ruleThread(state(), target.id)?.title ?? target.title,
      pinned: state().pinned.includes(target.id),
      unread: isUnread(target.id),
      status: statusOf(state(), target.id),
      archived: statusOf(state(), target.id) === "done",
    });
  }
  return {
    changed: threads.length,
    threads,
    ...(skipped.length ? { skipped } : {}),
  };
}

export async function interruptSession(caller: RuleThread, id: string): Promise<JsonObject> {
  const target = manageTarget(caller, id, true);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) return failureJson(failure("not_allowed", "a thread cannot stop itself"));
  if (!target.running) return { session_id: target.id, interrupted: false, status: target.status ?? "idle" };
  const no = await permitted(caller, `interrupt:${target.id}`, {
    kind: "interrupt",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail:
      "Its current turn stops, and the threads it is working through stop with it. The thread stays and can be messaged again.",
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // What goes down with it, counted before the cascade starts.
  const stopped = activeDescendants(book(), target.id).length;
  await cancel(target.id);
  return { session_id: target.id, interrupted: true, stopped };
}

export async function configureSession(
  caller: RuleThread,
  call: ConfigureCall,
  machine = false,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target, machine);
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
      machine: targetMachine(target.id),
    });
    if (no) return failureJson(no);
  }
  // An agent's own configuring is not the user's pick: it leaves the effort memory alone
  // (state/store.ts `setConfigOption`).
  for (const [id, value] of changes) await setConfigOption(target.id, id, value, { remember: false });
  return {
    session_id: target.id,
    harness: target.harness ?? "rustcode",
    access: permissionModeOf(useApp.getState(), target.id),
    ...ranWith(target.id),
  };
}

export async function transferSession(
  caller: RuleThread,
  id: string,
  harness: string,
  accountId?: string,
  model?: string,
): Promise<JsonObject> {
  // Another project's thread moves too when agents may reach across projects:
  // the move always asks, and the card names the project.
  const across = useApp.getState().readAcrossProjects;
  const target = manageTarget(caller, id, false, across);
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
  const named = accountId ? to.accounts?.find((a) => a.id === accountId || a.name === accountId) : undefined;
  if (accountId && !named) {
    const known = (to.accounts ?? []).map((a) => a.id);
    return failureJson(
      failure(
        "invalid",
        known.length
          ? `${to.name} has no account “${accountId}”; choose from ${known.join(", ")}`
          : `${to.name} has no named accounts; leave account out`,
      ),
    );
  }
  if (target.harness === harness && !named && !model) return { session_id: target.id, harness, moved: false };
  const elsewhere = sameProject(projectOf(target), projectOf(caller))
    ? ""
    : ` It is in ${projectOf(target)}, another project.`;
  const on = [named && `the ${named.name} account`, model && `model ${model}`].filter(Boolean).join(" and ");
  const no = await permitted(caller, `transfer:${target.id}`, {
    kind: "transfer",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `It would continue on ${to.name}${on ? ` with ${on}` : ""}, which gets the conversation so far with its next message.${elsewhere}`,
  });
  if (no) return failureJson(no);
  await openThread(target.id);
  // The login a new thread on that harness would start on, as the picker moves
  // a thread: the one named, else its remembered account, never a harness login
  // the picker no longer offers. A harness with no named accounts passes `null`,
  // exactly as before accounts existed.
  const account = named
    ? named.id
    : newThreadAccountOn(to, lastAccountFor(harness), useApp.getState().signedOutLogins);
  if (!(await switchThreadHarness(target.id, harness, account, model ? { model } : {}))) {
    return failureJson(failure("failed", "the thread could not be moved"));
  }
  return {
    session_id: target.id,
    harness,
    moved: true,
    ...(named ? { account: named.id } : {}),
    ...ranWith(target.id),
  };
}

/** What this project's open threads wait on. Only what they ask: answering stays with the user. */
export function listPendingRequests(caller: RuleThread, machine?: MachineRef): JsonObject {
  const s = useApp.getState();
  const rank = machine?.host;
  const waiting = Object.values(s.threads)
    .filter((t) => t.permissions.length)
    .map((thread) => ({ thread, rule: ruleThread(s, thread.id) }))
    .filter(({ rule }) => {
      if (!rule) return false;
      // A named machine is the whole answer; without one, every thread this
      // thread reaches, wherever it runs.
      if (machine) return splitThreadKey(rule.id).host === rank;
      return reachesThread(caller, rule);
    })
    .flatMap(({ thread, rule }) => thread.permissions.map((request) => ({ thread, rule, request })))
    // Oldest first, as docs/orchestrator.md says: the card that has waited longest comes first.
    .sort((a, b) => (a.request.at ?? 0) - (b.request.at ?? 0));
  const requests = waiting.map(({ thread, rule, request }) => ({
    session_id: thread.id,
    request_id: String(request.rpcId),
    ...(targetMachine(thread.id) ? { machine: targetMachine(thread.id) } : {}),
    title: rule?.title ?? "",
    kind: request.secret ? "secret" : request.question ? "question" : "approval",
    asks: request.question?.text ?? request.title,
    // An approval's choices and a question's answers, in the order the card lists them.
    options: request.options.map((o) => ({ id: o.id, name: o.name })),
  }));
  return { requests };
}

export async function read(
  caller: RuleThread,
  targetId: string,
  lastTurns: number,
  offset: number,
): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  // Reading another machine's thread is that machine's to allow; the app adds
  // nothing to the rule, since the user named the machine and approves the read.
  const refused = checkMessage(
    caller,
    target,
    book(),
    true,
    useApp.getState().readAcrossProjects || isRemoteThread(targetId),
  );
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
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  const openError = await openThread(target.id);
  if (openError) return failureJson(failure("target_unavailable", openError));
  const items = useApp.getState().threads[target.id]?.items ?? [];
  const turns = turnsOf(items).slice(-lastTurns);
  // One rendered answer, then a page of it: pages join with nothing lost or repeated.
  const rendered = renderRead({
    session_id: target.id,
    title: target.title,
    status: target.status ?? "idle",
    project: projectOf(target),
    turns: turns.map((t) => ({
      user: String(t.user ?? ""),
      assistant: String(t.assistant ?? ""),
      files_changed: Number(t.files_changed ?? 0),
    })),
  });
  const page = pageOf(rendered, offset);
  if (isFailure(page)) return failureJson(page);
  return {
    session_id: target.id,
    title: target.title,
    status: target.status ?? "idle",
    project: projectOf(target),
    offset: page.offset,
    text: page.text,
    ...(page.nextOffset === null ? { complete: true } : { next_offset: page.nextOffset }),
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

/**
 * The files a message carries: each under the caller's project or worktree,
 * measured by the shell (the web view cannot stat a file). `src-tauri`'s
 * `attached_files` enforces the same roots and the 5 MB ceiling for the paths
 * the pure rule above accepted.
 */
async function attachmentFiles(
  caller: RuleThread,
  paths: readonly string[],
): Promise<AttachedFile[] | Failure> {
  if (!paths.length) return [];
  if (paths.length > MAX_ATTACHMENT_FILES) {
    return failure("invalid", `a message attaches at most ${MAX_ATTACHMENT_FILES} files`);
  }
  const roots = attachmentRoots(caller);
  for (const path of paths) {
    const refused = attachmentRefusal(path, roots);
    if (refused) return refused;
  }
  try {
    return await shellCall<AttachedFile[]>("attached_files", { paths: [...paths], roots });
  } catch (error) {
    return failure("invalid", `the files could not be attached: ${errorText(error)}`);
  }
}

export function message(
  caller: RuleThread,
  targetId: string,
  text: string,
  attachments: readonly string[] = [],
  steerNow = false,
  wait = false,
): Promise<JsonObject> {
  return withLinkGate(() => messageNow(caller, targetId, text, attachments, steerNow, wait));
}

/** Do not message a settled or archived thread. */
function messageTargetRefusal(state: AppState, targetId: string): Failure | null {
  if (targetId in state.archived) {
    return failure("conflict", "that thread is archived; unarchive it before messaging");
  }
  const target = ruleThread(state, targetId);
  if (target && statusOf(state, targetId) === "done") {
    return failure("conflict", "that thread is settled; set it to idle before messaging");
  }
  return null;
}

export async function messageNow(
  caller: RuleThread,
  targetId: string,
  text: string,
  attachments: readonly string[] = [],
  steerNow = false,
  /** `wait: false` is one-way: no link, so the caller never shows Waiting and nothing comes back (issue 3544). */
  wait = false,
): Promise<JsonObject> {
  const state = useApp.getState();
  const target = ruleThread(state, targetId);
  // A thread on another machine is not this app's project topic to check: the
  // user named that machine, and the card below is the gate.
  const remote = isRemoteThread(targetId);
  const refused = checkMessage(caller, target, book(), false, false, delegateLimits(), remote, !wait);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const unavailable = messageTargetRefusal(state, targetId);
  if (unavailable) return failureJson(unavailable);
  // The files are checked before the user is asked about a message that could not carry
  // all of them, so a refusal never costs an approval.
  const files = await attachmentFiles(caller, attachments);
  if (isFailure(files)) return failureJson(files);
  const extras = files.length ? attachedExtras(files) : undefined;
  const withFiles = files.length
    ? `${text}\n\n${files.length === 1 ? "1 file attached" : `${files.length} files attached`}: ${files.map((f) => f.name).join(", ")}`
    : text;
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: steerNow
      ? `Deliver into the running turn if supported; otherwise queue.\n\n${withFiles}`
      : withFiles,
    ...(!sameReach(caller, target) ? { project: projectOf(target) } : {}),
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // The user took their time: look again.
  const afterPermission = useApp.getState();
  const current = ruleThread(afterPermission, targetId);
  const again = checkMessage(caller, current, book(), false, false, delegateLimits(), remote, !wait);
  if (again) return failureJson(again);
  const stopped = messageTargetRefusal(afterPermission, targetId);
  if (stopped) return failureJson(stopped);
  // A thread whose last turn failed, or whose harness process was stopped for idling, is opened
  // and prompted like any other: the engine starts its harness again with the prompt, as it does
  // when the user types there. Only a thread that cannot be opened at all is unavailable.
  const openError = await openThread(target.id);
  const afterOpen = useApp.getState();
  const openedTarget = ruleThread(afterOpen, targetId);
  const noLongerReachable = checkMessage(
    caller,
    openedTarget,
    book(),
    false,
    false,
    delegateLimits(),
    remote,
    !wait,
  );
  if (noLongerReachable) return failureJson(noLongerReachable);
  const noLongerAvailable = messageTargetRefusal(afterOpen, targetId);
  if (noLongerAvailable) return failureJson(noLongerAvailable);
  const thread = afterOpen.threads[target.id];
  if (openError || !thread || threadOpenError(thread)) {
    return failureJson(failure("target_unavailable", openError ?? "that thread could not be opened"));
  }
  const busy = thread.running;
  const session = afterOpen.sessions.find((x) => x.id === target.id);
  const harnessName = session?.harness
    ? afterOpen.harnesses?.harnesses.find((h) => h.id === session.harness)?.name
    : undefined;
  const id = wait
    ? addLink({
        orchestrator: caller.id,
        target: target.id,
        orchestratorTitle: caller.title,
        targetTitle: target.title,
        message: text,
        state: busy ? "queued" : "working",
        harness: session?.harness,
        harnessName,
        model: session?.model,
      })
    : undefined;
  // The sender's id (and the link it waits on) rides under the heading, so the target can tell two
  // senders with the same title apart and the app links the sender's name (issue 3539).
  const sent = delivered(text, caller.title, { session: caller.id, link: id });
  let delivery: "steered" | "queued" | "started";
  if (
    steerNow &&
    busy &&
    sendDecisionFor(thread, extras).action === "steer" &&
    (await steer(target.id, sent, extras, caller.id, id))
  ) {
    delivery = "steered";
    if (id) linkSteered(target.id, id, sent);
  } else {
    const before = new Set(useApp.getState().threads[target.id]?.queue.map((q) => q.id));
    deliver(target.id, sent, id, caller.id, extras);
    delivery = useApp.getState().threads[target.id]?.queue.some((q) => !before.has(q.id))
      ? "queued"
      : "started";
  }
  return {
    ...(id ? { link_id: id } : { return_to: "none" }),
    receipt: delivery,
    delivered: delivery,
    session_id: target.id,
    title: target.title,
    ...(files.length ? { attachments: files.map((f) => f.path) } : {}),
  };
}

export function addLink(link: Omit<Link, "id" | "createdAt" | "events">): string {
  let id = "";
  setBook((b) => {
    const next = nextId(b, link.orchestrator);
    id = next.id;
    const at = Date.now();
    // The first event is what made the link: a thread started on a harness and model, or a message
    // sent into one, which is the line the chip's tip opens with.
    const meta = [link.harnessName ?? link.harness, link.model].filter(Boolean).join(" · ");
    const first = noteEvent({ ...link, id, createdAt: at }, link.started ? "started" : "message", at, meta);
    // Then the state it is in from this moment: a queued message reads `Queued`, a turn that starts
    // at once reads `Working`, without waiting for the next transition to write it.
    const seeded =
      link.state === "queued" || link.state === "working"
        ? noteEvent(first, link.state === "queued" ? "queued" : "working", at)
        : first;
    return { ...next.book, links: [...next.book.links, seeded] };
  });
  return id;
}

/** After the turn the target runs, never into it: the same queue a typed message uses. */
export function deliver(
  target: string,
  text: string,
  link: string | undefined,
  from: string,
  extras?: ComposeExtras,
) {
  const state = useApp.getState();
  if (
    (isContinueMessage(text) || isWakeMessage(text)) &&
    (state.statusMarks[target] ?? sessionById(state, target)?.status) === "done"
  )
    return;
  const thread = useApp.getState().threads[target];
  if (thread?.running) {
    // `from` is who queued it, so that thread can correct it while it waits (`manageQueue`).
    updateThread(target, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text,
          ...(link ? { link } : {}),
          from,
          queuedAt: Date.now(),
          ...(extras ? { extras } : {}),
        },
      ],
    }));
  } else {
    void runPrompt(target, text, extras, { link, sender: from });
  }
}

/** The ids of messages that left a queue in this window: they started, or a person took them out. */
function sentQueueIds(): readonly string[] {
  // The book that records them is read the way start-up reads it, and kept once read (state/host.ts
  // `restoreQueuedThreads`); a window that never restored a queue reads storage here instead.
  return (queueBook ?? parseQueueBook(storedQueues())).acked;
}

/** A queued message that is no longer waiting: it started, or nothing ever had its id. */
function goneFromQueue(queueId: string | undefined): Failure {
  return sentQueueIds().includes(queueId ?? "")
    ? failure("conflict", "that message has already started, so it cannot be changed")
    : failure("not_found", "no queued message of that thread has that queue_id; list them again");
}

/**
 * What a queued message says, without the `delivered` attribution: that line is the app's, so an
 * agent reads its own text back and an edit says only what it says (`manageQueue`).
 */
function bodyOf(item: QueuedPrompt, from: string): string {
  const said = parseDelivered(item.text);
  return said && said.title === from ? said.body : item.text;
}

/** One queued message as `manage_queue` lists it: which one, what it says, and when it waits. */
function listedQueueItem(item: QueuedPrompt, caller: RuleThread): JsonObject {
  return {
    queue_id: item.id,
    preview: bodyOf(item, caller.title).slice(0, QUEUE_PREVIEW_CHARS),
    ...(item.queuedAt ? { queued_at: new Date(item.queuedAt).toISOString() } : {}),
  };
}

type ManageQueueCall = Extract<ReturnType<typeof parseCall>, { tool: "manage_queue" }>;

/**
 * `manage_queue`: what this thread queued on another one while it was busy, and the three things it
 * may still do to those messages. Only a message this thread queued (`from`) is its own; anything
 * else in that queue belongs to the user or to another thread and is reported as a count. A message
 * that has started left the queue and cannot be touched. `steer` delivers one now, by the same rule
 * ⌘⏎ follows (`sendDecisionFor`), so a harness that takes no message mid-turn is refused rather
 * than quietly queued again.
 */
export async function manageQueue(caller: RuleThread, call: ManageQueueCall): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), call.target);
  // Only the caller's own queued messages are exposed below, in any project.
  // Slot, mailbox and chain rules were checked when the message queued.
  const refused = checkMessage(caller, target, book(), true, true);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const waiting = () => useApp.getState().threads[target.id]?.queue ?? [];
  const mine = (id: string | undefined) => waiting().find((q) => q.id === id);

  if (call.action === "list") {
    const thread = useApp.getState().threads[target.id];
    const own = waiting()
      .filter((q) => !q.wake && q.from === caller.id)
      .slice(0, MAX_QUEUE_ITEMS);
    const others = waiting().filter((q) => !q.wake && q.from !== caller.id).length;
    return {
      session_id: target.id,
      title: target.title,
      running: !!thread?.running,
      ...(thread?.queueHeld ? { held: true } : {}),
      queued: own.map((item) => listedQueueItem(item, caller)),
      // What waits there that this thread may not touch: the user's own messages, or another
      // thread's. Count only: their text is not this thread's to read.
      ...(others ? { others } : {}),
    };
  }

  const item = mine(call.queueId);
  if (!item) return failureJson(goneFromQueue(call.queueId));
  if (item.from !== caller.id) {
    return failureJson(failure("not_allowed", "that message was not queued by this thread"));
  }
  const verb =
    call.action === "edit"
      ? "Change a queued message to"
      : call.action === "cancel"
        ? "Drop a queued message from"
        : "Send a queued message now to";
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    ...(!sameReach(caller, target) ? { project: projectOf(target) } : {}),
    detail: `${verb}: ${call.action === "edit" ? (call.text ?? "").trim() : bodyOf(item, caller.title).slice(0, QUEUE_PREVIEW_CHARS)}`,
  });
  if (no) return failureJson(no);
  // The user took their time: the message may have started meanwhile.
  const again = mine(call.queueId);
  if (!again) return failureJson(goneFromQueue(call.queueId));
  if (again.from !== caller.id) {
    return failureJson(failure("not_allowed", "that message was not queued by this thread"));
  }

  switch (call.action) {
    case "edit": {
      const text = (call.text ?? "").trim();
      // Delivered the way `message` delivered it, so the target still reads who asked.
      updateQueued(
        target.id,
        again.id,
        delivered(text, caller.title, { session: caller.id, ...(again.link ? { link: again.link } : {}) }),
      );
      // The link card says what was sent, so it says the new text now.
      const link = again.link;
      if (link) setBook((b) => updateLink(b, link, { message: text }));
      return { session_id: target.id, queue_id: again.id, preview: text.slice(0, QUEUE_PREVIEW_CHARS) };
    }
    case "cancel": {
      // The same removal the row's own ✕ makes: its link never answers, and it cannot come back.
      removeQueued(target.id, again.id);
      return { session_id: target.id, queue_id: again.id, cancelled: true };
    }
    case "steer": {
      const thread = useApp.getState().threads[target.id];
      if (!thread?.running) {
        return failureJson(failure("conflict", "no turn is running in that thread to steer into"));
      }
      const verdict = sendDecisionFor(thread, again.extras);
      if (verdict.action === "interruptRestart") {
        // Stopping the turn to take it is not steering: this call only steers.
        return failureJson(failure("not_allowed", "this harness cannot steer"));
      }
      if (verdict.action === "queue") {
        return failureJson(
          failure(
            "not_allowed",
            verdict.reason === "harness"
              ? "this harness cannot steer"
              : `it cannot go into the turn now: ${QUEUE_REASON[verdict.reason].toLowerCase()}`,
          ),
        );
      }
      if (!(await sendQueuedNow(target.id, again.id))) {
        return failureJson(failure("conflict", "the turn would not take it now, so it is still queued"));
      }
      return { session_id: target.id, queue_id: again.id, steered: true };
    }
  }
}

export type StartCall = Extract<ReturnType<typeof parseCall>, { tool: "start_session" }>;
export type FanOutCall = Extract<ReturnType<typeof parseCall>, { tool: "fan_out" }>;
