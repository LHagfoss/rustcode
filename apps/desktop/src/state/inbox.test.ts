import { describe, expect, test } from "bun:test";
import type { PermissionRequest, SessionSummary } from "../acp/types";
import { DECIDED_MAX, deriveInbox, foldDecided, groupInbox, type InboxItem, showsNeedsYou } from "./inbox";
import type { Link } from "./orchestration";

const NOW = 1_700_000_000_000;

function session(id: string, title: string, cwd = "/code/app"): SessionSummary {
  return { id, title, cwd };
}

function request(
  sessionId: string,
  rpcId: number,
  at: number,
  extra: Partial<PermissionRequest> = {},
): PermissionRequest {
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
    ...extra,
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

const sessions = [
  session("one", "Fix the composer"),
  session("two", "Pricing page", "/code/website"),
  session("child", "The delegated one", "/code/website"),
];

describe("the needs-you inbox", () => {
  test("lists approvals and questions newest first, with their thread and folder", () => {
    const items = deriveInbox(
      {
        sessions,
        threads: {
          one: { permissions: [request("one", 1, NOW - 60_000)] },
          two: {
            permissions: [
              request("two", 2, NOW - 1_000, {
                question: { header: "Tests", text: "Which runner?", descriptions: [], multiple: false },
              }),
            ],
          },
        },
        failed: {},
        links: [],
      },
      NOW,
    );
    expect(items.map((item) => [item.kind, item.threadId, item.detail])).toEqual([
      ["question", "two", "Which runner?"],
      ["approval", "one", "Wants to bash"],
    ]);
    expect(items[1]!.project).toBe("/code/app");
    expect(items[0]!.title).toBe("Pricing page");
  });

  test("one thread's approvals carry their place in the set", () => {
    const items = deriveInbox(
      {
        sessions,
        threads: {
          one: { permissions: [request("one", 1, NOW - 3_000), request("one", 2, NOW - 2_000)] },
        },
        failed: {},
        links: [],
      },
      NOW,
    );
    expect(items.map((item) => [item.number, item.pending])).toEqual([
      [2, 2],
      [1, 2],
    ]);
  });

  test("failed turns are rows, from the store's map and from a replayed last turn", () => {
    const items = deriveInbox(
      {
        sessions,
        threads: {
          two: {
            permissions: [],
            items: [{ kind: "turnEnd", outcome: "failed", at: NOW - 500 }],
          },
        },
        failed: { one: NOW - 1_000 },
        links: [],
      },
      NOW,
    );
    expect(items.map((item) => [item.kind, item.threadId, item.at])).toEqual([
      ["failed", "two", NOW - 500],
      ["failed", "one", NOW - 1_000],
    ]);
  });

  test("a settled thread's failed turn is no row, from either source", () => {
    const settled = sessions.map((s) => (s.id === "one" ? { ...s, status: "done" as const } : s));
    const items = deriveInbox(
      {
        sessions: settled,
        threads: {
          two: {
            permissions: [],
            items: [{ kind: "turnEnd", outcome: "failed", at: NOW - 500 }],
          },
        },
        failed: { one: NOW - 1_000 },
        archived: { two: NOW },
        links: [],
      },
      NOW,
    );
    expect(items).toEqual([]);
  });

  test("a delegate that asks is one row, and not a second one beside its request", () => {
    const items = deriveInbox(
      {
        sessions,
        threads: { child: { permissions: [request("child", 7, NOW - 100)] } },
        failed: {},
        links: [link()],
      },
      NOW,
    );
    expect(items.map((item) => item.kind)).toEqual(["approval"]);

    const asking = deriveInbox({ sessions, threads: {}, failed: {}, links: [link()] }, NOW);
    expect(asking.map((item) => [item.kind, item.threadId, item.title])).toEqual([
      ["delegate", "child", "The delegated one"],
    ]);
  });
});

describe("the sidebar's Needs you row", () => {
  const read = (overrides: Partial<Parameters<typeof deriveInbox>[0]> = {}) =>
    deriveInbox({ sessions, threads: {}, failed: {}, links: [], ...overrides }, NOW);

  test("is not drawn when nothing waits, so the thread rows start the list", () => {
    expect(showsNeedsYou(read())).toBe(false);
  });

  test("a thread marked for review is not something that waits on you", () => {
    // Review is a mark the user sets to come back to; it asks nothing of them.
    const review = [{ ...session("three", "Read this later"), status: "review" as const }];
    expect(showsNeedsYou(read({ sessions: review }))).toBe(false);
    expect(showsNeedsYou(read({ sessions: [...sessions, ...review] }))).toBe(false);
  });

  test("is drawn for what the inbox holds, and its count is the rows", () => {
    const approval = read({ threads: { one: { permissions: [request("one", 1, NOW)] } } });
    const question = read({
      threads: {
        two: {
          permissions: [
            request("two", 2, NOW, {
              question: { header: "Tests", text: "Which runner?", descriptions: [], multiple: false },
            }),
          ],
        },
      },
    });
    const failed = read({ failed: { one: NOW } });
    const delegate = read({ links: [link()] });
    for (const items of [approval, question, failed, delegate]) {
      expect(showsNeedsYou(items)).toBe(true);
      expect(items).toHaveLength(1);
    }
  });
});

describe("the list's grouping", () => {
  const row = (threadId: string, at: number): InboxItem => ({
    key: `${threadId}:${at}`,
    kind: "approval",
    threadId,
    title: threadId === "one" ? "Add the login form" : "Pricing page",
    project: "/code/app",
    detail: "Wants to bash",
    at,
    pending: 1,
    number: 1,
  });

  test("consecutive rows from one thread share a header", () => {
    const groups = groupInbox([row("one", 4), row("one", 3), row("two", 2), row("one", 1)]);
    expect(groups.map((group) => [group.threadId, group.title, group.items.length])).toEqual([
      ["one", "Add the login form", 2],
      ["two", "Pricing page", 1],
      ["one", "Add the login form", 1],
    ]);
  });
});

describe("the decided section", () => {
  const kept: InboxItem = {
    key: "one:request:1",
    kind: "approval",
    threadId: "one",
    title: "Fix the composer",
    project: "/code/app",
    detail: "Wants to bash",
    at: NOW - 1_000,
    pending: 1,
    number: 1,
  };
  const gone: InboxItem = { ...kept, key: "one:request:2", at: NOW - 2_000 };

  test("a row that left with an answer is folded in, with what was done", () => {
    const decided = foldDecided([kept, gone], [kept], [], {
      now: NOW,
      labels: { [gone.key]: "Approved" },
    });
    expect(decided.map((item) => [item.key, item.label, item.decidedAt])).toEqual([
      [gone.key, "Approved", NOW],
    ]);
  });

  test("a first render folds nothing, and the list stays capped", () => {
    expect(foldDecided([], [kept], [])).toEqual([]);
    const many = Array.from({ length: DECIDED_MAX + 4 }, (_, i) => ({
      ...gone,
      key: `${gone.key}:${i}`,
      decidedAt: NOW + i,
      label: "Opened",
    }));
    const folded = foldDecided([gone], [], many, { now: NOW });
    expect(folded).toHaveLength(DECIDED_MAX);
    expect(folded[0]!.key).toBe(gone.key);
  });
});
