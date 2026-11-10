// Agents across machines (orchestration version 11): a call that names a
// paired machine is executed over that machine's own connection, an id it
// returns carries the machine (`state/threadKeys.ts`), a call without one
// stays here, an offline machine is refused by name, a delegate that finishes
// there wakes its parent here as a local one does, and an answer about a
// keyed thread names the machine that thread runs on.
//
// Everything this file attaches is process-global (the shell transport, the
// machines module's links, the store's machine map): `afterAll` resets it.

import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import { answerOrchestration, resolveMachine } from "./host";
import { type Machine, refreshMachines, resetMachinesForTests, useMachines } from "./machines";
import { connectionFor, resetForTests, startMachineRouting, useApp, watchMachineConnection } from "./store";
import { LOCAL_HOST } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const LINUX_NAME = "Lars's Linux box";
const LINUX_PROJECT = "/home/lars/code/rustcode";
const CALLER = "caller-1";
const CALLER_PROJECT = "/Users/lars/code/rustcode";
const REMOTE = `${LINUX}/remote-1`;

function machine(id: string, name: string, state: Machine["state"] = "connected"): Machine {
  return {
    id,
    name,
    platform: "linux",
    main: false,
    routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
    state,
    detail: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: "2026-11-08T09:00:00+00:00",
  };
}

/** The app's shell, answering the commands a machine's supervisor needs, without one. */
class FakeShell implements Transport {
  readonly name = "mock";
  machines: Machine[] = [machine(LINUX, LINUX_NAME)];

  async start(): Promise<void> {}

  send(): void {}

  async restart(): Promise<void> {}

  async stop(): Promise<void> {}

  async call(command: string): Promise<unknown> {
    if (command === "machines_list") return this.machines;
    if (command === "machine_connect") return { connected: true };
    if (command === "machine_send" || command === "machine_disconnect") return null;
    return null;
  }
}

const sleep = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/** A snapshot the machine would answer `_rustcode/harnesses` with. */
const snapshot = {
  harnesses: [
    {
      id: "claude-code",
      name: "Claude Code",
      kind: "acp",
      builtin: true,
      known: true,
      enabled: true,
      installed: true,
      auth: "ok",
      command: [],
      capabilities: {},
      models: [{ id: "sonnet-5.5", name: "Sonnet 5.5", efforts: ["low", "high"] }],
      accounts: [],
    },
  ],
  default: { harness: "claude-code", model: "sonnet-5.5" },
  onboarded: true,
};

/**
 * A machine's connection whose writes the test can read: the machine answers
 * a harness listing and `session/new`, and every other request as a no-op.
 */
function recordingConnection(sessionId: string | (() => string) = "remote-1"): {
  conn: AcpConnection;
  sent: () => JsonObject[];
} {
  const next = typeof sessionId === "function" ? sessionId : () => sessionId;
  const writes: string[] = [];
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    writes.push(line);
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    const result =
      frame.method === "_rustcode/harnesses"
        ? snapshot
        : frame.method === "session/new"
          ? {
              sessionId: next(),
              configOptions: [],
              _meta: { rustcode: { harness: { id: "claude-code", capabilities: {} } } },
            }
          : { stopReason: "end_turn", configOptions: [] };
    queueMicrotask(() => {
      if (conn.isClosed) return;
      conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result }));
    });
  });
  return { conn, sent: () => writes.map((line) => JSON.parse(line) as JsonObject) };
}

let shell: FakeShell | null = null;

/** The caller and the machine's thread as the app lists them. */
function listed(): void {
  useApp.setState({
    selectedId: null,
    orchestration: true,
    sessions: [
      {
        id: CALLER,
        cwd: CALLER_PROJECT,
        title: "Work the machine network",
        updatedAt: Date.now(),
        harness: "claude-code",
      },
    ],
    threads: {
      [CALLER]: {
        id: CALLER,
        cwd: CALLER_PROJECT,
        title: "Work the machine network",
        items: [],
        queue: [],
        permissions: [],
        running: true,
        loaded: true,
        loading: false,
      },
    } as never,
    links: {
      links: [],
      // This app's grant for a start on that machine, in its folder and there.
      grants: { [CALLER]: [`start@${LINUX}`, `start@${LINUX}:${LINUX_PROJECT}`] },
      counters: {},
      fanOuts: [],
    },
  });
}

/** The machine's thread as a copy this app has open, and the grants a read, message or stop needs. */
function withRemoteThread(): void {
  useApp.setState((s) => ({
    sessions: [
      ...s.sessions.filter((x) => x.id !== REMOTE),
      {
        id: REMOTE,
        cwd: LINUX_PROJECT,
        title: "Run the tests there",
        updatedAt: Date.now(),
        harness: "claude-code",
      },
    ],
    threads: {
      ...s.threads,
      [REMOTE]: {
        id: REMOTE,
        cwd: LINUX_PROJECT,
        title: "Run the tests there",
        items: [
          { kind: "user", text: "run it" },
          { kind: "assistant", text: "done" },
        ],
        queue: [],
        permissions: [],
        running: false,
        loaded: true,
        loading: false,
      },
    } as never,
    links: {
      ...s.links,
      grants: {
        ...s.links.grants,
        [CALLER]: [...(s.links.grants[CALLER] ?? []), REMOTE, `interrupt:${REMOTE}`],
      },
    },
  }));
}

/** A link from the caller to a thread, for the calls that read the link book. */
function link(id: string, target: string) {
  return {
    id: `${CALLER}/${id}`,
    orchestrator: CALLER,
    target,
    orchestratorTitle: "Work the machine network",
    targetTitle: "child",
    message: "go",
    state: "working" as const,
    createdAt: Date.now(),
  };
}

beforeAll(async () => {
  resetForTests();
  shell = new FakeShell();
  setShellTransport(shell);
  await refreshMachines();
  await startMachineRouting();
  await sleep(20);
});

afterAll(() => {
  resetForTests();
  resetMachinesForTests();
  setShellTransport(null);
  shell = null;
});

beforeEach(() => {
  listed();
  useMachines.setState({
    sessions: {
      [LINUX]: [
        {
          id: "remote-1",
          cwd: LINUX_PROJECT,
          title: "Run the tests there",
          updatedAt: Date.now(),
          harness: "claude-code",
        },
      ],
    },
  });
});

describe("naming a machine", () => {
  test("a call without one is this computer, never a main machine", () => {
    expect(resolveMachine(undefined)).toMatchObject({ host: LOCAL_HOST, id: "local" });
    expect(resolveMachine("  ")).toMatchObject({ host: LOCAL_HOST });
    expect(resolveMachine("local")).toMatchObject({ host: LOCAL_HOST });
  });

  test("a machine is its id or its name, any case", () => {
    expect(resolveMachine(LINUX)).toMatchObject({ host: LINUX });
    expect(resolveMachine("LARS'S LINUX BOX")).toMatchObject({ host: LINUX, name: LINUX_NAME });
  });

  test("an unknown name is refused with the ones there are", () => {
    const answer = resolveMachine("Studio") as { code?: string; message?: string };
    expect(answer.code).toBe("not_found");
    expect(answer.message).toContain("list_machines has");
    expect(answer.message).toContain(LINUX_NAME);
  });
});

describe("list_machines", () => {
  test("lists this computer and the paired one, with its harnesses and projects", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const answer = await answerOrchestration({ sessionId: CALLER, tool: "list_machines", args: {} });
    expect(answer.error).toBeUndefined();
    const machines = answer.machines as JsonObject[];
    expect(machines).toHaveLength(2);
    // No local engine runs in this file: this computer is listed, and says it is away.
    expect(machines[0]).toMatchObject({ id: "local", current: true });
    expect(machines[1]).toMatchObject({ id: LINUX, name: LINUX_NAME, platform: "linux", state: "connected" });
    // What the machine says it can run and where it has worked, not this app's own answer.
    expect(machines[1]!.harnesses).toEqual([{ id: "claude-code", name: "Claude Code", available: true }]);
    expect(machines[1]!.projects).toEqual(["rustcode"]);
  });
});

describe("a call on another machine", () => {
  test("starts the thread over that machine's connection and keys it with the machine", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "start_session",
      args: { prompt: "Run the tests there", machine: LINUX_NAME },
    });
    expect(answer.error).toBeUndefined();
    // The id is the key every later call resolves to that machine, and the answer says where.
    expect(answer.session_id).toBe(`${LINUX}/remote-1`);
    expect(answer.machine).toBe(LINUX_NAME);
    expect(answer.machine_id).toBe(LINUX);
    const calls = sent().filter((frame) => typeof frame.method === "string");
    const created = calls.find((frame) => frame.method === "session/new")?.params as JsonObject;
    expect(created).toMatchObject({ cwd: LINUX_PROJECT });
    expect(created._meta).toMatchObject({ rustcode: { harness: "claude-code", parent: CALLER } });
    // The turn goes to the machine's own session id, never the key.
    const prompted = calls.find((frame) => frame.method === "session/prompt");
    expect(prompted?.params).toMatchObject({ sessionId: "remote-1" });
  });

  test("its threads are listed with keys that carry the machine", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const listed = await answerOrchestration({
      sessionId: CALLER,
      tool: "list_sessions",
      args: { machine: LINUX },
    });
    expect(listed.error).toBeUndefined();
    expect(listed.machine).toBe(LINUX_NAME);
    expect(listed.sessions).toMatchObject([{ session_id: `${LINUX}/remote-1`, machine: LINUX_NAME }]);
  });

  test("a call on a keyed id names that thread's machine, without machine", async () => {
    withRemoteThread();
    const read = await answerOrchestration({
      sessionId: CALLER,
      tool: "read_session",
      args: { session_id: REMOTE },
    });
    expect(read.error).toBeUndefined();
    expect(read.machine).toBe(LINUX_NAME);
    expect(read.machine_id).toBe(LINUX);
    expect(String(read.text)).toContain("run it");

    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const message = await answerOrchestration({
      sessionId: CALLER,
      tool: "message_session",
      args: { session_id: REMOTE, message: "again" },
    });
    expect(message.error).toBeUndefined();
    expect(message.machine).toBe(LINUX_NAME);
    // It went to the machine's own session id, never the key.
    const prompted = sent().find((frame) => frame.method === "session/prompt");
    expect(prompted?.params).toMatchObject({ sessionId: "remote-1" });

    const interrupt = await answerOrchestration({
      sessionId: CALLER,
      tool: "interrupt_session",
      args: { session_id: REMOTE },
    });
    expect(interrupt.error).toBeUndefined();
    expect(interrupt.machine).toBe(LINUX_NAME);

    const status = await answerOrchestration({
      sessionId: CALLER,
      tool: "thread_status",
      args: { session_id: REMOTE },
    });
    expect(status.error).toBeUndefined();
    expect(status.machine).toBe(LINUX_NAME);
  });

  test("thread_status without a target reports only the named machine's delegates", async () => {
    useApp.setState((s) => ({
      sessions: [
        ...s.sessions,
        { id: "local-2", cwd: CALLER_PROJECT, title: "Local child", updatedAt: Date.now() },
      ],
      links: { ...s.links, links: [link("9", "local-2"), link("10", REMOTE)] },
    }));
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "thread_status",
      args: { machine: LINUX },
    });
    expect(answer.error).toBeUndefined();
    expect(answer.machine).toBe(LINUX_NAME);
    expect((answer.delegates as JsonObject[]).map((entry) => entry.session_id)).toEqual([REMOTE]);
  });

  test("wait_session refuses a link that is not on the machine it names", async () => {
    useApp.setState((s) => ({
      sessions: [
        ...s.sessions,
        { id: "local-2", cwd: CALLER_PROJECT, title: "Local child", updatedAt: Date.now() },
      ],
      links: { ...s.links, links: [link("9", "local-2")] },
    }));
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "wait_session",
      args: { link_id: `${CALLER}/9`, machine: LINUX },
    });
    expect(answer.error).toMatchObject({ code: "invalid" });
    expect(String((answer.error as JsonObject).message)).toContain(`${CALLER}/9`);
    expect(String((answer.error as JsonObject).message)).toContain(LINUX_NAME);
  });

  test("a wait on an offline machine's link waits rather than refusing it", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const started = await answerOrchestration({
      sessionId: CALLER,
      tool: "start_session",
      args: { prompt: "Run the tests there", machine: LINUX },
    });
    const id = String(started.link_id);
    // The link is still this app's to read: only the machine went away.
    watchMachineConnection(LINUX, null);
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "wait_session",
      args: { link_id: id, machine: LINUX, timeout_ms: 1 },
    });
    expect(answer.error).toBeUndefined();
    expect(answer.machine).toBe(LINUX_NAME);
    // A timeout or the link's own state, never an offline refusal.
    expect(Array.isArray(answer.links) || typeof answer.status === "string").toBe(true);
  });

  test("fan_out names the project on that machine", async () => {
    let n = 0;
    const { conn, sent } = recordingConnection(() => `fan-${++n}`);
    watchMachineConnection(LINUX, conn);
    // The grant names that folder there, as a remote start in it would.
    useApp.setState((s) => ({
      links: {
        ...s.links,
        grants: {
          ...s.links.grants,
          [CALLER]: [...(s.links.grants[CALLER] ?? []), `start@${LINUX}:/home/lars/other`],
        },
      },
    }));
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "fan_out",
      args: {
        prompt: "Run the tests there",
        machine: LINUX,
        project: "/home/lars/other",
        harnesses: [
          { harness: "claude-code", model: "sonnet-5.5" },
          { harness: "claude-code", effort: "high" },
        ],
      },
    });
    expect(answer.error).toBeUndefined();
    expect(answer.machine).toBe(LINUX_NAME);
    const cwds = sent()
      .filter((frame) => frame.method === "session/new")
      .map((frame) => (frame.params as JsonObject).cwd);
    expect(cwds).toEqual(["/home/lars/other", "/home/lars/other"]);
  });

  test("an offline machine is refused by name and when it went away", async () => {
    watchMachineConnection(LINUX, null);
    useMachines.setState({ offlineSince: { [LINUX]: Date.parse("2026-11-08T09:12:00") } });
    const answer = await answerOrchestration({
      sessionId: CALLER,
      tool: "start_session",
      args: { prompt: "Run the tests there", machine: LINUX_NAME },
    });
    expect(answer.error).toMatchObject({ code: "offline" });
    expect(String((answer.error as JsonObject).message)).toContain(LINUX_NAME);
    expect(String((answer.error as JsonObject).message)).toContain("offline since");
    // A second call on it says the same, and nothing was started there.
    const listed = await answerOrchestration({
      sessionId: CALLER,
      tool: "list_sessions",
      args: { machine: LINUX },
    });
    expect(listed.error).toMatchObject({ code: "offline" });
    expect(useApp.getState().threads[`${LINUX}/remote-1`]).toBeUndefined();
    expect(connectionFor(LINUX)).toBeNull();
  });
});

describe("a delegate that finishes on another machine", () => {
  test("its link settles and its parent is woken with the answer", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    const started = await answerOrchestration({
      sessionId: CALLER,
      tool: "start_session",
      args: { prompt: "Run the tests there", machine: LINUX },
    });
    expect(started.error).toBeUndefined();
    const link = String(started.link_id);
    // The parent is busy, so the wake waits in its queue instead of starting a turn here.
    await sleep(120);
    const state = useApp.getState();
    const settled = state.links.links.find((entry) => entry.id === link);
    expect(settled?.state).toBe("done");
    expect(settled?.machine).toBe(LINUX_NAME);
    // The wake names the link: the parent is told its remote delegate finished.
    const queued = state.threads[CALLER]?.queue ?? [];
    expect(queued.some((item) => item.wake && item.wakeLinks?.includes(link))).toBe(true);
  });
});
