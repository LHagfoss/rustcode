import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { ACP_CAPS } from "../acp/mockHarnesses";
import { MockMachine } from "../acp/mockMachine";
import { MOCK_MACHINE, MOCK_STUDIO } from "../acp/mockMachineFixtures";
import { MockTransport } from "../acp/mockTransport";
import type { HarnessSnapshot, SessionSummary } from "../acp/types";
import { setShellTransport } from "../lib/shell";
import { mergeMachineLists } from "./machineSync";
import { type Machine, useMachines } from "./machines";
import { sidebarProjects } from "./sidebarGroups";
import {
  connect,
  keptUnlistedThreads,
  moveThread,
  openThread,
  refreshSessions,
  resetForTests,
  selectThread,
  setDraft,
  togglePin,
  useApp,
  watchMachineConnection,
} from "./store";
import {
  moveMachineBlockedReason,
  moveThreadToMachine,
  planMachineMoveDestination,
  rememberedMachineOrigin,
  rememberMachineOrigin,
  resetMoveMachineForTests,
  resolveTargetHarness,
} from "./threadMoveMachine";

function pairConnection(machine: MockMachine): AcpConnection {
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    queueMicrotask(() => {
      for (const frame of machine.receive(line)) conn.receive(frame);
    });
  });
  return conn;
}

function fakeMachine(id: string, name: string, state: Machine["state"] = "connected"): Machine {
  return {
    id,
    name,
    platform: id === MOCK_STUDIO.id ? "macos" : "linux",
    version: "1.28.0",
    routes: [{ kind: "lan", address: "192.168.1.20:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.20:17879", encrypted: false },
    state,
    detail: null,
    hint: null,
    retryInSecs: null,
    addedAt: "2026-11-01T00:00:00Z",
    lastConnectedAt: "2026-11-21T00:00:00Z",
  };
}

describe("planMachineMoveDestination", () => {
  afterEach(() => resetMoveMachineForTests());

  const sessions: SessionSummary[] = [
    {
      id: "t-local",
      cwd: "/Users/lucas/code/rustcode",
      remote: "git@github.com:LHagfoss/rustcode.git",
      updatedAt: 100,
    },
    {
      id: "gw-vps/t-vps",
      host: "gw-vps",
      cwd: "/home/lucas/repos/rc-checkout",
      remote: "https://github.com/lhagfoss/rustcode",
      updatedAt: 200,
    },
    {
      id: "gw-vps/t-other",
      host: "gw-vps",
      cwd: "/home/lucas/notes",
      updatedAt: 50,
    },
  ];

  test("matches destination checkout by git remote even when folder names differ", () => {
    const dest = planMachineMoveDestination({
      sessionId: "t-local",
      fromHost: "",
      fromCwd: "/Users/lucas/code/rustcode",
      toHost: "gw-vps",
      sessions,
    });
    expect(dest).toEqual({
      suggestedCwd: "/home/lucas/repos/rc-checkout",
      displayFolder: "rc-checkout",
      returning: false,
    });
  });

  test("prioritizes remembered origin folder when moving back to a previously visited machine", () => {
    rememberMachineOrigin("t-local", "", "/Users/lucas/code/worktrees/rustcode-feature");
    const dest = planMachineMoveDestination({
      sessionId: "t-local",
      fromHost: "gw-vps",
      fromCwd: "/home/lucas/repos/rc-checkout",
      toHost: "",
      sessions,
    });
    expect(dest).toEqual({
      suggestedCwd: "/Users/lucas/code/worktrees/rustcode-feature",
      displayFolder: "rustcode-feature",
      returning: true,
    });
  });

  test("falls back to Home folder when the target machine has no matching project", () => {
    const dest = planMachineMoveDestination({
      sessionId: "t-unmatched",
      fromHost: "",
      fromCwd: "/Users/lucas/code/brand-new-app",
      toHost: "gw-vps",
      sessions,
    });
    expect(dest).toEqual({
      suggestedCwd: undefined,
      displayFolder: "Home folder",
      returning: false,
    });
  });
});

describe("resolveTargetHarness", () => {
  const snapshot: HarnessSnapshot = {
    harnesses: [
      {
        id: "rustcode",
        name: "RustCode",
        kind: "builtin",
        known: true,
        command: [],
        builtin: true,
        enabled: true,
        installed: true,
        auth: "ok",
        models: [],
        capabilities: { ...ACP_CAPS, rewind: true },
      },
      {
        id: "codex",
        name: "Codex",
        kind: "codex",
        known: true,
        command: ["codex"],
        builtin: false,
        enabled: true,
        installed: true,
        auth: "ok",
        models: [{ id: "gpt-5.5-codex", name: "GPT-5.5 Codex", efforts: ["medium", "high"] }],
        accounts: [{ id: "work", name: "Work", auth: "ok", config_dir: "/tmp/codex-work" }],
        capabilities: ACP_CAPS,
      },
    ],
    default: { harness: "codex", model: "gpt-5.5-codex", effort: "high" },
    onboarded: true,
    remembers: true,
  };

  test("keeps the same harness and matching account when available on destination", () => {
    expect(
      resolveTargetHarness({
        sourceHarness: "codex",
        sourceAccount: "work",
        sourceModel: "gpt-5.5-codex",
        sourceEffort: "high",
        targetSnapshot: snapshot,
      }),
    ).toEqual({
      harness: "codex",
      account: "work",
      model: "gpt-5.5-codex",
      effort: "high",
      harnessChanged: false,
    });
  });

  test("falls back to destination default when source harness is absent on destination", () => {
    expect(
      resolveTargetHarness({
        sourceHarness: "claude-code",
        sourceAccount: "personal",
        sourceModel: "sonnet-5.5",
        sourceEffort: "high",
        targetSnapshot: snapshot,
      }),
    ).toEqual({
      harness: "codex",
      account: "work",
      model: "gpt-5.5-codex",
      effort: "high",
      harnessChanged: true,
    });
  });
});

describe("moveMachineBlockedReason", () => {
  test("reports running turn, offline source machine, and moving state", () => {
    expect(moveMachineBlockedReason({ running: true, sourceHost: "", sourceConnected: true })).toContain(
      "Stop the running turn",
    );
    expect(
      moveMachineBlockedReason({
        running: false,
        sourceHost: "gw-vps",
        sourceConnected: false,
        sourceMachineName: "VPS",
      }),
    ).toContain("VPS is offline");
    expect(
      moveMachineBlockedReason({ running: false, sourceHost: "", sourceConnected: true, moving: true }),
    ).toContain("Moving");
    expect(
      moveMachineBlockedReason({ running: false, sourceHost: "gw-vps", sourceConnected: true }),
    ).toBeUndefined();
  });
});

describe("moveThreadToMachine end-to-end", () => {
  let studioConn: AcpConnection;
  const STUDIO_ID = "gw-test-studio";
  const LINUX_ID = "gw-test-linux";
  let linuxConn: AcpConnection;

  beforeEach(async () => {
    resetForTests();
    resetMoveMachineForTests();
    await connect(new MockTransport());
    // Wait for initial `initialize` + `session/list` to finish, then disconnect the mock shell
    // transport so its built-in machine supervisor doesn't overwrite our test connections.
    for (let i = 0; i < 40 && useApp.getState().sessions.length === 0; i++) {
      await new Promise((r) => setTimeout(r, 5));
    }
    setShellTransport(null);
    await refreshSessions({ quiet: true });
    const studio = new MockMachine(MOCK_STUDIO.name);
    const linux = new MockMachine(MOCK_MACHINE.name);
    studioConn = pairConnection(studio);
    linuxConn = pairConnection(linux);
    useMachines.setState({
      machines: [fakeMachine(STUDIO_ID, MOCK_STUDIO.name), fakeMachine(LINUX_ID, MOCK_MACHINE.name)],
      sessions: {
        [STUDIO_ID]: [
          {
            id: "mock-studio-1",
            cwd: "/Users/lars/code/rustcode",
            remote: "git@github.com:lhagfoss/rustcode.git",
            title: "Ship the mock window shots",
            harness: "claude-code",
            updatedAt: Date.now(),
          },
        ],
        [LINUX_ID]: [
          {
            id: "mock-linux-1",
            cwd: "/home/lars/code/rustcode",
            remote: "git@github.com:lhagfoss/rustcode.git",
            title: "Run the tests",
            harness: "codex",
            updatedAt: Date.now(),
          },
        ],
      },
    });
    useApp.setState((s) => ({
      machineNames: { [STUDIO_ID]: MOCK_STUDIO.name, [LINUX_ID]: MOCK_MACHINE.name },
      sessions: [
        ...s.sessions,
        {
          id: `${STUDIO_ID}/mock-studio-1`,
          host: STUDIO_ID,
          cwd: "/Users/lars/code/rustcode",
          remote: "git@github.com:lhagfoss/rustcode.git",
          title: "Ship the mock window shots",
          harness: "claude-code",
          updatedAt: Date.now(),
        },
        {
          id: `${LINUX_ID}/mock-linux-1`,
          host: LINUX_ID,
          cwd: "/home/lars/code/rustcode",
          remote: "git@github.com:lhagfoss/rustcode.git",
          title: "Run the tests",
          harness: "codex",
          updatedAt: Date.now(),
        },
      ],
    }));
    watchMachineConnection(STUDIO_ID, studioConn);
    watchMachineConnection(LINUX_ID, linuxConn);
  });

  afterEach(() => {
    resetMoveMachineForTests();
    resetForTests();
  });

  test("moves a local thread to Studio and right back to this computer, preserving draft, pin, history and folder", async () => {
    const localSession = useApp.getState().sessions.find((s) => !s.host);
    expect(localSession).toBeDefined();
    const origId = localSession!.id;
    const origCwd = localSession!.cwd;

    selectThread(origId);
    await openThread(origId);
    setDraft(origId, "continue after moving");
    togglePin(origId);

    // Move Local -> Studio
    const onStudio = await moveThreadToMachine(origId, STUDIO_ID);
    expect(onStudio).toBe(`${STUDIO_ID}/${origId}`);
    expect(useApp.getState().selectedId).toBe(onStudio);
    expect(useApp.getState().drafts[onStudio!]).toBe("continue after moving");
    expect(useApp.getState().drafts[origId]).toBeUndefined();
    expect(useApp.getState().pinned).toContain(onStudio!);
    expect(useApp.getState().pinned).not.toContain(origId);
    expect(useApp.getState().sessions.some((s) => s.id === origId)).toBe(false);
    expect(useApp.getState().sessions.some((s) => s.id === onStudio)).toBe(true);
    expect(rememberedMachineOrigin(origId, "")).toBe(origCwd);

    // Transcript on Studio carries the move divider.
    const studioItems = useApp.getState().threads[onStudio!]?.items ?? [];
    expect(studioItems.some((item) => item.kind === "handoff" && item.toMachine === MOCK_STUDIO.name)).toBe(
      true,
    );

    // Move right back: Studio -> This computer (clears tombstone & returns to origCwd!).
    const backLocal = await moveThreadToMachine(onStudio!, "");
    expect(backLocal).toBe(origId);
    expect(useApp.getState().selectedId).toBe(origId);
    expect(useApp.getState().drafts[origId]).toBe("continue after moving");
    expect(useApp.getState().pinned).toContain(origId);
    expect(useApp.getState().sessions.find((s) => s.id === origId)?.cwd).toBe(origCwd);
    expect(useApp.getState().sessions.some((s) => s.id === onStudio)).toBe(false);
    const localItems = useApp.getState().threads[origId]?.items ?? [];
    expect(localItems.some((item) => item.kind === "handoff" && item.fromMachine === MOCK_STUDIO.name)).toBe(
      true,
    );
  });

  test("moves directly between two paired machines (Studio -> Linux) with harness fallback", async () => {
    const studioKey = `${STUDIO_ID}/mock-studio-1`;
    selectThread(studioKey);
    await openThread(studioKey);

    // Studio runs claude-code; Linux only runs codex & rustcode -> smart fallback to codex!
    const onLinux = await moveThreadToMachine(studioKey, LINUX_ID);
    expect(onLinux).toBe(`${LINUX_ID}/mock-studio-1`);
    const moved = useApp.getState().sessions.find((s) => s.id === onLinux);
    expect(moved?.cwd).toBe("/home/lars/code/rustcode");
    expect(moved?.harness).toBe("codex");
    expect(useApp.getState().sessions.some((s) => s.id === studioKey)).toBe(false);
  });

  test("refuses to move when the destination machine is offline and leaves source untouched", async () => {
    const localSession = useApp.getState().sessions.find((s) => !s.host)!;
    watchMachineConnection(STUDIO_ID, null);

    const moved = await moveThreadToMachine(localSession.id, STUDIO_ID);
    expect(moved).toBeNull();
    expect(useApp.getState().sessions.some((s) => s.id === localSession.id)).toBe(true);
  });

  test("handles multi-chunk export and import transparently", async () => {
    const chunkedHost = "gw-chunked";
    const exportedPayload = btoa(
      JSON.stringify({
        version: 1,
        sessionId: "chunked-1",
        title: "Huge thread",
        cwd: "/remote/rustcode",
        harness: "codex",
        history: [],
        origins: {},
      }),
    );
    const half = Math.floor(exportedPayload.length / 2);
    const receivedChunks: string[] = [];
    let removed = false;

    let srcConn!: AcpConnection;
    srcConn = new AcpConnection((line) => {
      const msg = JSON.parse(line) as JsonObject;
      const id = msg.id as number;
      const params = obj(msg.params) ?? {};
      queueMicrotask(() => {
        if (msg.method === "_rustcode/session/export") {
          const offset = Number(params.offset ?? 0);
          if (offset === 0) {
            srcConn.receive(
              JSON.stringify({
                jsonrpc: "2.0",
                id,
                result: {
                  exportId: "exp-1",
                  offset: 0,
                  nextOffset: half,
                  totalBytes: exportedPayload.length,
                  data: exportedPayload.slice(0, half),
                  done: false,
                },
              }),
            );
          } else {
            srcConn.receive(
              JSON.stringify({
                jsonrpc: "2.0",
                id,
                result: {
                  exportId: "exp-1",
                  offset: half,
                  nextOffset: exportedPayload.length,
                  totalBytes: exportedPayload.length,
                  data: exportedPayload.slice(half),
                  done: true,
                },
              }),
            );
          }
          return;
        }
        if (msg.method === "_rustcode/session/remove") {
          removed = true;
          srcConn.receive(
            JSON.stringify({ jsonrpc: "2.0", id, result: { sessionId: "chunked-1", removed: true } }),
          );
        }
      });
    });

    const dstHost = "gw-chunked-dst";
    let dstConn!: AcpConnection;
    dstConn = new AcpConnection((line) => {
      const msg = JSON.parse(line) as JsonObject;
      const id = msg.id as number;
      const params = obj(msg.params) ?? {};
      queueMicrotask(() => {
        if (msg.method === "_rustcode/harnesses") {
          dstConn.receive(
            JSON.stringify({
              jsonrpc: "2.0",
              id,
              result: {
                harnesses: [],
                default: { harness: "rustcode", model: null, effort: null },
                onboarded: true,
                remembers: true,
              },
            }),
          );
          return;
        }
        if (msg.method === "_rustcode/session/import") {
          receivedChunks.push(str(params.data) ?? "");
          if (!params.done) {
            dstConn.receive(
              JSON.stringify({
                jsonrpc: "2.0",
                id,
                result: { uploadId: "imp-1", receivedBytes: receivedChunks[0]!.length, done: false },
              }),
            );
          } else {
            dstConn.receive(
              JSON.stringify({
                jsonrpc: "2.0",
                id,
                result: {
                  sessionId: "chunked-1",
                  cwd: "/dst/rustcode",
                  title: "Huge thread",
                  harness: "codex",
                  harnessChanged: false,
                  resumedNative: false,
                } as Json,
              }),
            );
          }
        }
      });
    });

    watchMachineConnection(chunkedHost, srcConn);
    watchMachineConnection(dstHost, dstConn);
    useApp.setState((s) => ({
      machineNames: { ...s.machineNames, [chunkedHost]: "Src", [dstHost]: "Dst" },
      sessions: [
        ...s.sessions,
        {
          id: `${chunkedHost}/chunked-1`,
          host: chunkedHost,
          cwd: "/remote/rustcode",
          title: "Huge thread",
          updatedAt: Date.now(),
        },
      ],
    }));

    const movedKey = await moveThreadToMachine(`${chunkedHost}/chunked-1`, dstHost);
    expect(movedKey).toBe(`${dstHost}/chunked-1`);
    expect(receivedChunks.length).toBe(2);
    expect(receivedChunks.join("")).toBe(exportedPayload);
    expect(removed).toBe(true);
  });

  test("changing folder after moving a thread to a paired machine updates the sidebar and survives machine list syncs", async () => {
    const localSession = useApp.getState().sessions.find((s) => !s.host)!;
    const origId = localSession.id;
    selectThread(origId);
    await openThread(origId);

    // Move Local -> Linux (initially lands in /home/lars/code/rustcode), then move its folder.
    const onLinux = await moveThreadToMachine(origId, LINUX_ID);
    expect(onLinux).toBe(`${LINUX_ID}/${origId}`);
    expect(keptUnlistedThreads.get(onLinux!)?.summary.cwd).toBe("/home/lars/code/rustcode");

    const nextCwd = "/home/lars/code/notes-app";
    expect(await moveThread(onLinux!, nextCwd)).toBe(true);

    // Both the app store, the machine cache, the unlisted guard and origin memory reflect the new folder.
    expect(useApp.getState().threads[onLinux!]?.cwd).toBe(nextCwd);
    expect(useApp.getState().sessions.find((s) => s.id === onLinux)?.cwd).toBe(nextCwd);
    expect(useMachines.getState().sessions[LINUX_ID]?.find((s) => s.id === origId)?.cwd).toBe(nextCwd);
    expect(keptUnlistedThreads.get(onLinux!)?.summary.cwd).toBe(nextCwd);
    expect(rememberedMachineOrigin(origId, LINUX_ID)).toBe(nextCwd);

    // Any later machine mirror / mergeMachineLists must keep the new folder in the sidebar.
    mergeMachineLists(useMachines.getState().sessions);
    expect(useApp.getState().sessions.find((s) => s.id === onLinux)?.cwd).toBe(nextCwd);
    const groups = sidebarProjects(useApp.getState().sessions);
    expect(groups.find((g) => g.label === "notes-app")?.items.some((s) => s.id === onLinux)).toBe(true);
    expect(groups.find((g) => g.label === "rustcode")?.items.some((s) => s.id === onLinux)).toBeFalsy();

    // Even if a stale machine list omits the thread and falls back to keptUnlisted, it keeps the new folder.
    mergeMachineLists({
      ...useMachines.getState().sessions,
      [LINUX_ID]: (useMachines.getState().sessions[LINUX_ID] ?? []).filter((s) => s.id !== origId),
    });
    expect(useApp.getState().sessions.find((s) => s.id === onLinux)?.cwd).toBe(nextCwd);
  });
});
