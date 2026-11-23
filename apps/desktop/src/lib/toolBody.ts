// What a tool call's expander shows, by what the call did (docs/desktop.md › Tool calls):
// a command with its output and exit code, a search or read with what it looked for and found,
// and any other call (MCP tools included) with its arguments as key/value rows. A call with
// diffs keeps its diff view. Pure, so the row and the tests share it.

import type { Json } from "../acp/jsonrpc";
import { unwrapToolError } from "../state/toolNames";
import { type AnsiLine, ansiPlainText, hasTerminalCodes, parseAnsi } from "./ansi";
import { shortenHome } from "./paths";
import { stripAnsi } from "./toolOutput";

/** One argument as a row: `inline` on the key's line, `text` a wrapped block, `json` its own small block. */
export interface ArgRow {
  key: string;
  /** What the row shows, and what its Copy copies: a string as it is, anything else as JSON. */
  value: string;
  shape: "inline" | "text" | "json";
}

export type ToolBody =
  | { kind: "diff" }
  | {
      kind: "command";
      command?: string;
      /** Plain text: what Copy copies. */
      output?: string;
      /** The same output with its colours, when the command printed any escape codes. */
      styled?: AnsiLine[];
      exitCode?: number;
      args: ArgRow[];
    }
  | { kind: "lookup"; args: ArgRow[]; output?: string; outputLabel: string }
  | { kind: "args"; args: ArgRow[]; output?: string };

/** A string this long, or with a line break, wraps under its key instead of beside it. */
const INLINE_MAX = 80;

/** Keys the row already says: the harness's one-line summary of the call. */
const SUMMARY_KEYS = new Set(["summary", "toolSummary"]);
const COMMAND_KEYS = ["command", "cmd", "CommandLine"];
const EXECUTE_INTERNAL_KEYS = ["WaitMsBeforeAsync"];

function isRecord(input: Json | undefined): input is { [key: string]: Json } {
  return !!input && typeof input === "object" && !Array.isArray(input);
}

/** One value as a row. */
export function argRow(key: string, value: Json): ArgRow {
  if (value !== null && typeof value === "object") {
    return { key, value: JSON.stringify(value, null, 2), shape: "json" };
  }
  const text = typeof value === "string" ? value : String(value);
  const long = typeof value === "string" && (text.length > INLINE_MAX || text.includes("\n"));
  return { key, value: text, shape: long ? "text" : "inline" };
}

/**
 * A call's arguments as rows, in the order the harness sent them. Not an object (a bare string or a
 * list) is one row named `input`; nothing to show is no rows.
 */
export function argRows(input: Json | undefined, skip: ReadonlySet<string> = SUMMARY_KEYS): ArgRow[] {
  if (input === undefined || input === null) return [];
  if (!isRecord(input)) return [argRow("input", input)];
  return Object.entries(input)
    .filter(([key, value]) => !skip.has(key) && value !== undefined)
    .map(([key, value]) => argRow(key, value));
}

/** The command line a shell call ran: a string, or argv joined the way a shell reads it. */
export function commandOf(input: Json | undefined): string | undefined {
  if (!isRecord(input)) return undefined;
  for (const key of COMMAND_KEYS) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value;
    if (Array.isArray(value) && value.length && value.every((part) => typeof part === "string"))
      return value
        .map((part) => (/^[\w@%+=:,./-]+$/.test(part as string) ? part : quote(part as string)))
        .join(" ");
  }
  return undefined;
}

function quote(part: string): string {
  return `'${part.replaceAll("'", "'\\''")}'`;
}

function outputOf(output: string | undefined): string | undefined {
  if (output === undefined) return undefined;
  const text = stripAnsi(unwrapToolError(output) || output);
  return text.trim() ? text : undefined;
}

/** What the expander of a call of `kind` (`state/fold.ts` inferToolKind) shows. */
export function toolBody(
  item: { input?: Json; output?: string; detail?: string; exitCode?: number; diffs: readonly unknown[] },
  kind: string,
): ToolBody {
  if (item.diffs.length) return { kind: "diff" };
  const output = outputOf(item.output);
  if (kind === "execute") {
    const command = commandOf(item.input) ?? item.detail;
    const skip = new Set([...SUMMARY_KEYS, ...COMMAND_KEYS, "description", ...EXECUTE_INTERNAL_KEYS]);
    if (isRecord(item.input) && item.input.IsDaemon === false) skip.add("IsDaemon");
    const args = argRows(item.input, skip);
    const raw = item.output === undefined ? "" : unwrapToolError(item.output) || item.output;
    if (hasTerminalCodes(raw)) {
      // A terminal's output: its colours drawn, and `\r` progress kept to what was written last.
      const styled = parseAnsi(raw);
      const plain = ansiPlainText(styled);
      if (!plain.trim()) return { kind: "command", command, exitCode: item.exitCode, args };
      const coloured = styled.some((line) => line.some((span) => Object.keys(span.style).length > 0));
      return {
        kind: "command",
        command,
        output: plain,
        styled: coloured ? styled : undefined,
        exitCode: item.exitCode,
        args,
      };
    }
    return { kind: "command", command, output, exitCode: item.exitCode, args };
  }
  if (kind === "search" || kind === "read") {
    return {
      kind: "lookup",
      args: kind === "read" ? readArgRows(item.input) : argRows(item.input),
      output,
      outputLabel: kind === "search" ? "Results" : "Content",
    };
  }
  return { kind: "args", args: argRows(item.input), output };
}

/** Normalizes Antigravity's PascalCase read arguments into concise lowercase rows. */
function readArgRows(input: Json | undefined): ArgRow[] {
  if (!isRecord(input) || typeof input.Path !== "string") return argRows(input);
  const rows: ArgRow[] = [argRow("path", shortenHome(input.Path))];
  const start = typeof input.StartLine === "number" ? input.StartLine : undefined;
  const end = typeof input.EndLine === "number" ? input.EndLine : undefined;
  if (start !== undefined && end !== undefined) rows.push(argRow("lines", `${start}–${end}`));
  else if (start !== undefined) rows.push(argRow("lines", `${start}+`));
  else if (end !== undefined) rows.push(argRow("lines", `1–${end}`));
  if (typeof input.ByteOffset === "number" && input.ByteOffset > 0) {
    rows.push(argRow("offset", input.ByteOffset));
  }
  for (const [key, value] of Object.entries(input)) {
    if (
      key === "Path" ||
      key === "StartLine" ||
      key === "EndLine" ||
      key === "ByteOffset" ||
      SUMMARY_KEYS.has(key) ||
      value === undefined
    ) {
      continue;
    }
    rows.push(argRow(key, value));
  }
  return rows;
}
