// Rows put away from "Needs you": the id one is remembered by, and the list the
// derivation leaves them out of. A dismissed row comes back only when its thread
// asks again.

import { describe, expect, test } from "bun:test";
import type { PermissionRequest, SessionSummary } from "../acp/types";
import { type DecidedItem, deriveInbox, foldDecided, type InboxItem, showsNeedsYou } from "./inbox";
import { DISMISSALS_MAX, dismissRows, isDismissed, restoreRows } from "./inboxDismissed";
import type { Link } from "./orchestration";
import { dismissInbox, useApp } from "./store";

const NOW = 1_700_000_000_000;

const sessions: SessionSummary[] = [
  { id: "one", title: "Fix the composer", cwd: "/code/app" },
  { id: "child", title: "The delegated one", cwd: "/code/app" },
];

function request(sessionId: string, rpcId: number, at: number): PermissionRequest {
  return {
    rpcId,
    sessionId,
    toolCallId: `call-${rpcId}`,
    title: "bash",
    options: [
      { id: "allow", name: "Allow", kind: "allow_once" },
      { id: "reject", name: "Reject", kind: "reject_once" },
    ],
    diffs: [],
    at,
  };
}

function link(overrides: Partial<Link> = {}): Link {
  return {
    id: "parent/1",
    orchestrator: "parent",
    target: "child",
    orchestratorTitle: "Parent",
    targetTitle: "Child",
    message: "Do the thing",
    state: "needs_user",
    createdAt: NOW - 5_000,
    ...overrides,
  };
}

/** The inbox as the app derives it, with the rows `dismissed` puts away. */
function read(
  dismissed: Record<string, number>,
  overrides: Partial<Parameters<typeof deriveInbox>[0]> = {},
): InboxItem[] {
  return deriveInbox(
    {
      sessions,
      threads: { one: { permissions: [request("one", 1, NOW)] } },
      failed: {},
      links: [],
      dismissed,
      ...overrides,
    },
    NOW,
  );
}

describe("dismissing a row", () => {
  test("takes the row out of the list, the sidebar count with it", () => {
    const [row] = read({});
    expect(row).toBeDefined();
    const rows = dismissRows({}, [row!], NOW + 1_000);
    expect(rows).toEqual({ [row!.key]: NOW + 1_000 });
    expect(read(rows)).toEqual([]);
    expect(showsNeedsYou(read(rows))).toBe(false);
  });

  test("keeps the thread's next ask: another request is a row of its own", () => {
    const [asked] = read({});
    const dismissed = dismissRows({}, [asked!], NOW);
    // The same thread asks again with a request the app had not seen.
    const again = read(dismissed, {
      threads: { one: { permissions: [request("one", 1, NOW), request("one", 2, NOW + 5_000)] } },
    });
    expect(again.map((item) => item.key)).toEqual(["one:request:2"]);
  });

  test("a failure that comes back asks again, the same failure does not", () => {
    const [failed] = read({}, { threads: {}, failed: { one: NOW } });
    expect(failed?.kind).toBe("failed");
    const dismissed = dismissRows({}, [failed!], NOW);
    // The same failure, whether the app knows it from the store or from the transcript: the
    // transcript's own moment is the same or earlier, so it stays away.
    expect(read(dismissed, { threads: {}, failed: { one: NOW } })).toEqual([]);
    expect(
      read(dismissed, {
        threads: { one: { permissions: [], items: [{ kind: "turnEnd", outcome: "failed", at: NOW - 500 }] } },
      }),
    ).toEqual([]);
    // The turn failed again later: the row arrives after it was put away, so it shows again.
    const later = read(dismissed, { threads: {}, failed: { one: NOW + 60_000 } });
    expect(later.map((item) => item.kind)).toEqual(["failed"]);
  });

  test("dismissing all takes every row at once, and undo puts them back", () => {
    const rows = read({}, { failed: { one: NOW + 1_000 }, links: [link()] });
    expect(rows).toHaveLength(3);
    const dismissed = dismissRows({}, rows, NOW + 2_000);
    expect(read(dismissed, { failed: { one: NOW + 1_000 }, links: [link()] })).toEqual([]);
    const back = restoreRows(
      dismissed,
      rows.map((item) => item.key),
    );
    expect(read(back, { failed: { one: NOW + 1_000 }, links: [link()] })).toHaveLength(3);
  });

  test("a row that comes back from Undo is not decided any more", () => {
    const [row] = read({});
    const dismissed = foldDecided([row!], [], [], { now: NOW, labels: { [row!.key]: "Dismissed" } });
    expect(dismissed.map((item: DecidedItem) => item.label)).toEqual(["Dismissed"]);
    expect(foldDecided([], [row!], dismissed)).toEqual([]);
  });

  test("restoring a key that was not dismissed changes nothing", () => {
    const dismissed = dismissRows({}, read({}), NOW);
    expect(restoreRows(dismissed, ["nothing:request:9"])).toBe(dismissed);
  });

  test("keeps the newest keys and drops the oldest past the cap", () => {
    const rows = Array.from({ length: DISMISSALS_MAX + 5 }, (_, i) => ({
      kind: "approval" as const,
      key: `one:request:${i}`,
      at: NOW,
    }));
    let dismissed: Record<string, number> = {};
    // One at a time, so each has its own moment and the oldest are known.
    rows.forEach((row, i) => {
      dismissed = dismissRows(dismissed, [row], NOW + i);
    });
    expect(Object.keys(dismissed)).toHaveLength(DISMISSALS_MAX);
    expect(isDismissed(dismissed, rows[0]!)).toBe(false);
    expect(isDismissed(dismissed, rows[4]!)).toBe(false);
    expect(isDismissed(dismissed, rows[5]!)).toBe(true);
    expect(isDismissed(dismissed, rows[rows.length - 1]!)).toBe(true);
  });
});

/** The app's storage in a test (there is no window): a map `store()` writes into. */
function testStorage() {
  const data = new Map<string, string>();
  const scope = globalThis as unknown as Record<string, unknown>;
  scope.localStorage = {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  } as unknown as Storage;
  return { data, done: () => delete scope.localStorage };
}

describe("the inbox's dismiss action", () => {
  const row: InboxItem = {
    key: "one:request:1",
    kind: "approval",
    threadId: "one",
    title: "Fix the composer",
    project: "/code/app",
    detail: "Wants to bash",
    at: NOW,
    pending: 1,
    number: 1,
  };

  test("drops the row at once, keeps it, and its Undo puts it back", () => {
    const storage = testStorage();
    useApp.setState({ inboxDismissed: {} });
    try {
      dismissInbox([row]);
      expect(Object.keys(useApp.getState().inboxDismissed)).toEqual([row.key]);
      expect(JSON.parse(storage.data.get("rustcode.inboxDismissed") ?? "{}")).toEqual(
        useApp.getState().inboxDismissed,
      );
      const card = useApp.getState().toasts[0];
      expect(card?.text).toBe('Dismissed "Fix the composer"');
      expect(card?.action?.label).toBe("Undo");
      card?.action?.run();
      expect(useApp.getState().inboxDismissed).toEqual({});
      expect(JSON.parse(storage.data.get("rustcode.inboxDismissed") ?? "{}")).toEqual({});
    } finally {
      storage.done();
      useApp.setState({ inboxDismissed: {}, toasts: [] });
    }
  });

  test("dismissing every row is one Undo", () => {
    const storage = testStorage();
    useApp.setState({ inboxDismissed: {} });
    try {
      const second: InboxItem = { ...row, key: "one:failed", kind: "failed", at: NOW + 5_000 };
      dismissInbox([row, second]);
      expect(Object.keys(useApp.getState().inboxDismissed)).toHaveLength(2);
      expect(useApp.getState().toasts[0]?.text).toBe("Dismissed 2 items");
      useApp.getState().toasts[0]?.action?.run();
      expect(useApp.getState().inboxDismissed).toEqual({});
    } finally {
      storage.done();
      useApp.setState({ inboxDismissed: {}, toasts: [] });
    }
  });
});
