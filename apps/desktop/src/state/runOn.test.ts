// **Run on**: the computer a new thread starts on (docs/desktop.md › Several machines). This
// computer by default, a machine's group in the sidebar for one of its own, and with it that
// machine's harnesses, its projects, and `session/new` over its own connection. The shell and the
// machine are stand-ins for the app's own (`src-tauri/src/machines.rs`) and the mock's ACP.
//
// Everything attached here is process-global (the shell transport, the machines module's links,
// the store's machine map), so the file resets all of it: another test file's engine must never
// see these machines.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { AcpConnection } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import { fallbackHarnessSnapshot, type HarnessSnapshot } from "../acp/types";
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
  loadMachineHarnesses,
  newThreadChoice,
  openThread,
  renameThread,
  resetForTests,
  runCommandAction,
  setNewThreadHost,
  startMachineRouting,
  startNewThread,
  useApp,
  watchMachineConnection,
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
    hint: null,
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

  test("the sidebar's New thread passes the group's folder and its machine", () => {
    localThreads();
    // What ui/Sidebar.tsx does for a machine's project group (state/sidebarGroups.ts `project`).
    startNewThread(STUDIO_CWD, STUDIO);
    const s = useApp.getState();
    expect(s.newThreadHost).toBe(STUDIO);
    expect(s.projectCwd).toBe(STUDIO_CWD);
  });

  test("a folder alone is this computer's, even a path a machine also has", () => {
    localThreads();
    // The palette and "New Thread in Folder…" browse this disk: the folder never names a machine.
    startNewThread(STUDIO_CWD);
    expect(useApp.getState().newThreadHost).toBe(LOCAL_HOST);
    expect(useApp.getState().projectCwd).toBe(STUDIO_CWD);
  });

  test("the last screen's machine folder is not kept by a thread that starts here", () => {
    localThreads();
    // Composing on the Studio in a project this computer does not have, then ⌘N here.
    useApp.setState({
      selectedId: null,
      newThreadHost: STUDIO,
      projectCwd: "/Users/lars/code/notes",
    });
    startNewThread();
    expect(useApp.getState().newThreadHost).toBe(LOCAL_HOST);
    // Neither matched by remote nor trusted as this computer's: nothing, not the Studio's folder.
    expect(useApp.getState().projectCwd).toBe("");
  });

  test("a machine worktree folder from New Thread Here is not kept by a thread here", () => {
    localThreads();
    const worktree = "/Users/lars/.config/rustcode/worktrees/rustcode/feat-x";
    // "New Thread Here" on a Studio worktree thread, then ⌘N: a worktree is under no project
    // path, so the folder guard needs the screen's computer, not just the path.
    startNewThread(worktree, STUDIO);
    expect(useApp.getState().projectCwd).toBe(worktree);
    startNewThread();
    expect(useApp.getState().newThreadHost).toBe(LOCAL_HOST);
    expect(useApp.getState().projectCwd).toBe("");
  });

  test("a machine that does not answer rolls Run on back without its folder", async () => {
    localThreads();
    useApp.setState({ selectedId: null, newThreadHost: LOCAL_HOST, projectCwd: "/Users/lars/code/notes" });
    // No channel for this id: the harness read cannot answer, so the choice goes back.
    await setNewThreadHost("gw-away");
    const s = useApp.getState();
    expect(s.newThreadHost).toBe(LOCAL_HOST);
    expect(s.projectCwd).toBe("");
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

describe("a machine's own operations", () => {
  test("/new in a machine's thread starts there, in that thread's folder", () => {
    localThreads();
    const from = `${STUDIO}/mock-studio-1`;
    // The engine lists `/new` with the action the app answers itself (state/commands.ts).
    useApp.setState({
      baseCommands: [
        {
          name: "new",
          description: "New thread",
          takesArguments: false,
          report: false,
          action: "new_session",
        },
      ],
      sessionCommands: {},
    });
    expect(runCommandAction("new", from)).toBe(true);
    const s = useApp.getState();
    expect(s.newThreadHost).toBe(STUDIO);
    // The thread's own folder, not whatever new-thread screen was named last.
    expect(s.projectCwd).toBe(STUDIO_CWD);
  });

  test("a machine's thread is not renamed from here", async () => {
    const key = `${STUDIO}/mock-studio-1`;
    const before = useApp.getState().sessions.find((session) => session.id === key)?.title;
    expect(await renameThread(key, "Renamed on this Mac")).toBe(false);
    expect(useApp.getState().sessions.find((session) => session.id === key)?.title).toBe(before);
  });

  test("a machine's harness with no learned models keeps no model picked here", () => {
    const listed = useApp.getState().remoteHarnesses[STUDIO];
    expect(listed).toBeDefined();
    // `_rustcode/harnesses` lists a harness's learned models only (a harness never started there
    // has none), and its models call is not on the gateway: the machine's own default applies.
    const bare: HarnessSnapshot = {
      ...listed!,
      harnesses: listed!.harnesses.map((harness) =>
        harness.id === "claude-code" ? { ...harness, models: [] } : harness,
      ),
    };
    const choice = newThreadChoice({
      newThread: { harness: "claude-code", model: "opus-5.5", effort: "max" },
      harnesses: null,
      newThreadHost: STUDIO,
      remoteHarnesses: { [STUDIO]: bare },
    });
    expect(choice).toEqual({ harness: "claude-code", model: null, effort: null });
  });

  test("a machine with no projects starts no thread until it has one", async () => {
    localThreads();
    // A freshly paired machine has listed no thread, so it has no project to start in.
    useApp.setState((s) => ({ sessions: s.sessions.filter((session) => !session.host) }));
    await setNewThreadHost(STUDIO);
    expect(useApp.getState().newThreadHost).toBe(STUDIO);
    expect(useApp.getState().projectCwd).toBe("");
    // Its threads come back for the tests after this one.
    refreshMachineThreads();
    await until(() => useApp.getState().sessions.some((session) => session.host === STUDIO));
  });

  test("removing the machine Run on names goes back here and forgets its harnesses", async () => {
    localThreads();
    const gone = "gw-removed";
    useApp.setState((s) => ({
      newThreadHost: gone,
      projectCwd: "/tmp/elsewhere",
      remoteHarnesses: { ...s.remoteHarnesses, [gone]: fallbackHarnessSnapshot() },
    }));
    // The machines list changes: what `machines.ts` does when one is removed (state/store.ts `mirror`).
    useMachines.setState((s) => ({ machines: [...s.machines] }));
    await until(() => useApp.getState().newThreadHost === LOCAL_HOST);
    expect(useApp.getState().projectCwd).toBe("");
    expect(useApp.getState().remoteHarnesses[gone]).toBeUndefined();
  });

  test("a machine that reconnects is read again, not shown its list from before", async () => {
    const id = "gw-reconnect";
    const before = fallbackHarnessSnapshot();
    useApp.setState((s) => ({ remoteHarnesses: { ...s.remoteHarnesses, [id]: before } }));
    let connection: AcpConnection;
    connection = new AcpConnection((line) => {
      const frame = JSON.parse(line) as { id?: number; method?: string };
      const result =
        frame.method === "_rustcode/harnesses"
          ? {
              default: { harness: "codex" },
              harnesses: [
                { id: "codex", name: "Codex", enabled: true, known: true, installed: true, auth: "ok" },
              ],
            }
          : {};
      queueMicrotask(() => connection.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result })));
    });
    watchMachineConnection(id, connection);
    await until(() => useApp.getState().remoteHarnesses[id] !== before);
    expect(useApp.getState().remoteHarnesses[id]?.harnesses.map((h) => h.id)).toEqual(["codex"]);
    watchMachineConnection(id, null);
  });

  test("a machine on an older engine lists the builtin harness instead of refusing", async () => {
    const old = "gw-old-engine";
    useApp.setState((s) => ({ machineNames: { ...s.machineNames, [old]: "Old Mac" } }));
    let connection: AcpConnection;
    connection = new AcpConnection((line) => {
      const frame = JSON.parse(line) as { id?: number };
      queueMicrotask(() =>
        connection.receive(
          JSON.stringify({
            jsonrpc: "2.0",
            id: frame.id,
            error: { code: -32601, message: "Method not found" },
          }),
        ),
      );
    });
    watchMachineConnection(old, connection);
    const listed = await loadMachineHarnesses(old);
    expect(listed?.unsupported).toBe(true);
    // Run on stays on it, and the composer starts its builtin harness rather than rolling back.
    await setNewThreadHost(old);
    expect(useApp.getState().newThreadHost).toBe(old);
    watchMachineConnection(old, null);
  });
});
