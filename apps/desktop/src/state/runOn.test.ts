// **Run on**: the computer a new thread starts on (docs/desktop.md › Several machines). This
// computer by default, a machine's group in the sidebar for one of its own, and with it that
// machine's harnesses, its projects, and `session/new` over its own connection. The shell and the
// machine are stand-ins for the app's own (`src-tauri/src/machines.rs`) and the mock's ACP.
//
// Everything attached here is process-global (the shell transport, the machines module's links,
// the store's machine map), so the file resets all of it: another test file's engine must never
// see these machines.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import {
  type Machine,
  machineConnection,
  refreshMachines,
  refreshMachineThreads,
  resetMachinesForTests as resetMachineLinksForTests,
  useMachines,
} from "./machines";
import {
  connectionFor,
  createSession,
  harnessesOn,
  newThreadChoice,
  openThread,
  resetForTests,
  setNewThreadHost,
  startMachineRouting,
  startNewThread,
  useApp,
} from "./store";
import { LOCAL_HOST } from "./threadKeys";

const STUDIO = "gw-8e1f4b7a5c2d4e6f";
const STUDIO_CWD = "/Users/lars/code/rustcode";
const LOCAL_CWD = "/Users/you/code/rustcode";

function studio(state: Machine["state"] = "connected"): Machine {
  return {
    id: STUDIO,
    name: "Studio",
    platform: "macos",
    main: false,
    routes: [{ kind: "lan", address: "192.168.1.20:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.20:17879", encrypted: false },
    state,
    detail: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: "2026-11-08T09:00:00+00:00",
  };
}

/** The app's shell, answering what a machine's supervisor needs, without one. */
class FakeShell implements Transport {
  readonly name = "mock";
  machines: Machine[] = [studio()];

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

async function until(check: () => boolean, ms = 1500): Promise<void> {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("timed out");
    await sleep(10);
  }
}

let shell: FakeShell | null = null;

beforeAll(async () => {
  // Nothing a previous file left behind: its machines, its shell, its engine.
  resetForTests();
  shell = new FakeShell();
  setShellTransport(shell);
  await refreshMachines();
  await startMachineRouting();
  // The machine answers `session/list` with the Studio's three threads once its channel is open.
  await until(() => !!useMachines.getState().sessions[STUDIO]);
});

afterAll(() => {
  resetForTests();
  resetMachineLinksForTests();
  setShellTransport(null);
  shell = null;
});

/** This computer's threads, as the sidebar and the folder list read them. */
function localThreads(): void {
  const now = Date.now();
  useApp.setState((s) => ({
    // This computer's threads beside the machine's, as the list holds both
    // (state/store.ts `mergeMachineLists`).
    sessions: [
      { id: "local-1", cwd: LOCAL_CWD, remote: "git@github.com:LHagfoss/rustcode.git", updatedAt: now },
      { id: "local-2", cwd: "/Users/you/code/website", updatedAt: now - 1000 },
      ...s.sessions.filter((session) => session.host),
    ],
    projectCwd: LOCAL_CWD,
    newThreadHost: LOCAL_HOST,
  }));
}

describe("the default Run on", () => {
  test("a new thread starts on this computer, whatever a machine's group was used for", async () => {
    localThreads();
    await setNewThreadHost(STUDIO);
    expect(useApp.getState().newThreadHost).toBe(STUDIO);

    // ⌘N with nothing picked: this computer, and the same project found back here.
    startNewThread();
    expect(useApp.getState().newThreadHost).toBe(LOCAL_HOST);
    expect(useApp.getState().projectCwd).toBe(LOCAL_CWD);
  });

  test("a machine's folder starts the thread on that machine", () => {
    localThreads();
    startNewThread(STUDIO_CWD, STUDIO);
    const s = useApp.getState();
    expect(s.newThreadHost).toBe(STUDIO);
    expect(s.projectCwd).toBe(STUDIO_CWD);
    // A folder only another machine has names that machine, even without one given.
    startNewThread(STUDIO_CWD);
    expect(useApp.getState().newThreadHost).toBe(STUDIO);
  });
});

describe("Run on a machine", () => {
  test("carries the project over by git remote, with that machine's harnesses", async () => {
    localThreads();
    await setNewThreadHost(STUDIO);
    const s = useApp.getState();
    // The Studio's folder is a different path for the same repository, and it is chosen.
    expect(s.projectCwd).toBe(STUDIO_CWD);
    // Its own harnesses and default: the Studio runs Claude Code, not this Mac's built-in one.
    const listed = harnessesOn(s, STUDIO);
    expect(listed?.harnesses.map((h) => h.id).sort()).toEqual(["claude-code", "codex"]);
    expect(listed?.default.harness).toBe("claude-code");
    expect(newThreadChoice(s)?.harness).toBe("claude-code");
    // This computer's engine is not the one that answered.
    expect(harnessesOn(s, LOCAL_HOST)).toBeNull();
  });

  test("a machine's projects are what its folder menu offers", async () => {
    await setNewThreadHost(STUDIO);
    await until(() => useMachines.getState().sessions[STUDIO]!.length > 0);
    const paths = new Set(useMachines.getState().sessions[STUDIO]!.map((session) => session.cwd));
    expect(paths).toContain(STUDIO_CWD);
    expect(paths).not.toContain(LOCAL_CWD);
  });
});

describe("a thread started on a machine", () => {
  test("session/new goes over that machine's connection, and it lists the thread", async () => {
    expect(connectionFor(LOCAL_HOST)).toBeNull();
    const studioConn = machineConnection(STUDIO);
    expect(studioConn).not.toBeNull();

    const key = await createSession({
      cwd: STUDIO_CWD,
      host: STUDIO,
      harness: "claude-code",
      title: "Run the tests",
      select: true,
    });
    expect(key).toBe(`${STUDIO}/mock-studio-new-1`);
    expect(connectionFor(STUDIO)).toBe(studioConn);
    expect(connectionFor(LOCAL_HOST)).toBeNull();
    const listed = useApp.getState().sessions.find((session) => session.id === key);
    expect(listed?.host).toBe(STUDIO);
    expect(useApp.getState().selectedId).toBe(key);

    // The machine's own thread list has it: the call landed there, not in a local engine.
    refreshMachineThreads();
    await until(() =>
      (useMachines.getState().sessions[STUDIO] ?? []).some((s) => s.id === "mock-studio-new-1"),
    );
  });

  test("reopens on its own machine", async () => {
    const key = `${STUDIO}/mock-studio-1`;
    await openThread(key);
    const thread = useApp.getState().threads[key];
    expect(thread?.openFailed).toBeUndefined();
    expect(thread?.loaded).toBe(true);
    expect(thread?.items.length ?? 0).toBeGreaterThan(0);
  });
});
