import type { Json, JsonObject } from "../acp/jsonrpc";
import { threadReference } from "../lib/deepLinks";
import { MAX_ATTACHMENT_FILES } from "./attachments";
import type { PermissionMode } from "./permissions";
import { snoozeAt } from "./snooze";

// The orchestration tool calls a thread can make (`Call`), checked and parsed from the MCP
// arguments (`parseCall`, `parseSchedule`). Split out of orchestration.ts, which re-exports it.

/** The longest prompt or message a thread may hand another one (32 KiB). */
export const MAX_MESSAGE_BYTES = 32 * 1024;

/**
 * A wait returns as soon as a link finishes, so a long default costs nothing; a short one makes the
 * orchestrator ask again, and each ask resends its whole context (the engine's `DEFAULT_WAIT_MS`).
 */
export const DEFAULT_WAIT_MS = 20 * 60 * 1000;
export const MAX_WAIT_MS = 25 * 60 * 1000;
export const MAX_LAST_TURNS = 10;

export interface Failure {
  code: string;
  message: string;
  /** How many more links the thread can open: sent with a `busy` refusal. */
  linksLeft?: number;
}

export const failure = (code: string, message: string, linksLeft?: number): Failure => ({
  code,
  message,
  ...(linksLeft === undefined ? {} : { linksLeft }),
});
/** What a failure says when whoever made it gave no words: never an empty reason (issue 3711). */
export const failureMessage = (f: Failure): string =>
  f.message.trim() ||
  (f.code === "denied"
    ? "the user did not allow it, or dismissed the approval"
    : `it failed (${f.code}) and no reason was reported`);

export const failureJson = (f: Failure): JsonObject => ({
  error: {
    code: f.code,
    message: failureMessage(f),
    ...(f.linksLeft === undefined ? {} : { links_left: f.linksLeft }),
  },
});

export interface HarnessChoice {
  harness: string;
  model?: string;
  effort?: string;
  /** Named login of that harness (`HarnessAccount.id`); omitted inherits it. */
  account?: string;
}

export type Call =
  | { tool: "list_sessions"; query: string; allProjects: boolean; machine?: string }
  | { tool: "read_session"; target: string; lastTurns: number; offset: number; machine?: string }
  /** `attachments` are absolute paths, under the caller's project or worktree. */
  | {
      tool: "message_session";
      target: string;
      text: string;
      attachments: string[];
      steer?: boolean;
      /** `false`: one-way, with no link and nothing delivered back. Waits when omitted. */
      wait?: boolean;
      machine?: string;
    }
  /** `callId` is the engine's name for this call, so a dropped call can end its wait. */
  | {
      tool: "wait_session";
      links: string[];
      timeoutMs: number;
      all?: boolean;
      callId?: string;
      machine?: string;
    }
  | {
      tool: "start_session";
      prompt: string;
      title?: string;
      harness?: string;
      model?: string;
      effort?: string;
      /** Named login of that harness; omitted is the caller's own, else a new thread's default. */
      account?: string;
      worktree?: boolean;
      /** Lowers the new thread's access (a delegate preset's, issue 3391); never raises it. */
      access?: PermissionMode;
      /**
       * A thread of the user's own at the top level, not a delegate (issue 3443):
       * no parent link, nothing delivered back, `wait_session` does not apply.
       */
      standalone?: boolean;
      project?: string;
      /** The paired machine it runs on; omitted is this computer. */
      machine?: string;
    }
  | {
      tool: "fan_out";
      prompt: string;
      title?: string;
      harnesses: HarnessChoice[];
      /** Lowers each new thread's access, as on `start_session`. */
      access?: PermissionMode;
      /** Each is a thread of the user's own at the top level, not a delegate. */
      standalone?: boolean;
      /** The project on that machine; without one, the caller's path there or its only project. */
      project?: string;
      machine?: string;
    }
  /** `harness` narrows to one harness, `model` to the models whose id or name matches. */
  | { tool: "list_harnesses"; harness?: string; model?: string; machine?: string }
  | { tool: "search_sessions"; query: string; allProjects: boolean; machine?: string }
  | { tool: "list_worktrees"; allProjects: boolean; machine?: string }
  | {
      tool: "update_session";
      target?: string;
      title?: string;
      pinned?: boolean;
      unread?: boolean;
      status?: SessionStatusChoice;
      /** When it comes back, in ms; `null` brings it back now. */
      snoozedUntil?: number | null;
      machine?: string;
    }
  | { tool: "interrupt_session"; target: string; machine?: string }
  | { tool: "configure_session"; target?: string; model?: string; effort?: string; machine?: string }
  | { tool: "transfer_session"; target: string; harness: string; account?: string; model?: string }
  | { tool: "list_pending_requests"; machine?: string }
  | {
      tool: "answer_request";
      target: string;
      requestId: string;
      /** For an approval: how far the answer reaches. */
      choice?: ApprovalChoice;
      /** For a question: free text, or the id of one of its options. */
      answer?: string;
      machine?: string;
    }
  | {
      tool: "organize_threads";
      targets: string[];
      title?: string;
      pinned?: boolean;
      unread?: boolean;
      /** `true` settles the threads, `false` brings them back. */
      archived?: boolean;
    }
  | { tool: "list_projects"; machine?: string }
  | { tool: "list_pull_requests"; state: PullStateChoice; machine?: string }
  | {
      tool: "read_activity";
      target: string;
      after?: string;
      limit: number;
      order?: "newest";
      machine?: string;
    }
  | {
      tool: "manage_queue";
      target: string;
      action: QueueAction;
      /** One of this thread's own queued messages, from `list`. */
      queueId?: string;
      /** What it says instead, for `edit`. */
      text?: string;
      machine?: string;
    }
  | { tool: "compact_session"; target?: string; resume: boolean }
  | { tool: "thread_status"; target?: string; machine?: string }
  | { tool: "merge_when_green"; target: string; number?: number; timeoutMs: number; callId?: string }
  | { tool: "cleanup_worktree"; target: string; settle: boolean }
  /** Where worktrees stand: one thread's, or every worktree this thread reaches. */
  | { tool: "worktree_status"; target?: string; allProjects: boolean; machine?: string }
  /** Move a thread's work between the user's checkout and a worktree of its own. */
  | { tool: "handoff_worktree"; target: string; to: HandoffTarget }
  /** The computers this RustCode is paired with (orchestration version 12). */
  | { tool: "list_machines"; probe: boolean }
  /** Run a shell command on any machine in the fleet (orchestration version 22). */
  | { tool: "run_machine_command"; command: string; cwd?: string; timeoutSecs?: number; machine?: string }
  | { tool: "read_project"; path: string; machine?: string }
  | { tool: "create_project"; path: string; init?: boolean }
  | { tool: "clone_project"; url: string; parent?: string; name?: string; machine?: string }
  | {
      tool: "update_project";
      path: string;
      destination?: string;
      name?: string;
      harness?: string;
      model?: string;
    }
  | { tool: "remove_project"; path: string }
  | {
      tool: "schedule_task";
      prompt: string;
      schedule: TaskSchedule;
      project?: string;
      name?: string;
      modelProfile?: string;
    }
  | {
      tool: "create_loop";
      name: string;
      prompt: string;
      schedule: TaskSchedule;
      project?: string;
      harness?: string;
      account?: string;
      model?: string;
      effort?: string;
      threadId?: string;
      readOnly: boolean;
    }
  | {
      tool: "update_loop";
      id: string;
      paused?: boolean;
      name?: string;
      prompt?: string;
      schedule?: TaskSchedule | { kind: "manual" };
      harness?: string;
      account?: string;
      model?: string;
      effort?: string;
      threadId?: string;
      readOnly?: boolean;
    }
  | { tool: "list_scheduled_tasks" }
  | { tool: "delete_scheduled_task"; id: string }
  | { tool: "run_scheduled_task_now"; id: string }
  | {
      tool: "computer_desktop";
      desktopTool: ComputerTool;
      args: JsonObject;
      machine?: string;
      callId?: string;
    };

/** The desktop-control tools that may route to another machine's desktop (version 23). */
export type ComputerTool =
  | "computer_windows"
  | "computer_snapshot"
  | "computer_screenshot"
  | "computer_click"
  | "computer_type"
  | "computer_key"
  | "computer_scroll";

/** What `update_session` may set a thread's status to. */
export type SessionStatusChoice = "active" | "review" | "settled";
const SESSION_STATUSES: readonly string[] = ["active", "review", "settled"];

/** What `answer_request` may answer an approval with. */
export type ApprovalChoice = "allow_once" | "allow_always" | "deny";
const APPROVAL_CHOICES: readonly string[] = ["allow_once", "allow_always", "deny"];
/** Threads one `organize_threads` call changes. */
export const MAX_ORGANIZED_THREADS = 50;

/** What `list_pull_requests` may ask for. */
export type PullStateChoice = "open" | "closed" | "merged" | "all";
const PULL_STATES: readonly string[] = ["open", "closed", "merged", "all"];
/** Items `read_activity` returns by default, and at most. */
export const DEFAULT_ACTIVITY_ITEMS = 50;
export const MAX_ACTIVITY_ITEMS = 200;

/** Where `handoff_worktree` may move a thread's work. */
export type HandoffTarget = "checkout" | "worktree";
const HANDOFF_TARGETS: readonly string[] = ["checkout", "worktree"];

/** What `manage_queue` may do to a message this thread queued on another one. */
export type QueueAction = "list" | "edit" | "cancel" | "steer";
const QUEUE_ACTIONS: readonly string[] = ["list", "edit", "cancel", "steer"];
/** Queued messages one `manage_queue` list carries: a queue holds a handful. */
export const MAX_QUEUE_ITEMS = 50;
/** How much of a queued message a list shows: enough to know which one it is. */
export const QUEUE_PREVIEW_CHARS = 200;

/** What `schedule_task` may ask the scheduler for: a cron, or one time. */
export type TaskSchedule =
  | { kind: "cron"; expression: string; timezone?: string }
  | { kind: "at"; at: string };

/** How long `merge_when_green` waits for checks by default, and at most: 25 minutes a call, like `wait_session`. */
export const DEFAULT_MERGE_MS = MAX_WAIT_MS;
export const MAX_MERGE_MS = MAX_WAIT_MS;

/** Pull requests listed, and projects: an agent's context pays for each. */
export const MAX_PULL_REQUESTS = 30;
export const MAX_PROJECTS = 50;
/** Delegates `thread_status` reports in one answer: one gh read each, run together. */
export const MAX_DELEGATES = 12;
/** Worktree threads `worktree_status` reports in one answer. */
export const MAX_WORKTREES = 20;
/** Pull requests `worktree_status` reads: one gh read each, run together, capped like `thread_status`. */
export const MAX_WORKTREE_PULLS = MAX_DELEGATES;

const s = (v: Json | undefined) => (typeof v === "string" ? v : undefined);
/** `all_projects`: the user's threads in every project, when the setting allows it. */
const allProjects = (args: JsonObject) => args.all_projects === true;
export const bytes = (text: string) => new TextEncoder().encode(text).length;
/** The paired machine a call acts on: an id or name `list_machines` gave, absent for this one. */
const machineOf = (args: JsonObject) => s(args.machine)?.trim() || undefined;
/** The optional field, present only when the call named a machine. */
/**
 * The access a call asks its new thread to start at (a delegate preset's,
 * issue 3391). It only ever lowers what the thread would get: the caller's own limit
 * and Default access still bound it.
 */
const accessArg = (value: Json | undefined): PermissionMode | undefined =>
  value === "ask" || value === "edits" || value === "full" ? value : undefined;

const machineField = (args: JsonObject) => {
  const machine = machineOf(args);
  return machine ? { machine } : {};
};
/**
 * A list of paths, at most `max`: the strings that are one, or null when the
 * value is not a list of paths within it.
 */
function pathsOf(value: Json | undefined, max = MAX_ATTACHMENT_FILES): string[] | null {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value) || value.length > max) return null;
  const paths = value.flatMap((one) => (typeof one === "string" && one.trim() ? [one.trim()] : []));
  return paths.length === value.length ? paths : null;
}

/** Accept the same reference from Copy Thread ID and Copy Link. */
function sessionReference(value: unknown): string | undefined {
  return typeof value === "string" ? threadReference(value) : undefined;
}

/** The call in a request's params, or the failure to answer with. */
export function parseCall(params: JsonObject): Call | Failure {
  const raw = params.args;
  const args = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  const tool = s(params.tool) ?? "";
  switch (tool) {
    case "list_sessions":
      return {
        tool: "list_sessions",
        query: s(args.query) ?? "",
        allProjects: allProjects(args),
        ...machineField(args),
      };
    case "list_harnesses": {
      const harness = s(args.harness)?.trim() || undefined;
      const model = s(args.model)?.trim() || undefined;
      return {
        tool: "list_harnesses",
        ...(harness ? { harness } : {}),
        ...(model ? { model } : {}),
        ...machineField(args),
      };
    }
    case "list_worktrees":
      return { tool: "list_worktrees", allProjects: allProjects(args), ...machineField(args) };
    case "worktree_status": {
      const target = sessionReference(args.session_id)?.trim() || undefined;
      return {
        tool: "worktree_status",
        ...(target ? { target } : {}),
        allProjects: allProjects(args),
        ...machineField(args),
      };
    }
    case "handoff_worktree": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      // The direction an orchestrator means when it says "the user takes over"
      // is the checkout, so it is the default.
      const to = s(args.to) ?? "checkout";
      if (!HANDOFF_TARGETS.includes(to)) {
        return failure("invalid", `to must be one of ${HANDOFF_TARGETS.join(", ")}`);
      }
      return { tool: "handoff_worktree", target, to: to as HandoffTarget };
    }
    case "list_projects":
      return { tool: "list_projects", ...machineField(args) };
    case "list_machines":
      return { tool: "list_machines", probe: args.probe === true };
    case "run_machine_command": {
      const command = (s(args.command) ?? "").trim();
      if (!command) return failure("invalid", "command is required");
      if (bytes(command) > MAX_MESSAGE_BYTES) return failure("invalid", "command is over 32 KiB");
      const cwd = s(args.cwd)?.trim() || undefined;
      if (cwd && !/^(\/|\\\\|[a-zA-Z]:[\\/])/.test(cwd)) {
        return failure("invalid", "cwd must be an absolute directory path");
      }
      let timeoutSecs: number | undefined;
      if (args.timeout_secs !== undefined && args.timeout_secs !== null) {
        if (
          typeof args.timeout_secs !== "number" ||
          !Number.isInteger(args.timeout_secs) ||
          args.timeout_secs < 1 ||
          args.timeout_secs > 300
        ) {
          return failure("invalid", "timeout_secs must be an integer between 1 and 300");
        }
        timeoutSecs = args.timeout_secs;
      }
      return {
        tool: "run_machine_command",
        command,
        ...(cwd ? { cwd } : {}),
        ...(timeoutSecs !== undefined ? { timeoutSecs } : {}),
        ...machineField(args),
      };
    }
    case "list_pull_requests": {
      const state = s(args.state) ?? "open";
      if (!PULL_STATES.includes(state)) {
        return failure("invalid", `state must be one of ${PULL_STATES.join(", ")}`);
      }
      return { tool: "list_pull_requests", state: state as PullStateChoice, ...machineField(args) };
    }
    case "search_sessions": {
      const query = (s(args.query) ?? "").trim();
      if (!query) return failure("invalid", "query is required");
      return { tool: "search_sessions", query, allProjects: allProjects(args), ...machineField(args) };
    }
    case "interrupt_session": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      return { tool: "interrupt_session", target, ...machineField(args) };
    }
    case "transfer_session": {
      const target = sessionReference(args.session_id) ?? "";
      const harness = s(args.harness) ?? "";
      if (!target || !harness) return failure("invalid", "session_id and harness are required");
      return {
        tool: "transfer_session",
        target,
        harness,
        account: s(args.account)?.trim() || undefined,
        model: s(args.model)?.trim() || undefined,
      };
    }
    case "list_pending_requests":
      return { tool: "list_pending_requests", ...machineField(args) };
    case "answer_request": {
      const target = sessionReference(args.session_id) ?? "";
      // The list prints `request_id` as a string, and the engine refuses anything else
      // (`orchestration.rs`: a name is a non-empty string), so a number is refused here too.
      const requestId = (s(args.request_id) ?? "").trim();
      if (!target || !requestId) {
        return failure(
          "invalid",
          "session_id and request_id (as list_pending_requests prints it) are required",
        );
      }
      const choice = s(args.choice);
      const answer = s(args.answer);
      if (choice && !APPROVAL_CHOICES.includes(choice)) {
        return failure("invalid", `choice must be one of ${APPROVAL_CHOICES.join(", ")}`);
      }
      if (choice && answer) {
        return failure("invalid", "give choice for an approval or answer for a question, not both");
      }
      if (!choice && !answer?.trim()) {
        return failure("invalid", "choice (an approval) or answer (a question) is required");
      }
      return {
        tool: "answer_request",
        target,
        requestId,
        ...(choice ? { choice: choice as ApprovalChoice } : {}),
        ...(answer ? { answer: answer.trim() } : {}),
        ...machineField(args),
      };
    }
    case "organize_threads": {
      const targets = pathsOf(args.session_ids, MAX_ORGANIZED_THREADS);
      if (!targets?.length) {
        return failure("invalid", `session_ids must be 1 to ${MAX_ORGANIZED_THREADS} thread ids`);
      }
      const title = s(args.title)?.trim();
      const pinned = typeof args.pinned === "boolean" ? args.pinned : undefined;
      const unread = typeof args.unread === "boolean" ? args.unread : undefined;
      const archived = typeof args.archived === "boolean" ? args.archived : undefined;
      if (!title && pinned === undefined && unread === undefined && archived === undefined) {
        return failure("invalid", "give at least one of title, pinned, unread or archived");
      }
      return {
        tool: "organize_threads",
        targets: [...new Set(targets)],
        title: title || undefined,
        pinned,
        unread,
        archived,
      };
    }
    case "compact_session": {
      const target = sessionReference(args.session_id)?.trim();
      if (args.session_id != null && !target)
        return failure("invalid", "session_id must be a non-empty thread id");
      if (args.resume !== undefined && typeof args.resume !== "boolean")
        return failure("invalid", "resume must be true or false");
      return { tool: "compact_session", target, resume: args.resume === true };
    }
    case "thread_status": {
      const target = sessionReference(args.session_id)?.trim() || undefined;
      return { tool: "thread_status", target, ...machineField(args) };
    }
    case "merge_when_green": {
      const target = sessionReference(args.session_id) ?? "";
      const number =
        typeof args.number === "number" && Number.isSafeInteger(args.number) && args.number > 0
          ? args.number
          : undefined;
      if ((!target && !number) || (target && args.number !== undefined))
        return failure("invalid", "give session_id or a positive PR number");
      const asked = typeof args.timeout_ms === "number" ? Math.floor(args.timeout_ms) : DEFAULT_MERGE_MS;
      return {
        tool: "merge_when_green",
        target,
        ...(number ? { number } : {}),
        timeoutMs: Math.min(Math.max(asked, 1), MAX_MERGE_MS),
        callId: s(params.callId),
      };
    }
    case "cleanup_worktree": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      // Settling after cleanup is the default; a caller can keep the thread as it is.
      return {
        tool: "cleanup_worktree",
        target,
        settle: typeof args.settle === "boolean" ? args.settle : true,
      };
    }
    case "read_activity": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const after = s(args.after)?.trim() || undefined;
      if (args.order != null && args.order !== "oldest" && args.order !== "newest") {
        return failure("invalid", "order must be oldest or newest");
      }
      const asked = typeof args.limit === "number" ? Math.floor(args.limit) : DEFAULT_ACTIVITY_ITEMS;
      // Clamped, not refused: an agent asking for 500 items gets the 200 cap.
      return {
        tool: "read_activity",
        target,
        after,
        ...(args.order === "newest" ? { order: "newest" as const } : {}),
        limit: Math.min(Math.max(asked, 1), MAX_ACTIVITY_ITEMS),
        ...machineField(args),
      };
    }
    case "manage_queue": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const action = s(args.action) ?? "";
      if (!QUEUE_ACTIONS.includes(action)) {
        return failure("invalid", `action must be one of ${QUEUE_ACTIONS.join(", ")}`);
      }
      if (action === "list") {
        return { tool: "manage_queue", target, action: action as QueueAction, ...machineField(args) };
      }
      const queueId = (s(args.queue_id) ?? "").trim();
      if (!queueId) return failure("invalid", `queue_id is required to ${action}`);
      const text = s(args.text) ?? "";
      if (action === "edit") {
        if (!text.trim()) return failure("invalid", "text is required to edit");
        if (bytes(text) > MAX_MESSAGE_BYTES) return failure("invalid", "the message is over 32 KiB");
        return {
          tool: "manage_queue",
          target,
          action: action as QueueAction,
          queueId,
          text,
          ...machineField(args),
        };
      }
      return { tool: "manage_queue", target, action: action as QueueAction, queueId, ...machineField(args) };
    }
    case "configure_session":
      return {
        tool: "configure_session",
        target: sessionReference(args.session_id),
        model: s(args.model),
        effort: s(args.effort),
        ...machineField(args),
      };
    case "update_session": {
      const status = s(args.status);
      if (status && !SESSION_STATUSES.includes(status)) {
        return failure("invalid", `status must be one of ${SESSION_STATUSES.join(", ")}`);
      }
      const title = s(args.title)?.trim();
      const pinned = typeof args.pinned === "boolean" ? args.pinned : undefined;
      const unread = typeof args.unread === "boolean" ? args.unread : undefined;
      let snoozedUntil: number | null | undefined;
      if (args.snoozed_until === null) snoozedUntil = null;
      else if (args.snoozed_until !== undefined) {
        snoozedUntil = snoozeAt(s(args.snoozed_until) ?? "", Date.now());
        if (Number.isNaN(snoozedUntil))
          return failure(
            "invalid",
            "snoozed_until must be 1h, evening, tomorrow, next_week or an ISO 8601 time",
          );
      }
      if (!title && pinned === undefined && unread === undefined && !status && snoozedUntil === undefined) {
        return failure("invalid", "give at least one of title, pinned, unread, status or snoozed_until");
      }
      return {
        tool: "update_session",
        target: sessionReference(args.session_id),
        title,
        pinned,
        unread,
        status: status as SessionStatusChoice | undefined,
        snoozedUntil,
        ...machineField(args),
      };
    }
    case "read_session": {
      const target = sessionReference(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const n = typeof args.last_turns === "number" ? Math.floor(args.last_turns) : 3;
      if (n < 1 || n > MAX_LAST_TURNS) return failure("invalid", `last_turns must be 1 to ${MAX_LAST_TURNS}`);
      const offset = typeof args.offset === "number" ? Math.floor(args.offset) : 0;
      // Bytes into the rendered output: a page is a UTF-8 slice of it, never a character split.
      if (!Number.isFinite(offset) || offset < 0)
        return failure("invalid", "offset must be a whole number of bytes");
      return { tool: "read_session", target, lastTurns: n, offset, ...machineField(args) };
    }
    case "message_session": {
      const target = sessionReference(args.session_id) ?? "";
      const text = s(args.message) ?? "";
      if (!target || !text.trim()) return failure("invalid", "session_id and message are required");
      if (bytes(text) > MAX_MESSAGE_BYTES) return failure("invalid", "the message is over 32 KiB");
      const attachments = pathsOf(args.attachments);
      if (attachments === null) {
        return failure("invalid", `attachments must be at most ${MAX_ATTACHMENT_FILES} absolute paths`);
      }
      if (args.steer !== undefined && typeof args.steer !== "boolean") {
        return failure("invalid", "steer must be a boolean");
      }
      if (args.return_to != null && args.return_to !== "me" && args.return_to !== "none") {
        return failure("invalid", "return_to must be me or none");
      }
      return {
        tool: "message_session",
        target,
        text,
        attachments,
        ...(args.steer !== undefined ? { steer: args.steer } : {}),
        wait: args.return_to === "me",
        ...machineField(args),
      };
    }
    case "wait_session": {
      // One answer per link: a repeated id is asked about once.
      const links = Array.isArray(args.link_ids)
        ? [...new Set(args.link_ids.flatMap((x) => (typeof x === "string" ? [x] : [])))]
        : [];
      const one = s(args.link_id);
      if (one && !links.includes(one)) links.unshift(one);
      if (!links.length) return failure("invalid", "link_id is required");
      const asked = typeof args.timeout_ms === "number" ? args.timeout_ms : DEFAULT_WAIT_MS;
      return {
        tool: "wait_session",
        links,
        timeoutMs: Math.min(Math.max(asked, 1), MAX_WAIT_MS),
        ...(args.all === true ? { all: true } : {}),
        callId: s(params.callId),
        ...machineField(args),
      };
    }
    case "start_session": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      return {
        tool: "start_session",
        prompt,
        title: s(args.title)?.trim() || undefined,
        harness: s(args.harness),
        model: s(args.model),
        effort: s(args.effort),
        account: s(args.account)?.trim() || undefined,
        worktree: typeof args.worktree === "boolean" ? args.worktree : undefined,
        access: accessArg(args.access),
        // A thread of the user's own, not a delegate: no link, nothing delivered back.
        standalone: args.standalone === true ? true : undefined,
        project: s(args.project),
        ...machineField(args),
      };
    }
    case "fan_out": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      const list = Array.isArray(args.harnesses) ? args.harnesses : [];
      const harnesses: HarnessChoice[] = [];
      for (const entry of list) {
        const o = entry && typeof entry === "object" && !Array.isArray(entry) ? entry : {};
        const harness = s(o.harness);
        if (!harness) return failure("invalid", "each harnesses entry needs a harness");
        harnesses.push({
          harness,
          model: s(o.model),
          effort: s(o.effort),
          account: s(o.account)?.trim() || undefined,
        });
      }
      if (harnesses.length < 2 || harnesses.length > 4)
        return failure("invalid", "harnesses takes 2 to 4 entries");
      return {
        tool: "fan_out",
        prompt,
        title: s(args.title)?.trim() || undefined,
        harnesses,
        access: accessArg(args.access),
        standalone: args.standalone === true ? true : undefined,
        project: s(args.project),
        ...machineField(args),
      };
    }
    case "read_project":
    case "remove_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      return tool === "read_project"
        ? { tool: "read_project", path, ...machineField(args) }
        : { tool: "remove_project", path };
    }
    case "create_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      return {
        tool: "create_project",
        path,
        init: typeof args.init === "boolean" ? args.init : undefined,
      };
    }
    case "clone_project": {
      const url = (s(args.url) ?? "").trim();
      if (!url) return failure("invalid", "url is required");
      return {
        tool: "clone_project",
        url,
        parent: s(args.parent)?.trim() || undefined,
        name: s(args.name)?.trim() || undefined,
        ...machineField(args),
      };
    }
    case "update_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      const change = {
        destination: s(args.destination)?.trim() || undefined,
        name: s(args.name)?.trim() || undefined,
        harness: s(args.harness)?.trim() || undefined,
        model: s(args.model)?.trim() || undefined,
      };
      if (!change.destination && !change.name && !change.harness && !change.model) {
        return failure("invalid", "give at least one of destination, name, harness or model");
      }
      return { tool: "update_project", path, ...change };
    }
    case "create_loop":
    case "schedule_task": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      const raw =
        args.schedule && typeof args.schedule === "object" && !Array.isArray(args.schedule)
          ? (args.schedule as Record<string, unknown>)
          : {};
      const schedule = parseSchedule(raw);
      if (typeof schedule === "string") return failure("invalid", schedule);
      if (tool === "create_loop") {
        const name = s(args.name)?.trim();
        if (!name) return failure("invalid", "name is required");
        const harness = s(args.harness)?.trim() || undefined;
        const account = s(args.account)?.trim() || undefined;
        const model = s(args.model)?.trim() || undefined;
        const effort = s(args.effort)?.trim() || undefined;
        const threadId = s(args.thread_id)?.trim() || undefined;
        if (account && !harness) return failure("invalid", "account requires a harness");
        if (threadId && (harness || account || model || effort)) {
          return failure("invalid", "an existing thread uses its own harness, account, model and effort");
        }
        if (args.read_only !== undefined && typeof args.read_only !== "boolean") {
          return failure("invalid", "read_only must be a boolean");
        }
        return {
          tool,
          name,
          prompt,
          schedule,
          project: s(args.project)?.trim() || undefined,
          harness,
          account,
          model,
          effort,
          threadId,
          readOnly: args.read_only !== false,
        };
      }
      return {
        tool: "schedule_task",
        prompt,
        schedule,
        project: s(args.project)?.trim() || undefined,
        name: s(args.name)?.trim() || undefined,
        modelProfile: s(args.model_profile)?.trim() || undefined,
      };
    }
    case "update_loop": {
      const id = (s(args.id) ?? "").trim();
      if (!id) return failure("invalid", "id is required");
      if (args.paused !== undefined && typeof args.paused !== "boolean") {
        return failure("invalid", "paused must be a boolean");
      }
      if (args.read_only !== undefined && typeof args.read_only !== "boolean") {
        return failure("invalid", "read_only must be a boolean");
      }
      let prompt: string | undefined;
      if (args.prompt !== undefined) {
        prompt = s(args.prompt) ?? "";
        if (!prompt.trim()) return failure("invalid", "prompt must not be empty");
        if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      }
      let schedule: TaskSchedule | { kind: "manual" } | undefined;
      if (args.schedule !== undefined) {
        const raw =
          args.schedule && typeof args.schedule === "object" && !Array.isArray(args.schedule)
            ? (args.schedule as Record<string, unknown>)
            : {};
        if (raw.kind === "manual") {
          schedule = { kind: "manual" };
        } else {
          const parsed = parseSchedule(raw);
          if (typeof parsed === "string") return failure("invalid", parsed);
          schedule = parsed;
        }
      }
      const name = s(args.name)?.trim() || undefined;
      const harness = s(args.harness)?.trim() || undefined;
      const account = s(args.account)?.trim() || undefined;
      const model = s(args.model)?.trim() || undefined;
      const effort = s(args.effort)?.trim() || undefined;
      const threadId = s(args.thread_id)?.trim() || undefined;
      if (threadId && (harness || account || model || effort)) {
        return failure("invalid", "an existing thread uses its own harness, account, model and effort");
      }
      if (
        args.paused === undefined &&
        name === undefined &&
        prompt === undefined &&
        schedule === undefined &&
        args.read_only === undefined &&
        harness === undefined &&
        account === undefined &&
        model === undefined &&
        effort === undefined &&
        threadId === undefined
      ) {
        return failure("invalid", "give at least one field to update");
      }
      return {
        tool: "update_loop",
        id,
        ...(args.paused !== undefined ? { paused: args.paused } : {}),
        ...(name ? { name } : {}),
        ...(prompt ? { prompt } : {}),
        ...(schedule ? { schedule } : {}),
        ...(harness ? { harness } : {}),
        ...(account ? { account } : {}),
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
        ...(threadId ? { threadId } : {}),
        ...(args.read_only !== undefined ? { readOnly: args.read_only } : {}),
      };
    }
    case "list_scheduled_tasks":
      return { tool: "list_scheduled_tasks" };
    case "delete_scheduled_task":
    case "run_scheduled_task_now": {
      const id = (s(args.id) ?? "").trim();
      if (!id) return failure("invalid", "id is required");
      return tool === "delete_scheduled_task"
        ? { tool: "delete_scheduled_task", id }
        : { tool: "run_scheduled_task_now", id };
    }
    case "computer_windows":
    case "computer_snapshot":
    case "computer_screenshot":
    case "computer_click":
    case "computer_type":
    case "computer_key":
    case "computer_scroll": {
      const callId = s(params.callId)?.trim() || undefined;
      return {
        tool: "computer_desktop",
        desktopTool: tool,
        args: args as JsonObject,
        ...(callId ? { callId } : {}),
        ...machineField(args),
      };
    }
    default:
      return failure("invalid", "unknown tool");
  }
}

/**
 * `schedule_task`'s schedule, checked here the way the engine checks it: a
 * five-field cron for a repeating task, an ISO 8601 time with its offset for
 * one that runs once.
 */
export function parseSchedule(raw: Record<string, unknown>): TaskSchedule | string {
  const kind = typeof raw.kind === "string" ? raw.kind : "";
  if (kind === "cron") {
    const expression = typeof raw.expression === "string" ? raw.expression.trim() : "";
    if (expression.split(/\s+/).filter(Boolean).length !== 5) {
      return "schedule.expression must be a five-field cron expression";
    }
    const timezone = typeof raw.timezone === "string" ? raw.timezone.trim() : "";
    return { kind: "cron", expression, ...(timezone ? { timezone } : {}) };
  }
  if (kind === "at") {
    const at = typeof raw.at === "string" ? raw.at.trim() : "";
    if (!at || Number.isNaN(Date.parse(at))) {
      return "schedule.at must be an ISO 8601 time with its offset";
    }
    return { kind: "at", at };
  }
  return "schedule.kind must be cron or at";
}
