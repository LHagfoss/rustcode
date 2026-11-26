// Approvals: answering a thread's permission requests, questions and secrets, the hour-long grants
// the engine keeps, and the access mode each thread runs with. Split out of state/store.ts; the
// state these write (`permissionModes`, `projectPermissionModes`, `grants`, `resolvedApprovals`)
// still lives in the app store.

import type { AcpConnection, JsonObject } from "../acp/jsonrpc";
import {
  Method,
  type PermissionOption,
  type PermissionRequest,
  permissionCancelled,
  permissionSelected,
} from "../acp/types";
import { approvalSubject, optionScope, resolvedLine } from "../lib/approval";
import { type QuestionChoice, shapeQuestionAnswer } from "../lib/questionAnswer";
import { browserGrantId, parseBrowserGrants, parseGrants } from "./grants";
import { linkWaitEnded } from "./host";
import { autoAnswer, type PermissionMode } from "./permissions";
import { projectKey } from "./projectBook";
import { dropQuestionDraft } from "./questionDrafts";
import {
  type AppState,
  connectionFor,
  connectionForThread,
  errorText,
  STORAGE,
  store,
  threadIsRemote,
  toast,
  updateThread,
  useApp,
} from "./store";
import { questionItem, settledWait } from "./thread";
import { LOCAL_HOST } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** One answered approval, kept a moment so its card can settle into a line. */
export interface ResolvedApproval {
  rpcId: number | string;
  /** "Allowed edit src/app.ts · once". */
  text: string;
  allowed: boolean;
  at: number;
}

/**
 * Says the thread's computer is away, once, for a caller that would otherwise answer every
 * pending card with its own refusal. True when there is nothing to answer on.
 */
function refuseAway(key: string): boolean {
  if (connectionForThread(key)) return false;
  toast(threadIsRemote(key) ? "That computer is not connected." : "The server is not connected.", "error");
  return true;
}

/**
 * Answer exactly this request; it leaves the queue before the reply, so a double click cannot
 * answer the next. A question also leaves its answer in the transcript, where the reader can see
 * what was asked and what they said (state/thread.ts `questionItem`).
 */
function reply(
  request: PermissionRequest,
  result: JsonObject,
  answer?: string,
  files: readonly string[] = [],
): boolean {
  // The answer goes back on the connection the request arrived on (`handleInbound`). With the
  // machine away there is nothing to answer on: the card stays, and an answer made now is refused
  // rather than dropped.
  const conn = connectionForThread(request.sessionId);
  if (!conn) {
    refuseAway(request.sessionId);
    return false;
  }
  updateThread(request.sessionId, (t) => {
    const settled = request.question ? t : settledWait(t, request.toolCallId, Date.now());
    return {
      ...settled,
      permissions: settled.permissions.filter((p) => p.rpcId !== request.rpcId),
      items:
        answer || files.length
          ? [...settled.items, questionItem(request, answer ?? "", Date.now(), files)]
          : settled.items,
    };
  });
  if (request.question) dropQuestionDraft(request);
  // Another request of the same thread keeps it waiting; the last one ends the wait.
  if (!get().threads[request.sessionId]?.permissions.length) linkWaitEnded(request.sessionId);
  conn.respond(request.rpcId, result);
  return true;
}

export function answerPermission(request: PermissionRequest, option: PermissionOption | null) {
  if (request.secret) return;
  const answered = reply(
    request,
    option ? permissionSelected(option.id) : permissionCancelled,
    request.question ? (option?.name ?? "Declined") : undefined,
  );
  if (answered && request.question)
    settleLine(request, option ? `Answered: ${option.name}` : "Dismissed the question", !!option);
  if (!answered || request.question) return;
  settleApproval(request, option);
  // An hour's grant and a browser site grant are the engine's to keep: list them again to show them.
  if (option && (optionScope(option) === "hour" || option.id === BROWSER_SITE_OPTION)) void refreshGrants();
}

/** How long an answered card stays as one settled line before it leaves. */
export const RESOLVED_APPROVAL_MS = 1800;

/**
 * The card an answer just closed folds into one line in place ("Allowed edit
 * src/app.ts · once") for a moment, so the answer has a visible result instead
 * of the card vanishing under the pointer.
 */
function settleApproval(request: PermissionRequest, option: PermissionOption | null) {
  const scope = option ? optionScope(option) : "decline";
  const text = option ? resolvedLine(request, option) : `Declined ${approvalSubject(request)}`;
  settleLine(request, text, scope !== "decline");
}

/** The settled line itself, for an approval or an answered question (`ResolvedApprovalBar`). */
function settleLine(request: PermissionRequest, text: string, allowed: boolean) {
  const threadId = request.sessionId;
  const previous = resolvedTimers.get(threadId);
  if (previous) clearTimeout(previous);
  set((s) => ({
    resolvedApprovals: {
      ...s.resolvedApprovals,
      [threadId]: { rpcId: request.rpcId, text, allowed, at: Date.now() },
    },
  }));
  const timer = setTimeout(() => {
    resolvedTimers.delete(threadId);
    set((s) => {
      // A later answer replaced it: that one owns the line and its own timer.
      if (s.resolvedApprovals[threadId]?.rpcId !== request.rpcId) return s;
      const { [threadId]: _gone, ...rest } = s.resolvedApprovals;
      return { resolvedApprovals: rest };
    });
  }, RESOLVED_APPROVAL_MS);
  resolvedTimers.set(threadId, timer);
}

const resolvedTimers = new Map<string, ReturnType<typeof setTimeout>>();

// ---------------------------------------------------------------- timed grants

/**
 * `_rustcode/permissions`: the "Allow for 1 hour" grants still in force, with
 * what each covers and when it ends. An engine without the method lists none.
 */
export async function refreshGrants(conn: AcpConnection | null = connectionFor("")): Promise<void> {
  if (!conn) return;
  try {
    const answer = await conn.request(Method.permissions, {}, 10_000);
    if (connectionFor("") === conn)
      set({ grants: parseGrants(answer), browserGrants: parseBrowserGrants(answer) });
  } catch {
    // An older engine has none to list; what is known stays as it is.
  }
}

/** Revoke one timed grant; the engine answers with the list as it stands then. */
export async function revokeGrant(id: string): Promise<void> {
  const conn = connectionFor("");
  if (!conn) return;
  try {
    const answer = await conn.request(Method.permissionsRevoke, { id }, 10_000);
    if (connectionFor("") === conn)
      set({ grants: parseGrants(answer), browserGrants: parseBrowserGrants(answer) });
  } catch (error) {
    toast(`Could not revoke it: ${errorText(error)}`, "error");
  }
}

/** The option id of a browser card's "Allow <host> for this thread" (engine computer_use/grants.rs). */
const BROWSER_SITE_OPTION = "allow-site";

/**
 * Stop a thread's browser: the engine drops its site grants and closes the browser, and the
 * browser cards still waiting are answered as declined.
 */
export async function stopBrowser(threadId: string): Promise<void> {
  for (const request of get().threads[threadId]?.permissions ?? []) {
    if (request.toolCallId.startsWith("computer-use-")) answerPermission(request, null);
  }
  set((s) => {
    const { [threadId]: _gone, ...rest } = s.browserGrants;
    return { browserGrants: rest };
  });
  await revokeGrant(browserGrantId(threadId));
}

/** Approve every approval the thread waits on (not questions), each with its "allow once" option. */
export function approveAll(threadId: string) {
  // The answers go over the thread's own connection; with the machine away, one refusal is said
  // once, not once per card.
  if (refuseAway(threadId)) return;
  const pending = get().threads[threadId]?.permissions.filter((p) => !p.question) ?? [];
  for (const request of pending) {
    const option =
      request.options.find((o) => o.kind === "allow_once") ??
      request.options.find((o) => o.kind === "allow_always");
    if (option) reply(request, permissionSelected(option.id));
  }
}

// ---------------------------------------------------------------- permission mode

/** The project a thread belongs to, as Settings › Permissions files a project's access. */
function threadProject(s: AppState, threadId: string): string {
  const thread = s.threads[threadId];
  const summary = thread ? undefined : s.sessions.find((x) => x.id === threadId);
  const path =
    thread?.worktree?.repo || thread?.cwd || summary?.worktree?.repo || summary?.repo || summary?.cwd;
  return path ? projectKey(path) : "";
}

/** What a new thread in `project` gets: the project's own default, else the default. */
export function defaultModeFor(s: AppState, project: string | undefined): PermissionMode {
  if (!project) return s.defaultPermissionMode;
  return s.projectPermissionModes[projectKey(project)] ?? s.defaultPermissionMode;
}

/**
 * What `threadId` may do without asking: its own choice, else its project's default, else the
 * default. A thread nobody chose for follows the default as it changes, so raising the default to
 * Full access reaches the threads already open. `cwd` names a new thread's folder.
 */
export function permissionModeOf(s: AppState, threadId: string | undefined, cwd?: string): PermissionMode {
  if (!threadId) return s.newThreadPermissionMode ?? defaultModeFor(s, cwd);
  const own = s.permissionModes[threadId];
  if (own) return own;
  // Most people set no project defaults: no lookup for the thread's folder then.
  if (!Object.keys(s.projectPermissionModes).length) return s.defaultPermissionMode;
  return defaultModeFor(s, threadProject(s, threadId));
}

/** Whether a thread has its own access, rather than following its project's or the default. */
export function hasOwnPermissionMode(s: AppState, threadId: string | undefined): boolean {
  return !!threadId && s.permissionModes[threadId] !== undefined;
}

/** What a thread would follow without its own choice: its project's default, else the default. */
export function inheritedModeOf(s: AppState, threadId: string | undefined, cwd?: string): PermissionMode {
  return defaultModeFor(s, threadId ? threadProject(s, threadId) : cwd);
}

/**
 * Settings › Permissions › Default access: what threads may do without asking, unless a project or
 * the thread itself says otherwise. Threads that follow it change with it, so a thread does not stay
 * at Ask after the user chose Full access. Requests already waiting that it now allows are answered.
 */
export function setDefaultPermissionMode(mode: PermissionMode) {
  if (get().defaultPermissionMode === mode) return;
  set({ defaultPermissionMode: mode });
  store(STORAGE.permissionMode, mode);
  answerAllowedWaiting();
}

/** A project's own default (`null` follows the default again); applied at once, like the default. */
export function setProjectPermissionMode(
  project: string,
  mode: PermissionMode | null,
  options: { fromServer?: boolean } = {},
) {
  const key = projectKey(project);
  if (!key) return;
  const modes = { ...get().projectPermissionModes };
  if (mode) modes[key] = mode;
  else delete modes[key];
  set({ projectPermissionModes: modes });
  store(STORAGE.projectPermissionModes, JSON.stringify(modes));
  answerAllowedWaiting();
  // The same project override Settings › Applying to › a project edits, on this machine's server,
  // so the engine resolves a thread's access through it too. Not echoed back when it came from there.
  if (!options.fromServer)
    void import("./settingsScope")
      .then((scope) => scope.writeProjectSetting(LOCAL_HOST, key, "default-permission-mode", mode))
      .catch(() => undefined);
}

/**
 * Set what `threadId` may do without asking, from the composer's pill: only that thread, or (for a
 * thread not started yet) the next one. `null` drops the thread's own choice, so it follows its
 * project's default again. Requests already waiting that the mode now allows are answered.
 */
export function setPermissionMode(threadId: string | undefined, mode: PermissionMode | null) {
  if (!threadId) {
    set({ newThreadPermissionMode: mode });
    return;
  }
  const modes = { ...get().permissionModes };
  if (mode) modes[threadId] = mode;
  else delete modes[threadId];
  set({ permissionModes: modes });
  store(STORAGE.permissionModes, JSON.stringify(modes));
  answerWaiting(threadId);
}

/** Drop the own access of every thread listed, so each follows its project's default again. */
export function clearPermissionModes(threadIds: readonly string[]) {
  const modes = { ...get().permissionModes };
  for (const id of threadIds) delete modes[id];
  set({ permissionModes: modes });
  store(STORAGE.permissionModes, JSON.stringify(modes));
  answerAllowedWaiting();
}

/** Answer what `threadId` waits on that its access now allows. */
function answerWaiting(threadId: string) {
  const pending = get().threads[threadId]?.permissions ?? [];
  if (!pending.length) return;
  // With the machine away nothing can be answered: one refusal, not one per pending card.
  if (refuseAway(threadId)) return;
  const mode = permissionModeOf(get(), threadId);
  for (const request of pending) {
    const option = autoAnswer(request, mode);
    if (option) reply(request, permissionSelected(option.id));
  }
}

/** After a default changed: every waiting thread answers what its access now allows. */
function answerAllowedWaiting() {
  for (const [id, thread] of Object.entries(get().threads)) if (thread.permissions.length) answerWaiting(id);
}

/** Where a requested secret goes: a dotenv file, the thread's command environment, or both. */
export function secretTargets(secret: NonNullable<PermissionRequest["secret"]>): string[] {
  const targets = secret.destination ? [`saved to ${secret.destination}`] : [];
  if (secret.env) targets.push(`$${secret.name} set for this thread's commands, in memory only`);
  return targets;
}

/** Secret values travel only on the originating connection, never into items or drafts. */
export function answerSecret(request: PermissionRequest, value: string | null): boolean {
  if (!request.secret) return false;
  const result: JsonObject =
    value === null
      ? permissionCancelled
      : {
          ...permissionSelected("secret_save"),
          _meta: { rustcode: { secret: value } },
        };
  const answered = reply(request, result);
  if (answered) {
    const text =
      value === null
        ? "Declined"
        : `Secret ${request.secret.name}: ${secretTargets(request.secret).join("; ")}`;
    set((s) => ({
      resolvedApprovals: {
        ...s.resolvedApprovals,
        [request.sessionId]: { rpcId: request.rpcId, text, at: Date.now(), allowed: value !== null },
      },
    }));
  }
  return answered;
}

/**
 * Answer a question with what its form holds: options, words, files, or a file beside either
 * (`lib/questionAnswer.ts`). False when nothing was sent, so the form keeps its draft: it held no
 * answer, or the thread's computer is away.
 */
export function answerQuestion(request: PermissionRequest, choice: QuestionChoice): boolean {
  if (request.secret) return false;
  const shaped = shapeQuestionAnswer(request, choice);
  if (!shaped) return false;
  if (!reply(request, shaped.result, shaped.answer, shaped.files)) return false;
  const said = [shaped.answer, shaped.files.length ? plural(shaped.files.length, "file") : ""]
    .filter(Boolean)
    .join(" · ");
  settleLine(request, `Answered: ${said}`, true);
  return true;
}

export function answerQuestionOptions(request: PermissionRequest, options: PermissionOption[]) {
  answerQuestion(request, { options, text: "", attachments: [] });
}

export function answerQuestionText(request: PermissionRequest, text: string) {
  answerQuestion(request, { options: [], text, attachments: [] });
}
