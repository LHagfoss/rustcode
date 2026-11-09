// ACP v1 shapes the app uses, and parsers from raw JSON. Ported from the
// SwiftUI app's RustCodeKit (ACPTypes.swift, FileDiff.swift, SessionUsage.swift).

import { arr, bool, type Json, type JsonObject, num, obj, rustcodeMeta, str } from "./jsonrpc";
import { parseStatus, parseWorktree, type ThreadStatus, type Worktree } from "./v2";

export const ACP_PROTOCOL_VERSION = 1;
/** Version of the `_meta.rustcode` question payload this client understands. */
export const QUESTION_META_VERSION = 2;

export const Method = {
  initialize: "initialize",
  sessionNew: "session/new",
  sessionLoad: "session/load",
  sessionList: "session/list",
  sessionClose: "session/close",
  sessionMove: "_rustcode/session/move",
  sessionRepair: "_rustcode/session/repair",
  sessionActivity: "_rustcode/session/activity",
  /** A thread the engine started because a paired device asked for one. */
  threadCreated: "_rustcode/threads/created",
  sessionPrompt: "session/prompt",
  sessionCancel: "session/cancel",
  sessionSetConfigOption: "session/set_config_option",
  sessionUpdate: "session/update",
  requestPermission: "session/request_permission",
  warm: "_rustcode/warm",
  steer: "_rustcode/steer",
  agents: "_rustcode/agents",
  rewind: "_rustcode/rewind",
  usageLimits: "_rustcode/usage/limits",
  usagePlans: "_rustcode/usage/plans",
  /** The slash-command catalog, for a client with no session to advertise it on. */
  commands: "_rustcode/commands",
  dataUsage: "_rustcode/data/usage",
  dataQuery: "_rustcode/data/query",
  dataActivity: "_rustcode/data/activity",
  dataLinksLoad: "_rustcode/data/links/load",
  dataLinksSave: "_rustcode/data/links/save",
  /** An orchestration call's receipt: checked before the app acts, written after it succeeds. */
  dataReceiptGet: "_rustcode/data/receipts/get",
  dataReceiptPut: "_rustcode/data/receipts/put",
  /** The two payload-capture switches (`engine/src/acp/telemetry.rs`). */
  telemetry: "_rustcode/config/telemetry",
  telemetrySet: "_rustcode/config/telemetry/set",
  pullState: "_rustcode/pulls/state",
  pullMerge: "_rustcode/pulls/merge",
  pullView: "_rustcode/pulls/view",
  pullDiff: "_rustcode/pulls/diff",
  pullReview: "_rustcode/pulls/review",
  harnesses: "_rustcode/harnesses",
  harnessModels: "_rustcode/harnesses/models",
  harnessSet: "_rustcode/harnesses/set",
  harnessRemove: "_rustcode/harnesses/remove",
  harnessDefault: "_rustcode/harnesses/default",
  harnessRemember: "_rustcode/harnesses/remember",
  harnessAccountAdd: "_rustcode/harnesses/accounts/add",
  harnessAccountRemove: "_rustcode/harnesses/accounts/remove",
  harnessAccountCheck: "_rustcode/harnesses/accounts/check",
  sessionHarness: "_rustcode/session/harness",
  onboardingComplete: "_rustcode/onboarding/complete",
  capabilities: "_rustcode/capabilities",
  capabilitiesSet: "_rustcode/capabilities/set",
  mcpAdd: "_rustcode/capabilities/mcp/add",
  mcpRemove: "_rustcode/capabilities/mcp/remove",
  mcpRestart: "_rustcode/capabilities/mcp/restart",
  connections: "_rustcode/connections",
  githubLogin: "_rustcode/connections/github/login",
  githubWait: "_rustcode/connections/github/wait",
  githubCancel: "_rustcode/connections/github/cancel",
  githubLogout: "_rustcode/connections/github/logout",
  accountLogout: "_rustcode/connections/account/logout",
  pulls: "_rustcode/pulls",
  /** The timed approvals still in force ("Allow for 1 hour"), and revoking one. */
  permissions: "_rustcode/permissions",
  permissionsRevoke: "_rustcode/permissions/revoke",
} as const;

// ---------------------------------------------------------------- initialize

export interface AgentInfo {
  protocolVersion: number;
  loadSession: boolean;
  listSessions: boolean;
  closeSession: boolean;
  promptImages: boolean;
  steer: boolean;
  agents: boolean;
  plan: boolean;
  rewind: boolean;
  pullRequests: boolean;
  /** `_rustcode/session/move`: a closed thread can change folder. */
  sessionMove: boolean;
  /** `_rustcode/session/repair`: Repair Thread for one that will not open. */
  sessionRepair: boolean;
  /**
   * `_rustcode/checkpoints/restore` with `keep`: a rewind keeps the state it abandons as a
   * checkpoint, so the attempt it leaves can be returned to.
   */
  checkpointAttempts: boolean;
}

export function parseAgentInfo(result: Json): AgentInfo {
  const root = obj(result);
  const version = num(root?.protocolVersion);
  if (version === undefined)
    throw new Error("The RustCode server sent an unexpected response: no protocolVersion");
  if (version !== ACP_PROTOCOL_VERSION) {
    throw new Error(
      `The RustCode server speaks ACP version ${version}; this app requires version ${ACP_PROTOCOL_VERSION}. Update RustCode so the app and server match.`,
    );
  }
  const caps = obj(root?.agentCapabilities);
  const meta = rustcodeMeta(caps);
  const sessionCaps = obj(caps?.sessionCapabilities);
  return {
    protocolVersion: version,
    loadSession: bool(caps?.loadSession) ?? false,
    listSessions: sessionCaps?.list !== undefined,
    closeSession: sessionCaps?.close !== undefined,
    promptImages: bool(obj(caps?.promptCapabilities)?.image) ?? false,
    steer: num(meta?.steer) !== undefined,
    agents: num(meta?.agents) !== undefined,
    plan: (num(meta?.agents) ?? 0) >= 2,
    rewind: num(meta?.rewind) !== undefined,
    pullRequests: num(meta?.pullRequests) !== undefined,
    sessionMove: num(meta?.sessionMove) !== undefined,
    sessionRepair: num(meta?.sessionRepair) !== undefined,
    checkpointAttempts: (num(meta?.checkpoints) ?? 0) >= 2,
  };
}

/**
 * The client's initialize request. Only what this app implements is
 * advertised: no client fs or terminal (the engine uses its own tools under
 * its own sandbox), Smart UI as the setting says, and no preview or
 * visualizations yet.
 */
export function initializeParams(
  appVersion: string,
  options: {
    smartUi?: boolean;
    orchestration?: number;
    visualizations?: number;
    presence?: number;
    /** 1 when the client lists threads the engine starts for a paired device. */
    threads?: number;
  } = {},
): JsonObject {
  const rustcode: JsonObject = { smartUi: options.smartUi ? 1 : 0 };
  // Discord Rich Presence (docs/discord-rich-presence.md): the engine publishes the project and
  // whether a turn runs, for clients that ask; the setting itself is the engine's config.
  if (options.presence) rustcode.presence = options.presence;
  // Threads that start and message other threads (docs/orchestrator.md): only while the setting is on.
  if (options.orchestration) rustcode.orchestration = options.orchestration;
  if (options.visualizations) rustcode.visualizations = options.visualizations;
  // Remote access: a thread a paired device starts is created in the engine and named here, so
  // only a client that shows thread rows this way may host them (docs/remote.md).
  if (options.threads) rustcode.threads = options.threads;
  return {
    protocolVersion: ACP_PROTOCOL_VERSION,
    clientCapabilities: {
      fs: { readTextFile: false, writeTextFile: false },
      terminal: false,
      // Smart UI blocks run in the transcript (docs/smart-ui.md); 0 while the setting is off,
      // so the agent can name the setting instead of writing a block that shows as source.
      _meta: { rustcode },
    },
    clientInfo: { name: "rustcode-desktop", title: "RustCode", version: appVersion },
  };
}

// ---------------------------------------------------------------- sessions

export interface SessionSummary {
  id: string;
  cwd: string;
  title?: string;
  updatedAt?: number;
  model?: string;
  harness?: string;
  /** The worktree the thread runs in (`_meta.rustcode.worktree`). */
  worktree?: Worktree;
  /**
   * The repository the thread's folder belongs to (`_meta.rustcode.repo`), so a
   * main checkout and its worktrees are one project for the orchestration tools.
   */
  repo?: string;
  /** The named account the thread runs on (`_meta.rustcode.account`). */
  account?: string;
  /** Engine-reported status (`_meta.rustcode.status`). */
  status?: ThreadStatus;
  /** Pinned in the engine (`_meta.rustcode.pinned`). */
  pinned?: boolean;
  /**
   * The thread an agent started this one from (`_meta.rustcode.parent`), kept
   * in the thread's own metadata: the link book prunes old links, this does
   * not. `id` is absent for a thread an older release started, where only the
   * parent's title was recovered from the first message. `relationship` says
   * why the parent exists: a record without one was always a delegate.
   */
  parent?: ThreadParent;
}

/** Why a thread has a parent (`_meta.rustcode.parent.relationship`). */
export type ThreadRelationship = "delegate" | "fork" | "side_chat" | "other";

/** The thread above another, as the engine recorded it in the thread's own metadata. */
export interface ThreadParent {
  id?: string;
  title?: string;
  /** Absent means a delegate: every record written before the field was one. */
  relationship?: ThreadRelationship;
}

/** The relationship a `_meta` name stands for; a record without one is a delegate. */
export function parseRelationship(value: unknown): ThreadRelationship {
  if (value === undefined || value === null || value === "") return "delegate";
  return value === "delegate" || value === "fork" || value === "side_chat" ? value : "other";
}

/** How a thread relates to its parent; a record without one is a delegate. */
export const relationshipOf = (parent: ThreadParent): ThreadRelationship => parent.relationship ?? "delegate";

/** The parent of a thread an agent handed work to: the only relationship that is orchestration. */
export const delegateParent = (parent?: ThreadParent): ThreadParent | undefined =>
  parent && relationshipOf(parent) === "delegate" ? parent : undefined;

export function parseSessionList(result: Json): { sessions: SessionSummary[]; nextCursor?: string } {
  const root = obj(result);
  const sessions: SessionSummary[] = [];
  for (const entry of arr(root?.sessions)) {
    const e = obj(entry);
    const id = str(e?.sessionId);
    if (!id) continue;
    const stamp = str(e?.updatedAt);
    const meta = obj(e?._meta);
    const rc = rustcodeMeta(e);
    const harness = rc?.harness;
    const parent = obj(rc?.parent);
    const parentId = str(parent?.sessionId);
    const parentTitle = str(parent?.title);
    const relationship = parseRelationship(parent?.relationship);
    sessions.push({
      worktree: parseWorktree(rc?.worktree),
      repo: str(rc?.repo),
      status: parseStatus(rc?.status),
      pinned: bool(rc?.pinned),
      account: str(rc?.account),
      parent: parentId || parentTitle ? { id: parentId, title: parentTitle, relationship } : undefined,
      id,
      cwd: str(e?.cwd) ?? "",
      title: str(e?.title),
      updatedAt: stamp ? Date.parse(stamp) || undefined : undefined,
      model: str(meta?.model),
      harness: str(harness) ?? str(obj(harness)?.id),
    });
  }
  return { sessions, nextCursor: str(root?.nextCursor) };
}

/** Latest activity first; ties keep their order. */
export function byActivity(sessions: SessionSummary[]): SessionSummary[] {
  return sessions
    .map((session, index) => ({ session, index }))
    .sort((a, b) => (b.session.updatedAt ?? 0) - (a.session.updatedAt ?? 0) || a.index - b.index)
    .map(({ session }) => session);
}

/**
 * A folder as people read it: the home folder as `~`, macOS's /private/tmp as /tmp, and Windows'
 * extended-length prefix (`\\?\C:\code\app`, `\\?\UNC\server\share`) as the plain path it wraps.
 * The prefix is a Win32 API detail, and it only makes a path harder to read in a tooltip.
 */
export function displayPath(path: string): string {
  return path
    .replace(/^\\\\\?\\UNC\\/, "\\\\")
    .replace(/^\\\\\?\\/, "")
    .replace(/^\/Users\/[^/]+/, "~")
    .replace(/^\/private(?=\/(?:tmp|var|etc)(?:\/|$))/, "");
}

/**
 * The last folder of a path, which names it in the app: the sidebar, the header, the composer and
 * the new-thread heading. Windows paths use `\` and often carry the extended-length prefix, so a
 * path is split on both separators; a drive or share root names itself (`C:`, `share`).
 */
export function projectName(cwd: string): string {
  // The path inside `\\?\C:\code\app` (or `\\?\UNC\server\share`), whose own `\` and `?` are
  // separators and prefix, not folders.
  const plain = cwd.replace(/^\\\\\?\\UNC\\/, "\\\\").replace(/^\\\\\?\\/, "");
  const parts = plain.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? cwd;
}

/** `provider/model` reads as just the model. */
export function modelName(model: string | undefined): string | undefined {
  return model?.split("/").pop();
}

// ---------------------------------------------------------------- config options

export interface ConfigChoice {
  value: string;
  name: string;
  description?: string;
}

export interface ConfigOption {
  id: string;
  name: string;
  type: "select" | "boolean";
  current?: string;
  currentBool?: boolean;
  choices: ConfigChoice[];
}

export function parseConfigOptions(value: Json | undefined): ConfigOption[] {
  const options: ConfigOption[] = [];
  for (const raw of arr(value)) {
    const o = obj(raw);
    const id = str(o?.id);
    if (!id) continue;
    const type = str(o?.type) === "boolean" ? "boolean" : "select";
    options.push({
      id,
      name: str(o?.name) ?? id,
      type,
      current: type === "select" ? str(o?.currentValue) : undefined,
      currentBool: type === "boolean" ? bool(o?.currentValue) : undefined,
      choices: arr(o?.options).flatMap((c) => {
        const choice = obj(c);
        const v = str(choice?.value);
        return v ? [{ value: v, name: str(choice?.name) ?? v, description: str(choice?.description) }] : [];
      }),
    });
  }
  return options;
}

/** How an effort level reads in a picker: `Off`, `xHigh`, else capitalised. */
export function effortLabel(effort: string): string {
  if (effort === "default") return "Off";
  if (effort === "xhigh") return "xHigh";
  return effort.charAt(0).toUpperCase() + effort.slice(1);
}

// ---------------------------------------------------------------- usage

/** A change of model or effort, as the transcript shows it. */
export interface ChoiceChange {
  modelFrom?: string;
  modelTo?: string;
  effortFrom?: string;
  effortTo?: string;
}

export interface SessionUsage {
  used: number;
  size: number;
  estimated: boolean;
  inputTokens?: number;
  outputTokens?: number;
  cachedTokens?: number;
  cost?: number;
  currency?: string;
  /** What fills the window, when the engine reports it (`_meta.rustcode.context`). */
  context?: ContextDetail;
}

export interface ContextItem {
  name: string;
  /** An estimate in the engine's token unit, when it knows one. */
  tokens?: number;
}

export interface ContextDetail {
  model?: string;
  /** In the engine's order, which matches the TUI legend; empty ones are kept. */
  categories: { name: string; tokens: number; items?: ContextItem[] }[];
  free?: number;
  compactions?: number;
  reclaimed?: number;
  /** Where the harness compacts on its own, when it reports a threshold. */
  compactAt?: number;
}

function parseContextDetail(value: unknown): ContextDetail | undefined {
  const o = obj(value);
  if (!o || !Array.isArray(o.categories)) return undefined;
  const categories = o.categories.flatMap((c) => {
    const x = obj(c);
    const name = str(x?.name);
    const tokens = num(x?.tokens);
    if (!name || tokens === undefined) return [];
    const items = Array.isArray(x?.items)
      ? x.items.flatMap((item) => {
          const y = obj(item);
          const itemName = str(y?.name);
          return itemName ? [{ name: itemName, tokens: num(y?.tokens) }] : [];
        })
      : undefined;
    return [{ name, tokens, ...(items?.length ? { items } : {}) }];
  });
  const compaction = obj(o.compaction);
  const compactAt = obj(o.compactAt);
  return {
    model: str(o.model),
    categories,
    free: num(o.free),
    compactions: num(compaction?.count),
    reclaimed: num(compaction?.reclaimedTokens),
    compactAt: num(compactAt?.tokens),
  };
}

export function parseUsage(update: JsonObject): SessionUsage | undefined {
  const used = num(update.used);
  const size = num(update.size);
  if (used === undefined || size === undefined) return undefined;
  const meta = rustcodeMeta(update);
  const cost = obj(update.cost);
  return {
    used,
    size,
    estimated: bool(meta?.estimated) ?? false,
    inputTokens: num(meta?.inputTokens),
    outputTokens: num(meta?.outputTokens),
    cachedTokens: num(meta?.cachedTokens),
    cost: num(cost?.amount),
    currency: str(cost?.currency),
    context: parseContextDetail(meta?.context),
  };
}

// ---------------------------------------------------------------- tool calls

export type ToolStatus = "pending" | "in_progress" | "completed" | "failed";

export function parseToolStatus(value: unknown): ToolStatus | undefined {
  return value === "pending" || value === "in_progress" || value === "completed" || value === "failed"
    ? value
    : undefined;
}

/** One ACP `diff` item: a file's text before and after an edit. */
export interface FileDiff {
  path: string;
  /** Undefined for a file the edit created. */
  oldText?: string;
  newText: string;
  oldStart: number;
  newStart: number;
}

export function parseDiffs(content: Json | undefined): FileDiff[] {
  return arr(content).flatMap((item) => {
    const o = obj(item);
    const path = str(o?.path);
    const newText = str(o?.newText);
    if (str(o?.type) !== "diff" || !path || newText === undefined) return [];
    const meta = rustcodeMeta(o);
    return [
      {
        path,
        oldText: str(o?.oldText),
        newText,
        oldStart: num(meta?.oldStart) ?? 1,
        newStart: num(meta?.newStart) ?? 1,
      },
    ];
  });
}

export interface TurnTiming {
  id?: string;
  elapsedMs?: number;
  outcome?: string;
  endedAt?: number;
}

export interface PlanEntry {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority?: string;
}

export interface SlashCommand {
  name: string;
  description: string;
  takesArguments: boolean;
  report: boolean;
  action: string;
}

/** One `session/update`, reduced to what the transcript renders. */
export type SessionUpdate =
  | { kind: "user"; text: string; at?: number }
  | { kind: "agent"; text: string; turn?: TurnTiming; at?: number }
  | { kind: "thought"; text: string; summary: boolean; durationMs?: number }
  | { kind: "notice"; text: string; resumable: boolean }
  /** The harness compacted the conversation (`_meta.rustcode.kind == "compaction"`). */
  | { kind: "compaction"; text: string }
  /** The thread moved to another harness here (`_meta.rustcode.kind == "handoff"`). */
  | { kind: "handoff"; from: string; to: string; model?: string }
  /** The thread's model or effort changed here (`_meta.rustcode.kind == "switch"`). */
  | { kind: "switch"; change: ChoiceChange }
  | {
      kind: "toolCall";
      id: string;
      title: string;
      toolKind?: string;
      status?: ToolStatus;
      input?: Json;
      detail?: string;
      summary?: string;
      diffs: FileDiff[];
      output?: string;
      /** The harness's own tool name (`_meta.claudeCode.toolName`), such as `Agent`. */
      toolName?: string;
      /** The Agent/Task call whose subagent made this call (`_meta.claudeCode.parentToolUseId`). */
      parent?: string;
    }
  | {
      kind: "toolCallUpdate";
      id: string;
      title?: string;
      status?: ToolStatus;
      output?: string;
      outputDelta?: string;
      diffs: FileDiff[];
      exitCode?: number;
      parent?: string;
      /** The call's work went on in the background (Claude Code's async task marker). */
      backgrounded?: boolean;
    }
  | { kind: "plan"; entries: PlanEntry[] }
  | { kind: "title"; title: string }
  | { kind: "configOptions"; options: ConfigOption[] }
  | { kind: "commands"; commands: SlashCommand[] }
  | { kind: "usage"; usage: SessionUsage }
  | { kind: "unknown"; type: string };

function contentText(content: Json | undefined): string {
  const c = obj(content);
  if (!c) return "";
  if (str(c.type) === "text") return str(c.text) ?? "";
  return str(obj(c.resource)?.text) ?? "";
}

function toolOutput(update: JsonObject): string | undefined {
  const raw = str(obj(update.rawOutput)?.content);
  if (raw !== undefined) return raw;
  const blocks = arr(update.content)
    .map((b) => contentText(obj(b)?.content))
    .filter((t) => t.length > 0);
  return blocks.length ? blocks.join("\n") : undefined;
}

function parseTurn(value: Json | undefined): TurnTiming | undefined {
  const t = obj(value);
  if (!t) return undefined;
  const ended = str(t.endedAt);
  return {
    id: str(t.id),
    elapsedMs: num(t.elapsedMs),
    outcome: str(t.outcome),
    endedAt: ended ? Date.parse(ended) : undefined,
  };
}

function parseDate(value: Json | undefined): number | undefined {
  const s = str(value);
  if (!s) return undefined;
  const t = Date.parse(s);
  return Number.isNaN(t) ? undefined : t;
}

/** Claude Code's adapter metadata on an update (`_meta.claudeCode`). */
function claudeMeta(update: JsonObject): JsonObject | undefined {
  return obj(obj(update._meta)?.claudeCode);
}

export function parseSessionUpdate(params: JsonObject): SessionUpdate {
  const update = obj(params.update) ?? {};
  const type = str(update.sessionUpdate) ?? "";
  const meta = rustcodeMeta(update);
  switch (type) {
    case "user_message_chunk":
      return { kind: "user", text: contentText(update.content), at: parseDate(meta?.at) };
    case "agent_message_chunk":
      if (str(meta?.kind) === "compaction") return { kind: "compaction", text: contentText(update.content) };
      if (str(meta?.kind) === "handoff") {
        return {
          kind: "handoff",
          from: str(meta?.from) ?? "",
          to: str(meta?.to) ?? "",
          model: str(meta?.model),
        };
      }
      if (str(meta?.kind) === "switch") {
        return {
          kind: "switch",
          change: {
            modelFrom: str(meta?.model_from),
            modelTo: str(meta?.model_to),
            effortFrom: str(meta?.effort_from),
            effortTo: str(meta?.effort_to),
          },
        };
      }
      if (str(meta?.kind) === "notice") {
        return {
          kind: "notice",
          text: contentText(update.content),
          resumable: bool(meta?.resumable) ?? false,
        };
      }
      return {
        kind: "agent",
        text: contentText(update.content),
        turn: parseTurn(meta?.turn),
        at: parseDate(meta?.at),
      };
    case "agent_thought_chunk":
      return {
        kind: "thought",
        text: contentText(update.content),
        summary: str(meta?.thoughtSource) === "summary",
        durationMs: num(meta?.thoughtMs),
      };
    case "tool_call":
      return {
        kind: "toolCall",
        id: str(update.toolCallId) ?? crypto.randomUUID(),
        title: str(update.title) ?? "Tool",
        toolKind: str(update.kind),
        status: parseToolStatus(update.status),
        input: update.rawInput,
        detail: str(meta?.detail),
        summary: str(meta?.summary) ?? str(obj(update.rawInput)?.summary),
        diffs: parseDiffs(update.content),
        output: toolOutput(update),
        toolName: str(claudeMeta(update)?.toolName),
        parent: str(claudeMeta(update)?.parentToolUseId),
      };
    case "tool_call_update":
      return {
        kind: "toolCallUpdate",
        id: str(update.toolCallId) ?? "",
        title: str(update.title),
        status: parseToolStatus(update.status),
        output: toolOutput(update),
        outputDelta: str(meta?.outputDelta),
        diffs: parseDiffs(update.content),
        exitCode: num(obj(update.rawOutput)?.exitCode),
        parent: str(claudeMeta(update)?.parentToolUseId),
        backgrounded:
          bool(obj(obj(obj(obj(update._meta)?.jetbrains)?.air)?.asyncTasks)?.backgrounded) || undefined,
      };
    case "plan":
      return {
        kind: "plan",
        entries: arr(update.entries).flatMap((e) => {
          const entry = obj(e);
          const content = str(entry?.content);
          if (!content) return [];
          const status = str(entry?.status);
          return [
            {
              content,
              status: status === "completed" || status === "in_progress" ? status : "pending",
              priority: str(entry?.priority),
            },
          ];
        }),
      };
    case "session_info_update": {
      const title = str(update.title);
      return title ? { kind: "title", title } : { kind: "unknown", type };
    }
    case "config_option_update":
      return { kind: "configOptions", options: parseConfigOptions(update.configOptions) };
    case "available_commands_update":
      return {
        kind: "commands",
        commands: arr(update.availableCommands).flatMap((c) => {
          const cmd = obj(c);
          const name = str(cmd?.name);
          if (!name) return [];
          const m = rustcodeMeta(cmd);
          const action = str(m?.action) ?? "execute";
          return [
            {
              name: name.startsWith("/") ? name : `/${name}`,
              description: str(cmd?.description) ?? "",
              takesArguments: obj(cmd?.input)?.hint !== undefined,
              report: action === "execute" && bool(m?.report) === true,
              action,
            },
          ];
        }),
      };
    case "usage_update": {
      const usage = parseUsage(update);
      return usage ? { kind: "usage", usage } : { kind: "unknown", type };
    }
    default:
      return { kind: "unknown", type };
  }
}

/** The update is a report command's answer, which can arrive beside a running turn. */
export function isReportUpdate(params: JsonObject): boolean {
  return rustcodeMeta(obj(params.update))?.report === true;
}

// ---------------------------------------------------------------- permissions

export type PermissionKind = "allow_once" | "allow_always" | "reject_once" | "reject_always";

export interface PermissionOption {
  id: string;
  name: string;
  kind?: PermissionKind;
}

export interface Question {
  header: string;
  text: string;
  descriptions: string[];
  multiple: boolean;
}

/** A `session/request_permission`: a tool approval, or an `ask_question`. */
export interface PermissionRequest {
  rpcId: number | string;
  sessionId: string;
  toolCallId: string;
  title: string;
  toolKind?: string;
  input?: Json;
  options: PermissionOption[];
  question?: Question;
  diffs: FileDiff[];
  /** When the app received it, so several waiting requests can read oldest first. */
  at?: number;
}

function parseKind(value: unknown): PermissionKind | undefined {
  return value === "allow_once" ||
    value === "allow_always" ||
    value === "reject_once" ||
    value === "reject_always"
    ? value
    : undefined;
}

export function parsePermissionRequest(rpcId: number | string, params: JsonObject): PermissionRequest | null {
  const sessionId = str(params.sessionId);
  const toolCall = obj(params.toolCall);
  if (!sessionId || !toolCall || !Array.isArray(params.options)) return null;
  const input = toolCall.rawInput;
  const options = arr(params.options).flatMap((o) => {
    const option = obj(o);
    const id = str(option?.optionId);
    return id ? [{ id, name: str(option?.name) ?? id, kind: parseKind(option?.kind) }] : [];
  });
  const meta = rustcodeMeta(params);
  const i = obj(input);
  const question =
    str(meta?.kind) === "question" && num(meta?.version) === QUESTION_META_VERSION
      ? {
          header: str(i?.header) ?? "",
          text: str(i?.question) ?? str(toolCall.title) ?? "",
          descriptions: arr(i?.descriptions).map((d) => str(d) ?? ""),
          multiple: bool(i?.multiple) ?? false,
        }
      : undefined;
  return {
    rpcId,
    sessionId,
    toolCallId: str(toolCall.toolCallId) ?? "",
    title: str(toolCall.title) ?? "Tool call",
    toolKind: str(toolCall.kind),
    input,
    options,
    question,
    diffs: parseDiffs(toolCall.content),
  };
}

export const isAllow = (kind?: PermissionKind) => kind === "allow_once" || kind === "allow_always";

export function permissionSelected(optionId: string): JsonObject {
  return { outcome: { outcome: "selected", optionId } };
}

export const permissionCancelled: JsonObject = { outcome: { outcome: "cancelled" } };

/** Answer a question with several options; ACP needs one selected option, the engine reads the rest from `_meta`. */
export function questionOptionsAnswer(options: PermissionOption[]): JsonObject {
  return {
    outcome: { outcome: "selected", optionId: options[0]?.id ?? "" },
    _meta: { rustcode: { answer: { options: options.map((o) => o.name) } } },
  };
}

export function questionTextAnswer(text: string, carrier: PermissionOption): JsonObject {
  return {
    outcome: { outcome: "selected", optionId: carrier.id },
    _meta: { rustcode: { answer: { text } } },
  };
}

// ---------------------------------------------------------------- harnesses (docs/harnesses.md)

/**
 * How a harness takes a message sent while a turn runs. The engine reports one record per harness,
 * and `state/queue.ts` (`sendDecision`) turns it and the turn's state into steer, queue or
 * interrupt-and-restart, so no client branches on the harness id.
 */
export interface HarnessSteering {
  /** A message goes into the running turn (`_rustcode/steer` accepts it). */
  live: boolean;
  /** A live steer carries the message's pictures and files, not only its text. */
  attachments: boolean;
  /**
   * A live steer goes in even while an approval or a question waits on the user. The built-in turn
   * reads its steers between steps only, so it waits instead.
   */
  while_awaiting_user: boolean;
  /** The turn can be stopped and the message sent as a new turn in its place. */
  interrupt_restart: boolean;
  /** A message that does not steer can wait for the next turn. */
  queue: boolean;
}

export interface HarnessCapabilities {
  /** `live` or `none`, for a client that predates `steering` (the same fact, without the detail). */
  steer: "live" | "next_turn" | "none";
  steering: HarnessSteering;
  cancel: boolean;
  resume: boolean;
  rewind: boolean;
  images: boolean;
  models: "dynamic" | "static" | "none";
  effort: boolean;
  permission_modes: string[];
  subagents: boolean;
  slash_commands: boolean;
  smart_ui: boolean;
  rustcode_extensions: boolean;
}

export interface HarnessModel {
  id: string;
  name: string;
  efforts: string[];
  default_effort?: string;
}

/** A named login of a harness: a config directory of its own (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`). */
export interface HarnessAccount {
  id: string;
  name: string;
  config_dir: string;
  /**
   * `ok` when its own credentials were found, `needs_login` when a look found
   * none, `unknown` when they were not looked for (only a Refresh probes the
   * macOS Keychain).
   */
  auth: "ok" | "needs_login" | "unknown";
  /** True for the Mac's own login (`~/.claude`, `~/.codex`), whatever path it was written as. */
  own_dir?: boolean;
}

/** A login directory found on this Mac that no account names yet. */
export interface DiscoveredAccount {
  config_dir: string;
  suggested_name: string;
}

export interface Harness {
  id: string;
  name: string;
  kind: "builtin" | "acp" | "codex";
  builtin: boolean;
  /**
   * One RustCode ships with (`rustcode`, Codex, Claude Code, …): turn it off,
   * never remove it. False for a harness the user added by hand.
   */
  known: boolean;
  enabled: boolean;
  command: string[];
  installed: boolean;
  version?: string;
  auth: "ok" | "needs_login" | "unknown";
  login_command?: string[];
  install_hint?: string;
  capabilities: HarnessCapabilities;
  models: HarnessModel[];
  /** The model and effort last chosen for a new thread on it (`[[harnesses]]` `model`/`effort`). */
  last?: HarnessLast | null;
  /** Named logins of this harness (`[[harness_accounts]]`); empty for most harnesses. */
  accounts?: HarnessAccount[];
  /**
   * The variable naming a named login's directory (`CLAUDE_CONFIG_DIR`,
   * `CODEX_HOME`), and the harness's own command that signs one in. Both are
   * null for a harness without named logins.
   */
  account_env?: string | null;
  account_login_command?: string[] | null;
  /** Login directories on this Mac the engine found; only a harness with named logins lists any. */
  discovered_accounts?: DiscoveredAccount[];
  error?: string | null;
}

export interface HarnessLast {
  model: string | null;
  effort: string | null;
}

export interface HarnessDefault {
  harness: string;
  model: string | null;
  effort: string | null;
}

export interface HarnessSnapshot {
  harnesses: Harness[];
  default: HarnessDefault;
  onboarded: boolean;
  /** Set by the client when the engine does not implement harnesses yet. */
  unsupported?: boolean;
  /** The engine remembers each harness's last choice (`_rustcode/harnesses/remember`); else the app does. */
  remembers?: boolean;
}

export const BUILTIN_CAPABILITIES: HarnessCapabilities = {
  steer: "live",
  // The built-in turn reads a steer as text, between steps, and only while nothing waits on the
  // user; anything more waits for the next turn.
  steering: {
    live: true,
    attachments: false,
    while_awaiting_user: false,
    interrupt_restart: true,
    queue: true,
  },
  cancel: true,
  resume: true,
  rewind: true,
  images: true,
  models: "dynamic",
  effort: true,
  permission_modes: ["default", "auto_edit", "full_access"],
  subagents: true,
  slash_commands: true,
  smart_ui: true,
  rustcode_extensions: true,
};

function parseCapabilities(value: Json | undefined, builtin = false): HarnessCapabilities {
  const c = obj(value) ?? {};
  const steer = str(c.steer);
  const models = str(c.models);
  const live = steer === "live" || steer === "next_turn";
  const images = bool(c.images) ?? false;
  // An engine from before `steering` reported the same facts apart: `steer` and `images`, with the
  // built-in harness taking a steer as text only.
  const steering = obj(c.steering);
  return {
    steer: live ? (steer as "live" | "next_turn") : "none",
    steering: {
      live: bool(steering?.live) ?? live,
      attachments: bool(steering?.attachments) ?? (live && images && !builtin),
      // An engine from before this record steered a Codex or an ACP turn whenever it ran.
      while_awaiting_user: bool(steering?.while_awaiting_user) ?? (live && !builtin),
      interrupt_restart: bool(steering?.interrupt_restart) ?? true,
      queue: bool(steering?.queue) ?? true,
    },
    cancel: bool(c.cancel) ?? false,
    resume: bool(c.resume) ?? false,
    rewind: bool(c.rewind) ?? false,
    images,
    models: models === "dynamic" || models === "static" ? models : "none",
    effort: bool(c.effort) ?? false,
    permission_modes: arr(c.permission_modes).flatMap((m) => (typeof m === "string" ? [m] : [])),
    subagents: bool(c.subagents) ?? false,
    slash_commands: bool(c.slash_commands) ?? false,
    smart_ui: bool(c.smart_ui) ?? false,
    rustcode_extensions: bool(c.rustcode_extensions) ?? false,
  };
}

export function parseHarnessModels(value: Json | undefined): HarnessModel[] {
  return arr(value).flatMap((m) => {
    const model = obj(m);
    const id = str(model?.id);
    if (!id) return [];
    return [
      {
        id,
        name: str(model?.name) ?? id,
        efforts: arr(model?.efforts).flatMap((e) => (typeof e === "string" ? [e] : [])),
        default_effort: str(model?.default_effort),
      },
    ];
  });
}

export function parseHarnessAccounts(value: Json | undefined): HarnessAccount[] {
  return arr(value).flatMap((entry) => {
    const o = obj(entry);
    const id = str(o?.id);
    if (!o || !id) return [];
    const auth = str(o.auth);
    return [
      {
        id,
        name: str(o.name) ?? id,
        config_dir: str(o.config_dir) ?? "",
        auth: auth === "ok" ? "ok" : auth === "needs_login" ? "needs_login" : "unknown",
        own_dir: bool(o.own_dir) ?? false,
      },
    ];
  });
}

export function parseDiscoveredAccounts(value: Json | undefined): DiscoveredAccount[] {
  return arr(value).flatMap((entry) => {
    const o = obj(entry);
    const config_dir = str(o?.config_dir);
    if (!o || !config_dir) return [];
    return [{ config_dir, suggested_name: str(o.suggested_name) ?? config_dir }];
  });
}

export function parseHarnessLast(value: Json | undefined): HarnessLast | null {
  const last = obj(value);
  if (!last) return null;
  const model = str(last.model) ?? null;
  const effort = str(last.effort) ?? null;
  return model || effort ? { model, effort } : null;
}

/**
 * The standard harnesses of an engine that predates the snapshot's `known`
 * (before issue 2881). Current engines say which are standard, so this only
 * keeps an older engine's Codex from offering "Remove", which it refuses.
 */
const LEGACY_KNOWN_HARNESSES = ["codex", "claude-code", "gemini", "opencode", "pi"];

export function parseHarnessSnapshot(value: Json): HarnessSnapshot {
  const root = obj(value) ?? {};
  let remembers = false;
  const harnesses = arr(root.harnesses).flatMap((h): Harness[] => {
    const o = obj(h);
    const id = str(o?.id);
    if (!o || !id) return [];
    const kind = str(o.kind);
    const auth = str(o.auth);
    const builtin = bool(o.builtin) ?? false;
    // An engine that remembers choices sends `last` on every row, null when there is none.
    if ("last" in o) remembers = true;
    return [
      {
        id,
        name: str(o.name) ?? id,
        kind: kind === "builtin" || kind === "codex" ? kind : "acp",
        builtin,
        // The engine says which harnesses are standard; an older one did not,
        // so the ids it shipped with stand in until it does.
        known: bool(o.known) ?? (builtin || LEGACY_KNOWN_HARNESSES.includes(id)),
        enabled: bool(o.enabled) ?? false,
        command: arr(o.command).flatMap((c) => (typeof c === "string" ? [c] : [])),
        installed: bool(o.installed) ?? false,
        version: str(o.version),
        auth: auth === "ok" || auth === "needs_login" ? auth : "unknown",
        login_command: Array.isArray(o.login_command)
          ? arr(o.login_command).flatMap((c) => (typeof c === "string" ? [c] : []))
          : undefined,
        install_hint: str(o.install_hint),
        capabilities: parseCapabilities(o.capabilities, builtin),
        models: parseHarnessModels(o.models),
        last: parseHarnessLast(o.last),
        accounts: parseHarnessAccounts(o.accounts),
        account_env: str(o.account_env) ?? null,
        account_login_command: Array.isArray(o.account_login_command)
          ? arr(o.account_login_command).flatMap((c) => (typeof c === "string" ? [c] : []))
          : null,
        discovered_accounts: parseDiscoveredAccounts(o.discovered_accounts),
        error: str(o.error) ?? null,
      },
    ];
  });
  const d = obj(root.default);
  return {
    harnesses,
    default: {
      harness: str(d?.harness) ?? "rustcode",
      model: str(d?.model) ?? null,
      effort: str(d?.effort) ?? null,
    },
    onboarded: bool(root.onboarded) ?? false,
    remembers,
  };
}

/** What a client shows before the engine implements harnesses: the built-in harness only. */
export function fallbackHarnessSnapshot(): HarnessSnapshot {
  return {
    harnesses: [
      {
        id: "rustcode",
        name: "RustCode",
        kind: "builtin",
        builtin: true,
        known: true,
        enabled: true,
        command: [],
        installed: true,
        auth: "ok",
        capabilities: BUILTIN_CAPABILITIES,
        models: [],
        accounts: [],
        error: null,
      },
    ],
    default: { harness: "rustcode", model: null, effort: null },
    onboarded: true,
    unsupported: true,
  };
}

/** The harness a `session/new` response says the thread runs on. */
export interface SessionHarness {
  id: string;
  capabilities: HarnessCapabilities;
  /** The named account the thread runs on, when it is not the default login. */
  account?: string;
}

export function parseSessionHarness(result: Json): SessionHarness | undefined {
  const meta = rustcodeMeta(result);
  const h = obj(meta?.harness);
  const id = str(h?.id);
  if (!id) return undefined;
  const account = str(obj(meta?.account)?.id);
  return {
    id,
    capabilities: parseCapabilities(h?.capabilities, id === "rustcode"),
    ...(account ? { account } : {}),
  };
}

// ---------------------------------------------------------------- remote access

// The remote shapes and their parser live in `remote.ts`, next to the methods
// the page calls; they are re-exported here so every ACP shape stays reachable
// from this module.
export {
  heldBackCount,
  heldBackReason,
  parseRemoteStatus,
  type RemoteDevice,
  type RemoteGateway,
  RemoteMethod,
  type RemoteOffer,
  type RemotePairing,
  type RemoteStatus,
} from "./remote";
