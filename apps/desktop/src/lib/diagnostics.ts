// Settings › Diagnostics, the parts with no React and no store: what the shell
// answers about the RustCode server and the machine (src-tauri/src/diagnostics.rs),
// reading a server log line's level, redacting text for a public issue, and the
// Markdown debug report. Pure, so the report and its redaction are unit-tested.

/** How the last server process ended (`engine.rs` › `LastExit`). */
export interface LastExit {
  code: number | null;
  signal: number | null;
  /** Milliseconds since the epoch. */
  atMs: number;
  reason: string;
  /** The app asked for it (a restart, quitting); otherwise it crashed or did not start. */
  expected: boolean;
}

/** `server_status`: the server process as the shell runs it. */
export interface ServerInfo {
  running: boolean;
  pid?: number | null;
  path?: string | null;
  startedAtMs?: number | null;
  /** Times the shell started the server again after it crashed. */
  crashRestarts: number;
  lastExit?: LastExit | null;
  version?: string | null;
  configDir?: string | null;
  /** The server's own debug.log. */
  logFile?: string | null;
}

/** `system_info`: the machine and the web view. */
export interface SystemInfo {
  os: string;
  osVersion?: string | null;
  kernel?: string | null;
  arch: string;
  appVersion: string;
  tauriVersion?: string | null;
  webview: string;
  webviewVersion?: string | null;
  home?: string | null;
  graphicsEnv: string[];
}

/** What the page itself can see of how it is drawn. */
export interface PageInfo {
  devicePixelRatio: number;
  /** `<html data-platform>`, when the shell sets one. */
  platform?: string | null;
  /** The GPU WebGL reports (`WEBGL_debug_renderer_info`), when it does. */
  gpu?: string | null;
  viewport: string;
  userAgent: string;
}

/** One harness, as Agents knows it plus where this app's PATH finds its binary. */
export interface HarnessDiagnostics {
  id: string;
  name: string;
  enabled: boolean;
  installed: boolean;
  command: string[];
  path?: string | null;
  version?: string | null;
  auth: string;
  error?: string | null;
}

/** A row of `operational_events` (docs/data-store.md). */
export interface OperationalEvent {
  at: string;
  eventType: string;
  threadId?: string | null;
  fields?: string | null;
}

export type ServerState = "running" | "starting" | "crashed" | "stopped";

export const SERVER_STATE_LABEL: Record<ServerState, string> = {
  running: "Running",
  starting: "Starting…",
  crashed: "Crashed",
  stopped: "Stopped",
};

/**
 * The server's state from the app's connection and, when the shell answers,
 * its process: a process that ended without being asked to has crashed.
 */
export function serverState(
  connection: "connecting" | "ready" | "closed",
  info?: ServerInfo | null,
): ServerState {
  if (info) {
    if (info.running) return connection === "ready" ? "running" : "starting";
    if (info.lastExit && !info.lastExit.expected) return "crashed";
    return connection === "connecting" ? "starting" : "stopped";
  }
  if (connection === "ready") return "running";
  return connection === "connecting" ? "starting" : "stopped";
}

/** `3h 4m`, `4m 12s`, `12s`. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const d = Math.floor(total / 86_400);
  const h = Math.floor((total % 86_400) / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  if (m) return `${m}m ${s}s`;
  return `${s}s`;
}

/** How a process ended, in a few words. */
export function describeExit(exit: LastExit): string {
  const how =
    exit.code !== null ? `code ${exit.code}` : exit.signal !== null ? `signal ${exit.signal}` : "stopped";
  return exit.expected ? `${how} (asked to stop)` : how;
}

// ---------------------------------------------------------------- log levels

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

/** Most to least severe; a filter keeps its level and everything before it. */
export const LOG_LEVELS: readonly LogLevel[] = ["error", "warn", "info", "debug", "trace"];

const LEVEL_WORDS: [RegExp, LogLevel][] = [
  [/\b(ERROR|FATAL|PANIC|panicked)\b/, "error"],
  [/\b(WARN|WARNING)\b/, "warn"],
  [/\bINFO\b/, "info"],
  [/\bDEBUG\b/, "debug"],
  [/\bTRACE\b/, "trace"],
];

/**
 * A server log line's level: `level=warn`, `"level":"ERROR"`, `[INFO]` or a
 * tracing-style ` WARN ` word. A line without one is info.
 */
export function logLevel(line: string): LogLevel {
  const keyed = /\blevel["']?\s*[:=]\s*["']?(error|warn(?:ing)?|info|debug|trace)/i.exec(line)?.[1];
  if (keyed) {
    const word = keyed.toLowerCase();
    return word.startsWith("warn") ? "warn" : (word as LogLevel);
  }
  for (const [pattern, level] of LEVEL_WORDS) if (pattern.test(line)) return level;
  if (/\b(error|failed|panicked)\b/i.test(line)) return "error";
  return "info";
}

/** The lines at `level` or more severe that contain `query` (any case). */
export function filterLog(lines: readonly string[], level: LogLevel | "all", query: string): string[] {
  const max = level === "all" ? LOG_LEVELS.length - 1 : LOG_LEVELS.indexOf(level);
  const needle = query.trim().toLowerCase();
  return lines.filter(
    (line) => LOG_LEVELS.indexOf(logLevel(line)) <= max && (!needle || line.toLowerCase().includes(needle)),
  );
}

// ---------------------------------------------------------------- redaction

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Secrets that have a recognisable shape of their own. */
const SECRET_SHAPES: RegExp[] = [
  /\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{16,}/g,
  /\bgh[pousr]_[A-Za-z0-9]{20,}/g,
  /\bgithub_pat_[A-Za-z0-9_]{20,}/g,
  /\bxox[abprs]-[A-Za-z0-9-]{10,}/g,
  /\bAKIA[0-9A-Z]{16}\b/g,
  /\bAIza[0-9A-Za-z_-]{35}/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
];

/** `Bearer <token>` and `Basic <credentials>` in a header. */
const AUTH_SCHEME = /\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}/gi;

/** A value after a key that names a secret: `api_key=…`, `"token": "…"`, `Authorization: …`. */
const SECRET_KEY =
  /\b((?:[A-Za-z0-9]+[_-])*(?:api[_-]?key|apikey|token|secret|password|passwd|authorization|cookie|credentials?))(["']?\s*[:=]\s*["']?)(?!\[redacted\]|(?:Bearer|Basic)\s)([^\s"',;&}]+)/gi;

const EMAIL = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g;

/** Home folders spelled out when the real one is not known (or for another account's). */
const HOME_SHAPES: RegExp[] = [
  /\/Users\/[^/\s"'`:]+/g,
  /\/home\/[^/\s"'`:]+/g,
  /\b[A-Za-z]:\\\\Users\\\\[^\\\s"'`]+/g,
  /\b[A-Za-z]:\\Users\\[^\\\s"'`]+/g,
  /\b[A-Za-z]:\/Users\/[^/\s"'`]+/g,
];

/**
 * Text fit for a public issue: tokens and API keys become `[redacted]`,
 * email addresses `[email]`, and the home folder `~` (as written, with
 * forward slashes, and JSON-escaped on Windows).
 */
export function redact(text: string, home?: string | null): string {
  let out = text;
  for (const shape of SECRET_SHAPES) out = out.replace(shape, "[redacted]");
  out = out.replace(AUTH_SCHEME, "$1 [redacted]");
  out = out.replace(SECRET_KEY, "$1$2[redacted]");
  out = out.replace(EMAIL, "[email]");
  const homes = new Set<string>();
  const clean = home?.replace(/[\\/]+$/, "");
  if (clean && clean.length > 1) {
    homes.add(clean);
    homes.add(clean.replace(/\\/g, "/"));
    homes.add(clean.replace(/\\/g, "\\\\"));
  }
  // The longest spelling first, so `C:\\Users\\a` is not left half-replaced.
  for (const spelling of [...homes].sort((a, b) => b.length - a.length)) {
    out = out.replace(new RegExp(`${escapeRegExp(spelling)}(?![A-Za-z0-9_.-])`, "g"), "~");
  }
  for (const shape of HOME_SHAPES) out = out.replace(shape, "~");
  return out;
}

// ---------------------------------------------------------------- the report

/** Lines of the server log the report carries. */
export const REPORT_LOG_LINES = 200;

export interface DebugReportInput {
  generatedAt: Date;
  state: ServerState;
  /** What the app's connection says (`statusDetail`), when anything. */
  stateDetail?: string | null;
  transport?: string | null;
  server?: ServerInfo | null;
  system?: SystemInfo | null;
  page?: PageInfo | null;
  harnesses: HarnessDiagnostics[];
  log: readonly string[];
  events: OperationalEvent[];
  /** Why the events could not be read, when they could not. */
  eventsError?: string | null;
}

const cell = (value: unknown) =>
  value === null || value === undefined || value === ""
    ? "—"
    : String(value)
        .replace(/\|/g, "\\|")
        .replace(/\s*\n\s*/g, " ");

function table(rows: [string, unknown][]): string {
  return ["| | |", "| --- | --- |", ...rows.map(([k, v]) => `| ${k} | ${cell(v)} |`)].join("\n");
}

/** A fence longer than any run of backticks inside, so the block cannot end early. */
function fenced(body: string, lang = "text"): string {
  const longest = Math.max(0, ...[...body.matchAll(/`+/g)].map((m) => m[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}${lang}\n${body}\n${fence}`;
}

function details(summary: string, body: string): string {
  return `<details>\n<summary>${summary}</summary>\n\n${body}\n\n</details>`;
}

/**
 * Everything Diagnostics shows, as one Markdown block for a GitHub issue:
 * the server, the system, the harnesses, the last 200 server log lines and
 * recent operational events, redacted (`redact`) as a whole.
 */
export function buildDebugReport(input: DebugReportInput): string {
  const { server, system, page } = input;
  const now = input.generatedAt.getTime();
  const parts: string[] = ["## RustCode debug report", `Generated ${input.generatedAt.toISOString()}`];

  const lastExit = server?.lastExit;
  parts.push(
    "### Server",
    table([
      ["State", SERVER_STATE_LABEL[input.state] + (input.stateDetail ? ` (${input.stateDetail})` : "")],
      ["Version", server?.version],
      ["PID", server?.pid],
      ["Binary", server?.path],
      ["Uptime", server?.startedAtMs ? formatDuration(now - server.startedAtMs) : null],
      ["Config folder", server?.configDir],
      ["Restarts after a crash", server ? server.crashRestarts : null],
      [
        "Last exit",
        lastExit
          ? `${describeExit(lastExit)}, ${new Date(lastExit.atMs).toISOString()}: ${lastExit.reason}`
          : null,
      ],
      ["Connection", input.transport],
    ]),
  );

  parts.push(
    "### System",
    table([
      [
        "OS",
        system
          ? [system.os, system.osVersion, system.kernel && `kernel ${system.kernel}`]
              .filter(Boolean)
              .join(" ")
          : null,
      ],
      ["Architecture", system?.arch],
      ["App", system?.appVersion],
      ["Tauri", system?.tauriVersion],
      ["Web view", system ? [system.webview, system.webviewVersion].filter(Boolean).join(" ") : null],
      ["GPU", page?.gpu],
      ["Display scale", page ? `${page.devicePixelRatio}x` : null],
      ["Viewport", page?.viewport],
      ["data-platform", page?.platform],
      ["Graphics environment", system?.graphicsEnv.length ? system.graphicsEnv.join(", ") : null],
      ["User agent", page?.userAgent],
    ]),
  );

  parts.push(
    "### Harnesses",
    input.harnesses.length
      ? [
          "| Harness | On | Installed | Version | Binary | Sign-in | Last error |",
          "| --- | --- | --- | --- | --- | --- | --- |",
          ...input.harnesses.map(
            (h) =>
              `| ${cell(`${h.name} (${h.id})`)} | ${h.enabled ? "yes" : "no"} | ${h.installed ? "yes" : "no"} | ${cell(h.version)} | ${cell(h.path ?? h.command[0])} | ${cell(h.auth)} | ${cell(h.error)} |`,
          ),
        ].join("\n")
      : "No harnesses listed.",
  );

  const log = input.log.slice(-REPORT_LOG_LINES);
  parts.push(
    details(`Server log (last ${log.length} lines)`, log.length ? fenced(log.join("\n")) : "Nothing logged."),
  );

  const events = input.eventsError
    ? `Could not read them: ${input.eventsError}`
    : input.events.length
      ? fenced(
          input.events
            .map((e) => [e.at, e.eventType, e.threadId ?? "-", e.fields ?? ""].join("  ").trimEnd())
            .join("\n"),
        )
      : "None recorded.";
  parts.push(details(`Recent operational events (${input.events.length})`, events));

  return redact(parts.join("\n\n"), system?.home);
}

/** `rustcode-debug-2026-11-08-1205.md`. */
export function reportFileName(at: Date, kind: "debug" | "server-log" = "debug"): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}-${pad(at.getHours())}${pad(at.getMinutes())}`;
  return kind === "debug" ? `rustcode-debug-${stamp}.md` : `rustcode-server-log-${stamp}.log`;
}

/** Rows of `_rustcode/data/query` over `operational_events`, newest first. */
export const OPERATIONAL_EVENTS_SQL =
  "SELECT at, event_type, thread_id, fields_json FROM operational_events ORDER BY at DESC LIMIT 50";

export function parseOperationalEvents(value: unknown): OperationalEvent[] {
  const rows = (value as { rows?: unknown } | null)?.rows;
  if (!Array.isArray(rows)) return [];
  return rows.flatMap((row) => {
    const r = row as Record<string, unknown> | null;
    if (!r || typeof r.event_type !== "string") return [];
    return [
      {
        at: typeof r.at === "string" ? r.at : "",
        eventType: r.event_type,
        threadId: typeof r.thread_id === "string" ? r.thread_id : null,
        fields: typeof r.fields_json === "string" ? r.fields_json : null,
      },
    ];
  });
}
