import { beforeEach, describe, expect, test } from "bun:test";
import { STORAGE } from "./appState";
import { resetForTests, useApp } from "./store";
import { emptyThread, isRunning } from "./thread";
import {
  applyStorageChange,
  applyWindowMessage,
  dismissResolvedPermission,
  isPrimaryWindow,
  setWindowRoleForTests,
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
});
