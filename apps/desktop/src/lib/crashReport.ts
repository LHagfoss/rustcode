/**
 * Client-side crash reporting state, scrubber, rate limiter and global webview
 * error hooks. Reports are forwarded to the Tauri shell (`crash_report_send`),
 * which drops everything unless a build DSN is present and reporting is on.
 */

import { hasShell, shellCall } from "./shell";
import { readFlag, readStorage, writeFlag } from "./storage";

const ENABLED_KEY = "rustcode.crashReports";
const NOTICE_SEEN_KEY = "rustcode.crashReportsNoticeSeen";

/** Crash reporting is enabled by default; users can opt out without restarting. */
export const CRASH_REPORTS_DEFAULT = true;

export const CRASH_REPORT_NOTICE_ID = "crash-reports-notice";

const DEDUPE_WINDOW_MS = 60_000;
const RATE_WINDOW_MS = 5 * 60_000;
const RATE_MAX_EVENTS = 10;
const MAX_NAME_CHARS = 128;
const MAX_MESSAGE_CHARS = 512;
const MAX_STACK_CHARS = 8192;

export interface CrashReportStatus {
  enabled: boolean;
  configured: boolean;
}

const listeners = new Set<() => void>();
let configuredState = false;
const configuredListeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) listener();
}

function notifyConfigured() {
  for (const listener of configuredListeners) listener();
}

export function subscribeCrashReports(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function subscribeCrashReportConfigured(listener: () => void): () => void {
  configuredListeners.add(listener);
  return () => configuredListeners.delete(listener);
}

export function crashReportsConfigured(): boolean {
  return configuredState;
}

export function setCrashReportsConfiguredForTest(configured: boolean) {
  configuredState = configured;
  notifyConfigured();
}

export function crashReportsEnabled(): boolean {
  return readFlag(ENABLED_KEY, CRASH_REPORTS_DEFAULT);
}

export function crashReportNoticeSeen(): boolean {
  return readFlag(NOTICE_SEEN_KEY, false);
}

export function markCrashReportNoticeSeen() {
  writeFlag(NOTICE_SEEN_KEY, true);
}

export function setCrashReportsEnabled(enabled: boolean) {
  writeFlag(ENABLED_KEY, enabled);
  markCrashReportNoticeSeen();
  notify();
  if (hasShell()) {
    void shellCall<CrashReportStatus>("crash_report_set_enabled", { enabled }).catch(() => undefined);
  }
}

const SENSITIVE_JSON_KEYS = new Set([
  "prompt",
  "prompts",
  "content",
  "text",
  "message",
  "messages",
  "input",
  "system",
  "authorization",
  "api_key",
  "apikey",
  "token",
  "access_token",
  "refresh_token",
  "secret",
  "password",
  "dsn",
  "cookie",
]);

const SINGLE_WORD_ENV_KEYS = new Set([
  "HOME",
  "USER",
  "PATH",
  "SHELL",
  "PWD",
  "LOGNAME",
  "USERNAME",
  "USERPROFILE",
  "APPDATA",
  "LOCALAPPDATA",
  "TOKEN",
  "SECRET",
  "PASSWORD",
  "KEY",
  "DSN",
  "AUTH",
  "COOKIE",
]);

function truncate(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  return `${text.slice(0, maxChars)}…`;
}

function scrubJsonSensitiveFields(input: string): string {
  return input.replace(
    /"([A-Za-z0-9_]+)"(\s*:\s*)(?:"(?:\\.|[^"\\])*"|\[[^\]]*\])/g,
    (match, key: string, sep: string) =>
      SENSITIVE_JSON_KEYS.has(key.toLowerCase()) ? `"${key}"${sep}"[REDACTED]"` : match,
  );
}

/** Scrubs home paths, prompt fields, tokens and environment assignments before IPC or upload. */
export function scrubCrashText(input: string, homeDir?: string): string {
  let out = input;
  if (homeDir) {
    const trimmed = homeDir.replace(/[/\\]+$/, "");
    if (trimmed.length > 1) {
      out = out.split(trimmed).join("~");
      const slash = trimmed.replaceAll("\\", "/");
      if (slash !== trimmed) out = out.split(slash).join("~");
    }
  }
  out = out.replace(/(?:\/Users|\/home)\/[^\s/"'`),;:\]]+/g, "~");
  out = out.replace(/[A-Za-z]:[/\\][Uu]sers[/\\][^\s/\\"'`),;:\]]+/g, "~");
  out = scrubJsonSensitiveFields(out);
  out = out.replace(/\b(Bearer|Basic)\s+[A-Za-z0-9._+/=-]+/gi, "$1 [REDACTED]");
  out = out.replace(
    /(?<![A-Za-z0-9])(?:sk-ant-|sk-proj-|sk-|ghp_|gho_|ghu_|ghs_|github_pat_|AIza|xox[baprs]-|AKIA)[A-Za-z0-9._+/=-]{8,}/g,
    "[REDACTED]",
  );
  out = out.replace(
    /(?<![A-Za-z0-9_])([A-Z][A-Z0-9_]{2,})=(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s"',;)\]}]+)/g,
    (match, key: string) =>
      key.includes("_") || SINGLE_WORD_ENV_KEYS.has(key) ? `${key}=[REDACTED]` : match,
  );
  return out;
}

export class CrashRateLimiter {
  private readonly seen = new Map<string, number>();
  private readonly recent: number[] = [];

  allow(fingerprint: string, now = Date.now()): boolean {
    for (const [key, at] of this.seen) {
      if (now - at >= DEDUPE_WINDOW_MS) this.seen.delete(key);
    }
    while (this.recent.length > 0 && now - this.recent[0]! >= RATE_WINDOW_MS) {
      this.recent.shift();
    }
    if (this.seen.has(fingerprint) || this.recent.length >= RATE_MAX_EVENTS) {
      return false;
    }
    this.seen.set(fingerprint, now);
    this.recent.push(now);
    return true;
  }

  reset() {
    this.seen.clear();
    this.recent.length = 0;
  }
}

const limiter = new CrashRateLimiter();

export function resetCrashRateLimiterForTest() {
  limiter.reset();
}

function normalizeError(error: unknown): { name: string; message: string; stack?: string } {
  if (error instanceof Error) {
    return {
      name: error.name || "Error",
      message: error.message || String(error),
      stack: error.stack,
    };
  }
  if (typeof error === "string") {
    return { name: "Error", message: error };
  }
  try {
    return { name: "Error", message: JSON.stringify(error) ?? String(error) };
  } catch {
    return { name: "Error", message: String(error) };
  }
}

export interface ReportWebErrorOptions {
  source?: "webview" | "react";
  componentStack?: string;
  now?: number;
}

/** Scrubs, rate-limits and forwards an uncaught webview error to the Tauri shell. */
export function reportWebError(error: unknown, options: ReportWebErrorOptions = {}): boolean {
  if (!crashReportsEnabled() || !hasShell()) return false;
  const source = options.source ?? "webview";
  const normalized = normalizeError(error);
  const name = truncate(scrubCrashText(normalized.name), MAX_NAME_CHARS) || "Error";
  const message = truncate(scrubCrashText(normalized.message), MAX_MESSAGE_CHARS);
  const stack = normalized.stack ? truncate(scrubCrashText(normalized.stack), MAX_STACK_CHARS) : undefined;
  const componentStack = options.componentStack
    ? truncate(scrubCrashText(options.componentStack), MAX_STACK_CHARS)
    : undefined;
  const firstFrame = stack?.split("\n").find((line) => line.trim().length > 0) ?? "";
  const fingerprint = `${source}\0${name}\0${message.slice(0, 120)}\0${firstFrame.slice(0, 120)}`;
  if (!limiter.allow(fingerprint, options.now ?? Date.now())) return false;

  void shellCall("crash_report_send", {
    source,
    name,
    message,
    stack: stack ?? null,
    componentStack: componentStack ?? null,
  }).catch(() => undefined);
  return true;
}

/** Syncs crash reporting preference and build DSN status with the Tauri shell. */
export async function syncCrashReporting(): Promise<CrashReportStatus | null> {
  if (!hasShell()) return null;
  try {
    const hasLocal = readStorage(ENABLED_KEY) !== null;
    const status = hasLocal
      ? await shellCall<CrashReportStatus>("crash_report_set_enabled", {
          enabled: crashReportsEnabled(),
        })
      : await shellCall<CrashReportStatus>("crash_report_status");
    if (!hasLocal && typeof status?.enabled === "boolean" && !status.enabled) {
      writeFlag(ENABLED_KEY, false);
      markCrashReportNoticeSeen();
      notify();
    }
    if (typeof status?.configured === "boolean") {
      configuredState = status.configured;
      notifyConfigured();
    }
    return status;
  } catch {
    return null;
  }
}

/** Registers global error listeners and forwards uncaught webview errors when enabled. */
export function installCrashReporting(): () => void {
  const onError = (event: ErrorEvent) => {
    reportWebError(event.error ?? event.message ?? "Uncaught error", { source: "webview" });
  };
  const onRejection = (event: PromiseRejectionEvent) => {
    reportWebError(event.reason ?? "Unhandled promise rejection", { source: "webview" });
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onRejection);
  };
}
