// What a tool row says: MCP tools come named `mcp__server__tool` (or
// `server.tool`), which is neither readable nor sortable for the reader. This
// turns one into a row title, its server, and the line that tells two calls of
// the same tool apart. Pure, so the rows, the live label and the tests share
// it.

import type { Json } from "../acp/jsonrpc";
import type { ToolStatus } from "../acp/types";
import { clip } from "../lib/clip";

/** The tools RustCode's own MCP server offers to a thread orchestrating others. */
export const ORCHESTRATION_TOOLS = [
  "start_session",
  "message_session",
  "read_session",
  "wait_session",
  "list_sessions",
  "fan_out",
  "list_harnesses",
  "search_sessions",
  "list_worktrees",
  "update_session",
  "interrupt_session",
  "configure_session",
  "transfer_session",
  "list_pending_requests",
  "answer_request",
  "organize_threads",
  "list_projects",
  "list_pull_requests",
  "manage_queue",
  "thread_status",
  "merge_when_green",
  "cleanup_worktree",
  "worktree_status",
  "handoff_worktree",
] as const;

/** `mcp__server__tool`, `server__tool`, `server.tool`, `server/tool`, `server:tool`. */
const MCP_PREFIX = /^mcp__/;
const IDENT = /^[A-Za-z][A-Za-z0-9_-]*$/;
/**
 * The servers whose dotted (or slashed, or colon-separated) spelling is a tool call.
 * Only RustCode's own is known here: the other MCP servers come from the
 * engine at runtime, and guessing turned titles like `bin/run_tests` into MCP
 * calls.
 */
const KNOWN_SERVERS = new Set(["rustcode"]);

/** An object input, for the tools whose title comes from their arguments. */
const objectOf = (input: Json | undefined): Record<string, Json> =>
  input && typeof input === "object" && !Array.isArray(input) ? input : {};

const text = (value: Json | undefined) => (typeof value === "string" ? value.trim() : "");

/**
 * The server and tool a call's name holds, when it names one: `mcp__mail__send`
 * and `rustcode.start_session` both give `{ server, tool }`. A plain tool name
 * stays whole (`bash`), and so does a title that only looks like a server and a
 * tool (`bin/run_tests`, `foo.barBaz`): those are not MCP calls.
 */
export function splitToolName(name: string): { server?: string; tool: string } {
  const trimmed = name.trim();
  // The `mcp__` prefix is the MCP convention itself, so it always names a server.
  if (MCP_PREFIX.test(trimmed)) {
    const rest = trimmed.replace(MCP_PREFIX, "");
    const at = rest.indexOf("__");
    if (at > 0 && IDENT.test(rest.slice(0, at)) && IDENT.test(rest.slice(at + 2))) {
      return { server: rest.slice(0, at), tool: rest.slice(at + 2) };
    }
    if (IDENT.test(rest)) return { server: rest, tool: rest };
    return { tool: trimmed };
  }
  // The other spellings are only a call when the server is one we know.
  const at = trimmed.search(/[./:]/);
  if (at > 0) {
    const server = trimmed.slice(0, at);
    const tool = trimmed.slice(at + 1);
    if (KNOWN_SERVERS.has(server) && IDENT.test(tool)) return { server, tool };
  }
  const sep = trimmed.indexOf("__");
  if (sep > 0) {
    const server = trimmed.slice(0, sep);
    const tool = trimmed.slice(sep + 2);
    if (KNOWN_SERVERS.has(server) && IDENT.test(tool)) return { server, tool };
  }
  return { tool: trimmed };
}

/**
 * The RustCode orchestration tool a call names, when it names one: a bare
 * `start_session`, or one from RustCode's own server. Another MCP server's
 * `start_session` is its own tool, not ours.
 */
export function orchestrationTool(name: string): string | undefined {
  const { server, tool } = splitToolName(name);
  if (server && server !== "rustcode") return undefined;
  return (ORCHESTRATION_TOOLS as readonly string[]).includes(tool) ? tool : undefined;
}

/** `start_session` reads `Start session`: underscores become spaces, the first letter goes up. */
export const humanizeTool = (tool: string) =>
  tool
    .replace(/[_-]+/g, " ")
    .trim()
    .replace(/^./, (c) => c.toUpperCase());

/** A summary that says something: a lone fence marker or blank space is not a summary. */
export function usefulSummary(summary: string | undefined): string | undefined {
  const text = summary?.replace(/[`~\s]+/g, "").trim() ?? "";
  return text ? summary?.trim() : undefined;
}

/** A summary at most `max` long, its "…" never inside an emoji or an accented letter. */
const cut = (text: string, max: number) => (text.length > max ? `${clip(text, max - 1).trimEnd()}…` : text);

/** The newest call is still running, so a title may speak in the present tense. */
const running = (status: ToolStatus | undefined) => status === "pending" || status === "in_progress";

export interface ToolLabel {
  /** The row's title: `Read session`, `Started thread`. */
  title: string;
  /** The MCP server it comes from (`rustcode`, `mail`); the call is an MCP one when set. */
  source?: string;
  /** The line that tells two calls of one tool apart: a thread's title, a prompt's first line. */
  summary?: string;
}

/**
 * A call's row label. MCP names lose their server prefix and gain a readable
 * title; RustCode's orchestration tools say what they did (and to what), since
 * `mcp__rustcode__start_session` tells the reader nothing. Anything else keeps
 * its title as it came.
 */
export function displayTool(name: string, input?: Json, status?: ToolStatus): ToolLabel {
  const { server, tool } = splitToolName(name);
  const args = objectOf(input);
  const called = orchestrationTool(name);
  if (called) {
    const special = orchestrationLabel(called, args, status);
    if (special) return { ...special, source: server ?? "rustcode" };
    if (server) return { title: humanizeTool(tool), source: server };
  }
  if (!server) return { title: name.trim(), summary: usefulSummary(text(args.summary) || undefined) };
  return {
    title: humanizeTool(tool),
    source: server,
    summary: usefulSummary(text(args.summary) || text(args.description) || undefined),
  };
}

/** RustCode's orchestration tools, with what they did. */
function orchestrationLabel(
  tool: string,
  args: Record<string, Json>,
  status: ToolStatus | undefined,
): { title: string; summary?: string } | undefined {
  const links = Array.isArray(args.link_ids) ? args.link_ids.length : 0;
  switch (tool) {
    case "start_session": {
      const prompt = text(args.prompt).split("\n")[0] ?? "";
      const named = text(args.title) || prompt;
      // What it was told to run on, in the order the approval card names them.
      const harness = [text(args.harness), text(args.account), text(args.model)].filter(Boolean).join(" ");
      const summary = [named ? cut(named, 60) : "", harness].filter(Boolean).join(" · ");
      return { title: "Started thread", summary: summary || undefined };
    }
    case "message_session": {
      const files = Array.isArray(args.attachments) ? args.attachments.length : 0;
      return {
        title: "Messaged thread",
        summary: files ? `${files} file${files === 1 ? "" : "s"}` : undefined,
      };
    }
    case "manage_queue": {
      const what = text(args.action);
      const title =
        what === "list"
          ? "Listed queued messages"
          : what === "edit"
            ? "Changed a queued message"
            : what === "cancel"
              ? "Dropped a queued message"
              : what === "steer"
                ? "Sent a queued message now"
                : "Queued messages";
      return { title };
    }
    case "wait_session": {
      const n = links || 1;
      return { title: `${running(status) ? "Waiting" : "Waited"} for ${n} thread${n === 1 ? "" : "s"}` };
    }
    case "read_session":
      return { title: "Read thread" };
    case "list_sessions":
      return { title: "Listed threads" };
    case "list_harnesses":
      return { title: "Listed harnesses" };
    case "search_sessions":
      return { title: "Searched threads", summary: text(args.query) || undefined };
    case "list_worktrees":
      return { title: "Listed worktrees" };
    case "transfer_session":
      return { title: "Moved thread", summary: text(args.harness) || undefined };
    case "list_pending_requests":
      return { title: "Listed waiting requests" };
    case "answer_request":
      return { title: "Answered a request", summary: text(args.choice) || text(args.answer) || undefined };
    case "organize_threads": {
      const ids = Array.isArray(args.session_ids) ? args.session_ids.length : 0;
      return {
        title: `Organized ${ids || 1} thread${ids === 1 ? "" : "s"}`,
        summary: text(args.title) || undefined,
      };
    }
    case "list_projects":
      return { title: "Listed projects" };
    case "list_pull_requests": {
      const state = text(args.state) || "open";
      return { title: "Listed pull requests", summary: state };
    }
    case "thread_status":
      return { title: "Checked delegate pull requests" };
    case "merge_when_green":
      return { title: running(status) ? "Merging pull request" : "Merged pull request" };
    case "cleanup_worktree":
      return { title: "Cleaned up a worktree" };
    case "worktree_status":
      return { title: "Checked worktrees" };
    case "handoff_worktree":
      return text(args.to) === "worktree"
        ? { title: "Moved thread to a worktree" }
        : { title: "Moved work to the checkout" };
    case "update_session":
      return { title: "Updated thread", summary: text(args.title) || text(args.status) || undefined };
    case "interrupt_session":
      return { title: "Stopped thread" };
    case "configure_session": {
      const change = [text(args.model), text(args.effort)].filter(Boolean).join(" · ");
      return change ? { title: "Changed thread model", summary: change } : { title: "Read thread settings" };
    }
    case "fan_out": {
      const n = Array.isArray(args.harnesses) ? args.harnesses.length : 0;
      return { title: `Fanned out to ${n || 2} harnesses` };
    }
    default:
      return undefined;
  }
}

/** The `session_id` a call answered with, for the row that opens the thread it started. */
export function sessionIdOf(output: string | undefined): string | undefined {
  if (!output) return undefined;
  const match = /"session_id"\s*:\s*"([^"]+)"/.exec(output);
  return match?.[1];
}
