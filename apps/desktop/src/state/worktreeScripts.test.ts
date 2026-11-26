import { describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import {
  parseWorktreeScriptsState,
  runStatusLabel,
  scriptErrorOf,
  setupStatusLabel,
  shouldPollScripts,
} from "./worktreeScripts";

describe("parseWorktreeScriptsState", () => {
  test("parses configured, trusted script state with setup and run details", () => {
    const parsed = parseWorktreeScriptsState({
      configured: true,
      configPath: "/repo/.rustcode/worktrees.toml",
      sha256: "abc123",
      trusted: true,
      config: {
        setup: ["bun install", " "],
        teardown: ["docker compose down"],
        run: ["bun run dev"],
        cwd: "apps/web",
        timeoutSecs: 180,
      },
      setup: {
        status: "succeeded",
        output: "done",
        exitCode: 0,
      },
      run: {
        status: "running",
        pid: 4242,
        command: "bun run dev",
        output: "listening on 3000",
      },
    });
    expect(parsed).toEqual({
      configured: true,
      configPath: "/repo/.rustcode/worktrees.toml",
      sha256: "abc123",
      trusted: true,
      error: undefined,
      config: {
        setup: ["bun install"],
        teardown: ["docker compose down"],
        run: ["bun run dev"],
        cwd: "apps/web",
        timeoutSecs: 180,
      },
      setup: {
        status: "succeeded",
        output: "done",
        command: undefined,
        exitCode: 0,
        updatedAt: undefined,
      },
      run: {
        status: "running",
        pid: 4242,
        command: "bun run dev",
        exitCode: undefined,
        output: "listening on 3000",
        startedAt: undefined,
      },
    });
  });

  test("falls back cleanly for unconfigured projects and unknown statuses", () => {
    const parsed = parseWorktreeScriptsState({
      configured: false,
      trusted: false,
      setup: { status: "bogus" },
      run: { status: "bogus" },
    });
    expect(parsed?.configured).toBe(false);
    expect(parsed?.setup.status).toBe("idle");
    expect(parsed?.run.status).toBe("idle");
    expect(parseWorktreeScriptsState(undefined)).toBeUndefined();
  });
});

describe("polling and labels", () => {
  test("polls only while setup or run is active", () => {
    const idle = parseWorktreeScriptsState({
      configured: true,
      trusted: true,
      setup: { status: "succeeded" },
      run: { status: "stopped" },
    });
    expect(shouldPollScripts(idle)).toBe(false);

    const settingUp = parseWorktreeScriptsState({
      configured: true,
      trusted: true,
      setup: { status: "running" },
      run: { status: "idle" },
    });
    expect(shouldPollScripts(settingUp)).toBe(true);

    const runningServer = parseWorktreeScriptsState({
      configured: true,
      trusted: true,
      setup: { status: "idle" },
      run: { status: "running" },
    });
    expect(shouldPollScripts(runningServer)).toBe(true);
  });

  test("formats setup and run status labels", () => {
    expect(setupStatusLabel({ status: "needs_trust", output: "" })).toBe("Needs trust");
    expect(setupStatusLabel({ status: "running", output: "" })).toBe("Running…");
    expect(setupStatusLabel({ status: "succeeded", output: "" })).toBe("Ready");
    expect(setupStatusLabel({ status: "failed", output: "", exitCode: 2 })).toBe("Failed (2)");
    expect(runStatusLabel({ status: "running", output: "" })).toBe("Running");
    expect(runStatusLabel({ status: "failed", output: "", exitCode: 1 })).toBe("Exited (1)");
    expect(runStatusLabel({ status: "stopped", output: "" })).toBe("Stopped");
  });

  test("extracts engine reason and message from RPC errors", () => {
    const parsed = scriptErrorOf(
      new RpcError(-32602, "invalid", { reason: "untrusted", message: "Trust scripts first" }),
    );
    expect(parsed).toEqual({ reason: "untrusted", message: "Trust scripts first" });
  });
});
