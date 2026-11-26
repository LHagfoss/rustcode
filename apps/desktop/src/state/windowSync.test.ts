import { beforeEach, describe, expect, test } from "bun:test";
import { STORAGE } from "./appState";
import { useFilesFollowOff } from "./configFilesFollow";
import { OUTBOX_KEY, type OutboxChange, useOutbox } from "./machineOutbox";
import { useSettingsFollow } from "./settingsFollow";
import { resetForTests, useApp } from "./store";
import { emptyThread, isRunning } from "./thread";
import {
  applyStorageChange,
  applyWindowMessage,
  dismissResolvedPermission,
  isPrimaryWindow,
  primaryWindowChanged,
  setWindowRoleForTests,
  whenPrimaryWindow,
} from "./windowSync";

beforeEach(() => {
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
      useSettingsFollow.setState({ ids: [] });
      useFilesFollowOff.setState({ off: {} });
      useOutbox.setState({ byHost: {} });
    }
  });
});
