/**
 * Worktree lifecycle scripts section for `ThreadDetails`: shows explicit
 * first-time trust approval for `.rustcode/worktrees.toml`, `setup` status and
 * logs, and an on-demand Run/Stop control when `run` is configured.
 */

import { useCallback, useEffect, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { request, toast } from "../state/store";
import {
  parseWorktreeScriptsState,
  runStatusLabel,
  type ScriptAction,
  scriptErrorOf,
  setupStatusLabel,
  shouldPollScripts,
  WorktreeScriptMethod,
  type WorktreeScriptsState,
} from "../state/worktreeScripts";
import { ChevronDown, CircleCheck, CircleX, Play, RefreshCw, ShieldAlert, Square, Terminal } from "./icons";
import { cx, Pill, Spinner, Tooltip } from "./primitives";

/** Renders project worktree scripts for `cwd`, or nothing when unconfigured. */
export function WorktreeScriptsSection({ cwd }: { cwd: string | undefined }) {
  const [read, setRead] = useState<{ cwd: string; state: WorktreeScriptsState } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [logsOpen, setLogsOpen] = useState<{ scope: string; open: boolean }>({ scope: "", open: false });

  const state = read && read.cwd === cwd ? read.state : undefined;
  const polling = shouldPollScripts(state);

  const load = useCallback(async () => {
    if (!cwd) return;
    try {
      const answer = (await request(WorktreeScriptMethod.state, { path: cwd }, 15_000)) as Json;
      const parsed = parseWorktreeScriptsState(answer);
      if (parsed) setRead({ cwd, state: parsed });
    } catch {
      return;
    }
  }, [cwd]);

  useEffect(() => {
    if (!cwd) return;
    void load();
  }, [cwd, load]);

  useEffect(() => {
    if (!cwd || !polling) return;
    const timer = window.setInterval(() => void load(), 1_500);
    return () => window.clearInterval(timer);
  }, [cwd, load, polling]);

  if (!cwd || !state?.configured) return null;

  const setTrust = async (trusted: boolean, runSetupAfter = false) => {
    setBusy(trusted ? "trust" : "revoke");
    try {
      const answer = (await request(
        WorktreeScriptMethod.trust,
        { path: cwd, sha256: state.sha256 ?? null, trusted },
        20_000,
      )) as Json;
      const parsed = parseWorktreeScriptsState(answer);
      if (parsed) setRead({ cwd, state: parsed });
      if (trusted && runSetupAfter && (parsed?.config?.setup.length ?? 0) > 0) {
        const started = (await request(
          WorktreeScriptMethod.action,
          { path: cwd, action: "setup" },
          20_000,
        )) as Json;
        const next = parseWorktreeScriptsState(started);
        if (next) setRead({ cwd, state: next });
      }
    } catch (error) {
      toast(scriptErrorOf(error).message, "error");
    } finally {
      setBusy(null);
    }
  };

  const trigger = async (action: ScriptAction) => {
    setBusy(action);
    try {
      const answer = (await request(
        WorktreeScriptMethod.action,
        { path: cwd, action },
        action === "teardown" ? 310_000 : 20_000,
      )) as Json;
      const parsed = parseWorktreeScriptsState(answer);
      if (parsed) setRead({ cwd, state: parsed });
    } catch (error) {
      toast(scriptErrorOf(error).message, "error");
    } finally {
      setBusy(null);
    }
  };

  const config = state.config;
  const hasSetup = (config?.setup.length ?? 0) > 0;
  const hasTeardown = (config?.teardown.length ?? 0) > 0;
  const hasRun = (config?.run.length ?? 0) > 0;
  const runActive = state.run.status === "running";
  const setupLog = state.setup.output.trim();
  const runLog = state.run.output.trim();
  const activeLog = runLog || setupLog;
  const logScope = `${cwd}:${state.setup.status}:${state.run.status}`;
  const showLogs =
    logsOpen.scope === logScope
      ? logsOpen.open
      : state.setup.status === "failed" || state.run.status === "failed";

  return (
    <section className="mx-2 mt-2 rounded-lg bg-fill-subtle p-2 text-callout">
      <div className="flex items-center gap-1.5">
        <Terminal size={13} className="shrink-0 text-fg-muted" />
        <span className="font-medium text-fg">Worktree scripts</span>
        <span className="flex-1" />
        {state.trusted && (
          <Tooltip label="Revoke script trust for this repository">
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void setTrust(false)}
              className="rounded-sm px-1.5 py-0.5 text-caption text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
            >
              Revoke trust
            </button>
          </Tooltip>
        )}
      </div>

      {state.error && <p className="mt-2 text-caption text-danger">{state.error}</p>}

      {!state.error && !state.trusted && config && (
        <div className="mt-2 flex flex-col gap-2">
          <div className="flex items-start gap-1.5 text-caption text-warning">
            <ShieldAlert size={13} className="mt-0.5 shrink-0" />
            <span>Review commands in .rustcode/worktrees.toml before trusting this project.</span>
          </div>
          <div className="flex flex-col gap-1 rounded-md bg-bg/70 p-2 font-mono text-caption text-fg-muted">
            {hasSetup && (
              <div>
                <span className="text-fg-faint">setup: </span>
                {config.setup.join(" && ")}
              </div>
            )}
            {hasTeardown && (
              <div>
                <span className="text-fg-faint">teardown: </span>
                {config.teardown.join(" && ")}
              </div>
            )}
            {hasRun && (
              <div>
                <span className="text-fg-faint">run: </span>
                {config.run.join(" && ")}
              </div>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Pill prominent disabled={busy !== null} onClick={() => void setTrust(true, hasSetup)}>
              {busy === "trust" && <Spinner size={12} />}
              {hasSetup ? "Trust & Run Setup" : "Trust scripts"}
            </Pill>
          </div>
        </div>
      )}

      {!state.error && state.trusted && config && (
        <div className="mt-2 flex flex-col gap-1.5">
          {hasSetup && (
            <div className="flex items-center gap-2 rounded-md bg-bg/60 px-2 py-1.5">
              <span className="text-caption text-fg-muted">Setup</span>
              <span
                className={cx(
                  "flex items-center gap-1 text-caption font-medium",
                  state.setup.status === "succeeded" && "text-success",
                  state.setup.status === "failed" && "text-danger",
                  state.setup.status === "running" && "text-accent",
                  state.setup.status === "idle" && "text-fg-muted",
                )}
              >
                {state.setup.status === "running" && <Spinner size={12} />}
                {state.setup.status === "succeeded" && <CircleCheck size={12} />}
                {state.setup.status === "failed" && <CircleX size={12} />}
                {setupStatusLabel(state.setup)}
              </span>
              <span className="flex-1" />
              <Tooltip label="Run setup scripts">
                <button
                  type="button"
                  aria-label="Run setup scripts"
                  disabled={busy !== null || state.setup.status === "running"}
                  onClick={() => void trigger("setup")}
                  className="flex size-6 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
                >
                  <RefreshCw size={12} />
                </button>
              </Tooltip>
            </div>
          )}

          {hasRun && (
            <div className="flex items-center gap-2 rounded-md bg-bg/60 px-2 py-1.5">
              <span
                className={cx(
                  "size-1.5 shrink-0 rounded-full",
                  runActive ? "bg-success" : state.run.status === "failed" ? "bg-danger" : "bg-fg-faint",
                )}
                aria-hidden
              />
              <span className="min-w-0 flex-1 truncate font-mono text-caption text-fg">
                {config.run.join(" && ")}
              </span>
              <span className="shrink-0 text-caption text-fg-muted">{runStatusLabel(state.run)}</span>
              <Tooltip label={runActive ? "Stop dev server" : "Start dev server"}>
                <button
                  type="button"
                  aria-label={runActive ? "Stop dev server" : "Start dev server"}
                  disabled={busy !== null}
                  onClick={() => void trigger(runActive ? "run_stop" : "run_start")}
                  className={cx(
                    "flex h-6 items-center gap-1 rounded-sm px-2 text-caption font-medium disabled:opacity-40",
                    runActive
                      ? "bg-danger/15 text-danger hover:bg-danger/25"
                      : "bg-accent/15 text-accent hover:bg-accent/25",
                  )}
                >
                  {runActive ? <Square size={10} /> : <Play size={10} />}
                  {runActive ? "Stop" : "Run"}
                </button>
              </Tooltip>
            </div>
          )}

          {activeLog && (
            <div>
              <button
                type="button"
                aria-expanded={showLogs}
                onClick={() => setLogsOpen({ scope: logScope, open: !showLogs })}
                className="flex items-center gap-1 rounded-sm px-1 py-0.5 text-caption text-fg-muted hover:text-fg"
              >
                <ChevronDown size={12} className={cx("transition-transform", !showLogs && "-rotate-90")} />
                Script output
              </button>
              {showLogs && (
                <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-bg p-2 font-mono text-caption text-fg-muted">
                  {activeLog}
                </pre>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
