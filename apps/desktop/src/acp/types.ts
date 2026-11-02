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
  pullView: "_rustcode/pulls/view",
  pullDiff: "_rustcode/pulls/diff",
  pullReview: "_rustcode/pulls/review",
  harnesses: "_rustcode/harnesses",
  harnessModels: "_rustcode/harnesses/models",
  harnessSet: "_rustcode/harnesses/set",
  harnessRemove: "_rustcode/harnesses/remove",
  harnessDefault: "_rustcode/harnesses/default",
  onboardingComplete: "_rustcode/onboarding/complete",
  capabilities: "_rustcode/capabilities",
  capabilitiesSet: "_rustcode/capabilities/set",
  mcpAdd: "_rustcode/capabilities/mcp/add",
  mcpRemove: "_rustcode/capabilities/mcp/remove",
  connections: "_rustcode/connections",
  githubLogin: "_rustcode/connections/github/login",
  githubWait: "_rustcode/connections/github/wait",
  githubCancel: "_rustcode/connections/github/cancel",
  githubLogout: "_rustcode/connections/github/logout",
  accountLogout: "_rustcode/connections/account/logout",
  pulls: "_rustcode/pulls",
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
}

export function parseAgentInfo(result: Json): AgentInfo {
  const root = obj(result);
  const version = num(root?.protocolVersion);
  if (version === undefined) throw new Error("The engine sent an unexpected response: no protocolVersion");
  if (version !== ACP_PROTOCOL_VERSION) {
    throw new Error(
      `The engine speaks ACP version ${version}; this app requires version ${ACP_PROTOCOL_VERSION}. Update RustCode so the app and engine match.`,
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
  options: { smartUi?: boolean; orchestration?: number; visualizations?: number } = {},
): JsonObject {
  const rustcode: JsonObject = { smartUi: options.smartUi ? 1 : 0 };
  // Threads that start and message other threads (docs/orchestrator.md): only while the setting is on.
  if (options.orchestration) rustcode.orchestration = options.orchestration;
  if (options.visualizations) rustcode.visualizations = options.visualizations;
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
  /** Engine-reported status (`_meta.rustcode.status`). */
  status?: ThreadStatus;
  /** Pinned in the engine (`_meta.rustcode.pinned`). */
  pinned?: boolean;
}

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
    sessions.push({
      worktree: parseWorktree(rc?.worktree),
      status: parseStatus(rc?.status),
      pinned: bool(rc?.pinned),
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

export function projectName(cwd: string): string {
  const parts = cwd.split("/").filter(Boolean);
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

export interface SessionUsage {
  used: number;
  size: number;
  estimated: boolean;
  inputTokens?: number;
  outputTokens?: number;
  cachedTokens?: number;
  cost?: number;
  currency?: string;
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

export function parseSessionUpdate(params: JsonObject): SessionUpdate {
  const update = obj(params.update) ?? {};
  const type = str(update.sessionUpdate) ?? "";
  const meta = rustcodeMeta(update);
  switch (type) {
    case "user_message_chunk":
      return { kind: "user", text: contentText(update.content), at: parseDate(meta?.at) };
    case "agent_message_chunk":
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

export interface HarnessCapabilities {
  steer: "live" | "next_turn" | "none";
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

export interface Harness {
  id: string;
  name: string;
  kind: "builtin" | "acp" | "codex";
  builtin: boolean;
  enabled: boolean;
  command: string[];
  installed: boolean;
  version?: string;
  auth: "ok" | "needs_login" | "unknown";
  login_command?: string[];
  install_hint?: string;
  capabilities: HarnessCapabilities;
  models: HarnessModel[];
  error?: string | null;
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
}

export const BUILTIN_CAPABILITIES: HarnessCapabilities = {
  steer: "live",
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

function parseCapabilities(value: Json | undefined): HarnessCapabilities {
  const c = obj(value) ?? {};
  const steer = str(c.steer);
  const models = str(c.models);
  return {
    steer: steer === "live" || steer === "next_turn" ? steer : "none",
    cancel: bool(c.cancel) ?? false,
    resume: bool(c.resume) ?? false,
    rewind: bool(c.rewind) ?? false,
    images: bool(c.images) ?? false,
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

export function parseHarnessSnapshot(value: Json): HarnessSnapshot {
  const root = obj(value) ?? {};
  const harnesses = arr(root.harnesses).flatMap((h): Harness[] => {
    const o = obj(h);
    const id = str(o?.id);
    if (!o || !id) return [];
    const kind = str(o.kind);
    const auth = str(o.auth);
    return [
      {
        id,
        name: str(o.name) ?? id,
        kind: kind === "builtin" || kind === "codex" ? kind : "acp",
        builtin: bool(o.builtin) ?? false,
        enabled: bool(o.enabled) ?? false,
        command: arr(o.command).flatMap((c) => (typeof c === "string" ? [c] : [])),
        installed: bool(o.installed) ?? false,
        version: str(o.version),
        auth: auth === "ok" || auth === "needs_login" ? auth : "unknown",
        login_command: Array.isArray(o.login_command)
          ? arr(o.login_command).flatMap((c) => (typeof c === "string" ? [c] : []))
          : undefined,
        install_hint: str(o.install_hint),
        capabilities: parseCapabilities(o.capabilities),
        models: parseHarnessModels(o.models),
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
        enabled: true,
        command: [],
        installed: true,
        auth: "ok",
        capabilities: BUILTIN_CAPABILITIES,
        models: [],
        error: null,
      },
    ],
    default: { harness: "rustcode", model: null, effort: null },
    onboarded: true,
    unsupported: true,
  };
}

/** The harness a `session/new` response says the thread runs on. */
export function parseSessionHarness(
  result: Json,
): { id: string; capabilities: HarnessCapabilities } | undefined {
  const h = obj(rustcodeMeta(result)?.harness);
  const id = str(h?.id);
  return id ? { id, capabilities: parseCapabilities(h?.capabilities) } : undefined;
}
