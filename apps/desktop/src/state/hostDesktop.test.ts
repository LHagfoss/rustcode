/**
 * Cross-machine desktop control tests: routing `computer_*` with `machine`
 * through the local RustCode app, refusing third-party targets, showing the
 * approval dialog on the controlled machine, redacting typed text, and never
 * auto-approving under Full access.
 */

import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import type { Transport } from "../acp/transport";
import { Method } from "../acp/types";
import { answerOrchestration, resolveGrant } from "./host";
import { desktopApprovalDetail } from "./hostDesktop";
import { type Machine, resetMachinesForTests, useMachines } from "./machines";
import { parseCall } from "./orchestrationCalls";
import { connect, resetForTests, useApp, watchMachineConnection } from "./store";
import { LOCAL_HOST, threadKey } from "./threadKeys";

const VPS = "gw-vps-001122334455";
const VPS_NAME = "Ubuntu VPS";
const REMOTE_SESSION = "vps-thread-1";

function vpsMachine(): Machine {
  return {
    id: VPS,
    name: VPS_NAME,
    platform: "linux",
    routes: [{ kind: "overlay", address: "100.64.0.10:17879", encrypted: true }],
    route: { kind: "overlay", address: "100.64.0.10:17879", encrypted: true },
    state: "connected",
    detail: null,
    hint: null,
    version: "1.56.0",
    retryInSecs: null,
    addedAt: "2026-11-27T09:00:00+00:00",
    lastConnectedAt: "2026-11-27T09:00:00+00:00",
  };
}

class DesktopBridgeTransport implements Transport {
  readonly name = "mock";
  private readonly mock = new MockTransport({ delayMs: 0 });
  private handlers: Parameters<Transport["start"]>[0] | null = null;
  readonly desktopCalls: JsonObject[] = [];
  onDesktopCall: (params: JsonObject) => JsonObject = () => ({ status: "ok", result: {} });

  async start(handlers: Parameters<Transport["start"]>[0]): Promise<void> {
    this.handlers = handlers;
    await this.mock.start(handlers);
  }

  send(line: string): void {
    const message = JSON.parse(line) as { id?: number | string; method?: string; params?: JsonObject };
    if (message.method === Method.desktopCall && message.id !== undefined) {
      const params = message.params ?? {};
      this.desktopCalls.push(params);
      const result = this.onDesktopCall(params);
      queueMicrotask(() => {
        this.handlers?.({
          type: "line",
          line: JSON.stringify({ jsonrpc: "2.0", id: message.id, result }),
        });
      });
      return;
    }
    this.mock.send(line);
  }

  async restart(): Promise<void> {
    await this.mock.restart();
  }

  async stop(): Promise<void> {
    await this.mock.stop();
  }
}

const wait = (ms = 15) => new Promise((resolve) => setTimeout(resolve, ms));

describe("cross-machine desktop control", () => {
  afterAll(() => {
    watchMachineConnection(VPS, null);
    resetForTests();
    resetMachinesForTests();
  });

  beforeEach(() => {
    watchMachineConnection(VPS, null);
    resetForTests();
    useMachines.setState({ machines: [vpsMachine()] });
  });

  async function bootLocal(
    onDesktopCall: (params: JsonObject) => JsonObject,
  ): Promise<DesktopBridgeTransport> {
    const transport = new DesktopBridgeTransport();
    transport.onDesktopCall = onDesktopCall;
    await connect(transport);
    useMachines.setState({ machines: [vpsMachine()] });
    const remoteKey = threadKey(VPS, REMOTE_SESSION);
    useApp.setState((s) => ({
      sessions: [
        ...s.sessions,
        {
          id: remoteKey,
          title: "Fix Safari login",
          cwd: "/home/lucas/code/app",
          updatedAt: Date.now(),
        },
      ],
      permissionModes: { ...s.permissionModes, [remoteKey]: "full" },
    }));
    return transport;
  }

  test("parseCall accepts all computer_* tools with machine and callId", () => {
    const parsed = parseCall({
      tool: "computer_click",
      callId: "desktop-call-7",
      args: { pid: 42, ref: "e12", machine: "Main Mac", purpose: "click Save" },
    });
    expect(parsed).toEqual({
      tool: "computer_desktop",
      desktopTool: "computer_click",
      args: { pid: 42, ref: "e12", machine: "Main Mac", purpose: "click Save" },
      machine: "Main Mac",
      callId: "desktop-call-7",
    });
  });

  test("desktopApprovalDetail redacts typed text and omits machine", () => {
    const detail = desktopApprovalDetail("computer_type", {
      pid: 42,
      ref: "e3",
      text: "super-secret",
      machine: "local",
      purpose: "fill search box",
    });
    expect(detail).toContain("fill search box");
    expect(detail).toContain("[redacted]");
    expect(detail).not.toContain("super-secret");
    expect(detail).not.toContain("machine");
  });

  test("refuses desktop control targeting another paired machine instead of this app's computer", async () => {
    await bootLocal(() => ({ status: "ok", result: {} }));
    const vpsConn = new AcpConnection(() => {});
    watchMachineConnection(VPS, vpsConn);
    try {
      const answer = await answerOrchestration(
        {
          sessionId: REMOTE_SESSION,
          tool: "computer_windows",
          args: { machine: VPS_NAME },
        },
        VPS,
      );
      expect(answer.error).toMatchObject({ code: "not_allowed" });
    } finally {
      watchMachineConnection(VPS, null);
    }
  });

  test("asks on the controlled Mac even when the remote thread is in Full access, then forwards approval", async () => {
    const transport = await bootLocal((params): JsonObject => {
      if (!params.approval) {
        return {
          status: "needs_approval",
          app: { key: "com.apple.Safari", name: "Safari", sensitive: false },
        };
      }
      return {
        status: "ok",
        result: { clicked: "e12" },
      };
    });

    const pending = answerOrchestration(
      {
        sessionId: REMOTE_SESSION,
        tool: "computer_type",
        callId: "desktop-call-1",
        args: { pid: 99, ref: "e12", text: "hello", machine: "local", purpose: "enter query" },
      },
      VPS,
    );

    await wait();
    const grant = useApp.getState().grant;
    expect(grant).toBeTruthy();
    expect(grant?.kind).toBe("desktop");
    expect(grant?.target).toBe("Safari");
    expect(grant?.fromMachine).toBe(VPS_NAME);
    expect(grant?.onceOnly).toBe(false);
    expect(grant?.detail).toContain("[redacted]");
    expect(grant?.detail).not.toContain("hello");

    resolveGrant("always");
    const answer = await pending;
    expect(answer.error).toBeUndefined();
    expect(answer.clicked).toBe("e12");
    expect(answer.machine_id).toBe("local");
    expect(transport.desktopCalls).toHaveLength(2);
    expect(transport.desktopCalls[1]).toMatchObject({
      approval: "app",
      approvedApp: "com.apple.Safari",
      callerMachine: VPS_NAME,
    });
  });

  test("sensitive apps force once-only approval and never send app-wide grant", async () => {
    const transport = await bootLocal((params): JsonObject => {
      if (!params.approval) {
        return {
          status: "needs_approval",
          app: { key: "com.apple.Passwords", name: "Passwords", sensitive: true },
        };
      }
      return { status: "ok", result: { ok: true } };
    });

    const pending = answerOrchestration(
      {
        sessionId: REMOTE_SESSION,
        tool: "computer_snapshot",
        args: { pid: 55, machine: LOCAL_HOST || "local" },
      },
      VPS,
    );

    await wait();
    const grant = useApp.getState().grant;
    expect(grant?.onceOnly).toBe(true);
    expect(grant?.sensitive).toBe(true);
    resolveGrant("once");
    const answer = await pending;
    expect(answer.ok).toBe(true);
    expect(transport.desktopCalls[1]?.approval).toBe("once");
  });
});
