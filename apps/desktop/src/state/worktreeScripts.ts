/**
 * Client contract and helpers for project worktree lifecycle scripts
 * (`.rustcode/worktrees.toml`, `rustcode/engine/src/worktree_scripts.rs`).
 */

import { arr, bool, type Json, num, obj, RpcError, str } from "../acp/jsonrpc";

export const WorktreeScriptMethod = {
  state: "_rustcode/worktrees/scripts/state",
  trust: "_rustcode/worktrees/scripts/trust",
  action: "_rustcode/worktrees/scripts/action",
} as const;

export type SetupStatus = "idle" | "needs_trust" | "running" | "succeeded" | "failed";
export type RunStatus = "idle" | "running" | "stopped" | "failed";
export type ScriptAction = "setup" | "teardown" | "run_start" | "run_stop";

export interface WorktreeScriptConfig {
  setup: string[];
  teardown: string[];
  run: string[];
  cwd?: string;
  timeoutSecs: number;
}

export interface SetupState {
  status: SetupStatus;
  output: string;
  command?: string;
  exitCode?: number;
  updatedAt?: string;
}

export interface RunState {
  status: RunStatus;
  pid?: number;
  command?: string;
  exitCode?: number;
  output: string;
  startedAt?: string;
}

export interface WorktreeScriptsState {
  configured: boolean;
  configPath?: string;
  sha256?: string;
  trusted: boolean;
  config?: WorktreeScriptConfig;
  error?: string;
  setup: SetupState;
  run: RunState;
}

function parseSetupStatus(raw: string | undefined): SetupStatus {
  if (raw === "needs_trust" || raw === "running" || raw === "succeeded" || raw === "failed") {
    return raw;
  }
  return "idle";
}

function parseRunStatus(raw: string | undefined): RunStatus {
  if (raw === "running" || raw === "stopped" || raw === "failed") return raw;
  return "idle";
}

function stringList(value: Json | undefined): string[] {
  return arr(value)
    .map((item) => str(item)?.trim() ?? "")
    .filter(Boolean);
}

/** Parses the JSON answer from `_rustcode/worktrees/scripts/*`. */
export function parseWorktreeScriptsState(value: Json | undefined): WorktreeScriptsState | undefined {
  const o = obj(value);
  if (!o) return undefined;
  const cfg = obj(o.config);
  const setupObj = obj(o.setup);
  const runObj = obj(o.run);
  return {
    configured: bool(o.configured) ?? false,
    configPath: str(o.configPath),
    sha256: str(o.sha256),
    trusted: bool(o.trusted) ?? false,
    error: str(o.error),
    config: cfg
      ? {
          setup: stringList(cfg.setup),
          teardown: stringList(cfg.teardown),
          run: stringList(cfg.run),
          cwd: str(cfg.cwd),
          timeoutSecs: num(cfg.timeoutSecs) ?? 300,
        }
      : undefined,
    setup: {
      status: parseSetupStatus(str(setupObj?.status)),
      output: str(setupObj?.output) ?? "",
      command: str(setupObj?.command),
      exitCode: num(setupObj?.exitCode),
      updatedAt: str(setupObj?.updatedAt),
    },
    run: {
      status: parseRunStatus(str(runObj?.status)),
      pid: num(runObj?.pid),
      command: str(runObj?.command),
      exitCode: num(runObj?.exitCode),
      output: str(runObj?.output) ?? "",
      startedAt: str(runObj?.startedAt),
    },
  };
}

/** True while a setup hook or managed dev server is running and should be polled for updates. */
export function shouldPollScripts(state: WorktreeScriptsState | undefined): boolean {
  return state?.setup.status === "running" || state?.run.status === "running";
}

/** Readable error message from a refused worktree script RPC. */
export function scriptErrorOf(error: unknown): { reason: string; message: string } {
  const data = error instanceof RpcError ? obj(error.data as Json) : undefined;
  const message = str(data?.message) ?? (error instanceof Error ? error.message : String(error));
  return { reason: str(data?.reason) ?? "failed", message };
}

/** Concise label for the setup hook status pill. */
export function setupStatusLabel(setup: SetupState): string {
  switch (setup.status) {
    case "needs_trust":
      return "Needs trust";
    case "running":
      return "Running…";
    case "succeeded":
      return "Ready";
    case "failed":
      return setup.exitCode !== undefined ? `Failed (${setup.exitCode})` : "Failed";
    default:
      return "Idle";
  }
}

/** Concise label for the managed run script status pill. */
export function runStatusLabel(run: RunState): string {
  switch (run.status) {
    case "running":
      return "Running";
    case "failed":
      return run.exitCode !== undefined ? `Exited (${run.exitCode})` : "Failed";
    case "stopped":
      return "Stopped";
    default:
      return "Idle";
  }
}
