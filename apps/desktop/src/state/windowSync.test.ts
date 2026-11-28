import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { STORAGE } from "./appState";
import { useFilesFollowOff } from "./configFilesFollow";
import type { InternetWait } from "./internetWait";
import type { LimitWait } from "./limitResume";
import { OUTBOX_KEY, type OutboxChange, useOutbox } from "./machineOutbox";
import { SCHEDULED_KEY } from "./schedule";
import { useSettingsFollow } from "./settingsFollow";
import {
  connect,
  continueDueScheduled,
  continueDueThreads,
  continueInternetThreads,
  openThread,
  resetForTests,
  useApp,
} from "./store";
import { emptyThread, isLoaded, isRunning } from "./thread";
import {
  applyStorageChange,
  applyWindowMessage,
  dismissResolvedPermission,
  isPrimaryWindow,
  primaryWindowChanged,
  setWindowRoleForTests,
  whenPrimaryWindow,
} from "./windowSync";

const until = async (check: () => boolean, ms = 4000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

beforeEach(() => {
  resetForTests();
  setWindowRoleForTests(null, "main");
});

afterEach(() => {
  resetForTests();
  setWindowRoleForTests(null, "main");
});

describe("windowSync", () => {
  test("syncs shared settings from storage events without disturbing window-local view", () => {
    useApp.setState({
      selectedId: "thread-a",
      view: { kind: "inbox" },
      theme: "light",
      pinned: [],
    });

    expect(applyStorageChange(STORAGE.theme, "dark")).toBe(true);
    expect(applyStorageChange(STORAGE.pinned, JSON.stringify(["thread-a", "thread-b"]))).toBe(true);
    expect(applyStorageChange(STORAGE.sidebar, "1")).toBe(false);

    const s = useApp.getState();
    expect(s.theme).toBe("dark");
    expect(s.pinned).toEqual(["thread-a", "thread-b"]);
    expect(s.selectedId).toBe("thread-a");
    expect(s.view).toEqual({ kind: "inbox" });
    expect(s.sidebarHidden).toBe(false);
  });

  test("dismisses permission prompts answered in another window", () => {
    const thread = {
      ...emptyThread("t1", "/repo"),
      permissions: [
        {
          rpcId: 42,
          sessionId: "t1",
          toolCallId: "call-1",
          title: "Run tests",
          options: [],
          diffs: [],
        },
        {
          rpcId: 43,
          sessionId: "t1",
          toolCallId: "call-2",
          title: "Write file",
          options: [],
          diffs: [],
        },
      ],
    };
    useApp.setState({ threads: { t1: thread } });

    expect(dismissResolvedPermission(42)).toBe(true);
    expect(useApp.getState().threads.t1?.permissions.map((p) => p.rpcId)).toEqual([43]);
    expect(dismissResolvedPermission(999)).toBe(false);
  });

  test("synchronizes live turn starts and finishes for threads open in another window", () => {
    useApp.setState({ threads: { t1: emptyThread("t1", "/repo") } });

    applyWindowMessage({
      type: "thread:turn-started",
      threadId: "t1",
      text: "Check status",
      at: 1000,
    });
    const during = useApp.getState().threads.t1!;
    expect(isRunning(during)).toBe(true);
    expect(during.items.some((item) => item.kind === "user" && item.text === "Check status")).toBe(true);

    applyWindowMessage({
      type: "thread:turn-finished",
      threadId: "t1",
      outcome: "completed",
      at: 2000,
    });
    const after = useApp.getState().threads.t1!;
    expect(isRunning(after)).toBe(false);
  });

  test("tracks primary window role for background side effects", () => {
    setWindowRoleForTests("main", "main");
    expect(isPrimaryWindow()).toBe(true);

    setWindowRoleForTests("window-2", "main");
    expect(isPrimaryWindow()).toBe(false);

    setWindowRoleForTests("window-2", "window-2");
    expect(isPrimaryWindow()).toBe(true);
  });

  test("starts background loops in the primary window only", () => {
    setWindowRoleForTests("main", "main");
    const started: string[] = [];
    whenPrimaryWindow(() => started.push("fleet"));
    expect(started).toEqual(["fleet"]);

    setWindowRoleForTests("window-2", "main");
    whenPrimaryWindow(() => started.push("follow"));
    whenPrimaryWindow(() => started.push("updates"));
    expect(started).toEqual(["fleet"]);

    primaryWindowChanged("main");
    expect(started).toEqual(["fleet"]);
    primaryWindowChanged("window-2");
    expect(started).toEqual(["fleet", "follow", "updates"]);
    primaryWindowChanged("window-2");
    expect(started).toEqual(["fleet", "follow", "updates"]);
  });

  test("reads follow settings and the machine outbox again when another window writes them", () => {
    const scope = globalThis as { localStorage?: Storage };
    const data = new Map<string, string>();
    scope.localStorage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
    } as unknown as Storage;
    try {
      const follow = JSON.stringify(["studio"]);
      data.set("rustcode.settingsFollow", follow);
      expect(applyStorageChange("rustcode.settingsFollow", follow)).toBe(true);
      expect(useSettingsFollow.getState().ids).toEqual(["studio"]);

      const off = JSON.stringify({ studio: ["themes"] });
      data.set("rustcode.settingsFollow.filesOff", off);
      expect(applyStorageChange("rustcode.settingsFollow.filesOff", off)).toBe(true);
      expect(useFilesFollowOff.getState().off).toEqual({ studio: ["themes"] });

      const change: OutboxChange = { kind: "pin", key: "studio\u0000t1", value: true, base: false, at: 1 };
      const outbox = JSON.stringify({ studio: [change] });
      data.set(OUTBOX_KEY, outbox);
      expect(applyStorageChange(OUTBOX_KEY, outbox)).toBe(true);
      expect(useOutbox.getState().byHost).toEqual({ studio: [change] });
    } finally {
      delete scope.localStorage;
      useSettingsFollow.setState({ ids: [], all: false });
      useFilesFollowOff.setState({ off: {} });
      useOutbox.setState({ byHost: {} });
    }
  });

  test("syncs scheduled messages and waits from storage events", () => {
    const due = Date.now() + 60_000;
    const rawSchedule = JSON.stringify({
      t1: [{ id: "sched-1", text: "follow up later", due }],
    });
    expect(applyStorageChange(SCHEDULED_KEY, rawSchedule)).toBe(true);
    expect(useApp.getState().scheduled).toEqual({
      t1: [{ id: "sched-1", text: "follow up later", due }],
    });

    const limitWait: LimitWait = {
      until: due,
      attempts: 1,
      harness: "claude-code",
      account: null,
      reason: "limit",
    };
    expect(applyStorageChange(STORAGE.limitWaits, JSON.stringify({ t1: limitWait }))).toBe(true);
    expect(useApp.getState().limitWaits).toEqual({ t1: limitWait });

    const internetWait: InternetWait = {
      since: Date.now() - 1_000,
      attempts: 0,
      harness: "claude-code",
      account: null,
    };
    expect(applyStorageChange(STORAGE.internetWaits, JSON.stringify({ t2: internetWait }))).toBe(true);
    expect(useApp.getState().internetWaits).toEqual({ t2: internetWait });

    expect(applyStorageChange(STORAGE.limitWaits, "[]")).toBe(true);
    expect(useApp.getState().limitWaits).toEqual({});
    expect(applyStorageChange(STORAGE.internetWaits, "null")).toBe(true);
    expect(useApp.getState().internetWaits).toEqual({});
  });

  test("treats a Tauri window as secondary until its role is known", () => {
    const scope = globalThis as unknown as { window?: Record<string, unknown> };
    const hadWindow = "window" in scope;
    const previousWindow = scope.window;
    const target: Record<string, unknown> = previousWindow ?? {};
    const hadInternals = "__TAURI_INTERNALS__" in target;
    const previousInternals = target.__TAURI_INTERNALS__;
    target.__TAURI_INTERNALS__ = {};
    scope.window = target;
    try {
      setWindowRoleForTests(null, "main", false);
      expect(isPrimaryWindow()).toBe(false);

      setWindowRoleForTests("window-2", "main", false);
      expect(isPrimaryWindow()).toBe(false);

      primaryWindowChanged("main");
      expect(isPrimaryWindow()).toBe(false);

      primaryWindowChanged("window-2");
      expect(isPrimaryWindow()).toBe(true);
    } finally {
      if (hadInternals) target.__TAURI_INTERNALS__ = previousInternals;
      else delete target.__TAURI_INTERNALS__;
      if (hadWindow) scope.window = previousWindow;
      else delete scope.window;
    }
  });

  test("hands scheduled sends and wait timers to a secondary window when it becomes primary", async () => {
    const scope = globalThis as { localStorage?: Storage };
    const data = new Map<string, string>();
    const due = Date.now() - 10_000;
    data.set(
      SCHEDULED_KEY,
      JSON.stringify({
        "mock-thread-1": [{ id: "sched-handoff", text: "handed off message", due }],
      }),
    );
    scope.localStorage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
    } as unknown as Storage;
    try {
      setWindowRoleForTests("window-2", "main");
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().engine.phase === "ready");
      await openThread("mock-thread-1");
      await until(() => isLoaded(useApp.getState().threads["mock-thread-1"]) === true);

      useApp.setState({
        limitWaits: {
          "mock-thread-2": {
            until: Date.now() - 10_000,
            attempts: 1,
            harness: "mock",
            account: null,
            reason: "limit",
          },
        },
        internetWaits: {
          "mock-thread-2": {
            since: Date.now() - 10_000,
            attempts: 0,
            harness: "mock",
            account: null,
          },
        },
      });

      await continueDueScheduled();
      await continueDueThreads();
      await continueInternetThreads();
      expect(
        useApp
          .getState()
          .threads["mock-thread-1"]?.items.some(
            (item) => item.kind === "user" && item.text.includes("handed off message"),
          ),
      ).toBe(false);
      expect(useApp.getState().limitWaits["mock-thread-2"]).toBeDefined();

      primaryWindowChanged("window-2");

      await until(
        () =>
          !!useApp
            .getState()
            .threads["mock-thread-1"]?.items.some(
              (item) => item.kind === "user" && item.text.includes("handed off message"),
            ),
      );
      expect(useApp.getState().scheduled["mock-thread-1"]).toBeUndefined();
      await until(() => useApp.getState().limitWaits["mock-thread-2"] === undefined);
    } finally {
      delete scope.localStorage;
    }
  });
});
