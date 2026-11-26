import { describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import type { SessionSummary } from "../acp/types";
import {
  backfillCandidates,
  fetchReplay,
  inProject,
  MAX_DROP_RETRIES,
  ReplayDropGuard,
  threadFromReplay,
} from "./mirrorBackfill";

const row = (id: string, updatedAt: number, extra: Partial<SessionSummary> = {}): SessionSummary =>
  ({ id, cwd: "/srv/app", title: id, updatedAt, ...extra }) as SessionSummary;
const none = { held: () => false, excludedProjects: [] };

describe("which thread copies a pass reads", () => {
  test("a thread with no copy, or one older than the thread, newest first", () => {
    const rows = [row("old", 10), row("fresh", 30), row("stale", 20), row("kept", 40)];
    const mirrored = [
      { session: row("stale", 20), transcriptRevision: 15 },
      { session: row("kept", 40), transcriptRevision: 45 },
      { session: row("old", 10) },
    ];
    expect(backfillCandidates(rows, mirrored, none).map((r) => r.id)).toEqual(["fresh", "stale", "old"]);
  });

  test("skips running threads, threads the app holds and projects left out", () => {
    const rows = [
      row("running", 5, { status: "running" }),
      row("asking", 5, { status: "needs_input" }),
      row("open", 5),
      row("private", 5, { cwd: "/SRV/Private/repo" }),
      row("done", 5, { status: "done" }),
    ];
    const ids = backfillCandidates(rows, [], {
      held: (id) => id === "open",
      excludedProjects: ["/srv/private"],
    }).map((r) => r.id);
    expect(ids).toEqual(["done"]);
  });

  test("stops at the limit", () => {
    const rows = [row("a", 3), row("b", 2), row("c", 1)];
    expect(backfillCandidates(rows, [], { ...none, limit: 2 }).map((r) => r.id)).toEqual(["a", "b"]);
  });
});

describe("a folder inside a project left out", () => {
  test("matches by whole path components, case-insensitively", () => {
    expect(inProject("/srv/private/repo", "/srv/private")).toBe(true);
    expect(inProject("/srv/Private", "/SRV/private/")).toBe(true);
    expect(inProject("/srv/private-2", "/srv/private")).toBe(false);
    expect(inProject("/srv", "/srv/private")).toBe(false);
    expect(inProject("/srv", "")).toBe(false);
  });
});

describe("a replay drawn as a thread", () => {
  const fallback = row("t1", 1);
  test("folds the updates in, as opening the thread would", () => {
    const answer = {
      sessionId: "t1",
      cwd: "/srv/app",
      title: "Fix the build",
      updates: [
        { sessionUpdate: "user_message_chunk", content: { type: "text", text: "hello" } },
        { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hi there" } },
      ],
    };
    const thread = threadFromReplay("vps/t1", answer, fallback);
    expect(thread?.id).toBe("vps/t1");
    expect(thread?.title).toBe("Fix the build");
    expect(thread?.load).toEqual({ phase: "loaded" });
    expect(thread?.items.map((item) => item.kind)).toEqual(["user", "assistant"]);
  });

  test("is nothing when the answer is not a replay", () => {
    expect(threadFromReplay("vps/t1", null, fallback)).toBeNull();
    expect(threadFromReplay("vps/t1", { cwd: "/x" }, fallback)).toBeNull();
  });
});

describe("paginated replay fetching", () => {
  test("follows nextCursor until the final page and merges all updates", async () => {
    const calls: JsonObject[] = [];
    const pages: JsonObject[] = [
      {
        sessionId: "t1",
        cwd: "/srv/app",
        title: "Long thread",
        updates: [{ sessionUpdate: "user_message_chunk", content: { type: "text", text: "hi" } }],
        nextCursor: 1,
      },
      {
        sessionId: "t1",
        cwd: "/srv/app",
        title: "Long thread",
        updates: [{ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hello" } }],
      },
    ];
    const connection = {
      async request<T = unknown>(_method: string, params?: JsonObject): Promise<T> {
        calls.push(params ?? {});
        return pages[calls.length - 1] as T;
      },
    };
    const replay = await fetchReplay(connection, "t1");
    expect(calls).toEqual([{ sessionId: "t1" }, { sessionId: "t1", cursor: 1 }]);
    expect(replay).not.toBeNull();
    const thread = threadFromReplay("vps/t1", replay, row("t1", 1));
    expect(thread?.title).toBe("Long thread");
    expect(thread?.items.map((item) => item.kind)).toEqual(["user", "assistant"]);
  });

  test("stops when nextCursor does not advance", async () => {
    let calls = 0;
    const connection = {
      async request<T = unknown>(): Promise<T> {
        calls += 1;
        return {
          sessionId: "t1",
          cwd: "/srv/app",
          title: "Stuck",
          updates: [{ sessionUpdate: "user_message_chunk", content: { type: "text", text: "hi" } }],
          nextCursor: 0,
        } as T;
      },
    };
    const replay = await fetchReplay(connection, "t1");
    expect(calls).toBe(1);
    expect(Array.isArray(replay?.updates)).toBe(true);
  });
});

describe("replay drop guard", () => {
  test("skips a thread after repeated disconnects on the same revision and retries when updated", () => {
    const guard = new ReplayDropGuard();
    const t1 = row("t1", 100);
    for (let i = 0; i < MAX_DROP_RETRIES - 1; i++) {
      guard.recordDrop("vps", t1);
      expect(guard.isSkipped("vps", t1)).toBe(false);
    }
    guard.recordDrop("vps", t1);
    expect(guard.isSkipped("vps", t1)).toBe(true);
    expect(
      backfillCandidates([t1, row("t2", 50)], [], {
        ...none,
        skipped: (r) => guard.isSkipped("vps", r),
      }).map((r) => r.id),
    ).toEqual(["t2"]);
    expect(guard.isSkipped("vps", row("t1", 200))).toBe(false);
    guard.clear("vps", "t1");
    expect(guard.isSkipped("vps", t1)).toBe(false);
  });
});
