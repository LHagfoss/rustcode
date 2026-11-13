// The status panel's rows and the local reply to a bare "status" (issue 3651).

import { describe, expect, test } from "bun:test";
import type { DelegatePull } from "../acp/rustcode";
import {
  idleText,
  isStatusQuery,
  pullText,
  runsOn,
  type StatusInput,
  statusReply,
  statusRows,
  statusSummary,
} from "./delegateStatus";

const pull = (over: Partial<DelegatePull> = {}): DelegatePull => ({
  number: 3649,
  title: "feat",
  state: "open",
  merged: false,
  draft: false,
  branch: "feat/x",
  headSha: "abc",
  ci: "passing",
  failing: [],
  pending: 0,
  passed: 4,
  ...over,
});

const input = (over: Partial<StatusInput> = {}): StatusInput => ({
  id: "t1",
  title: "Fix composer",
  state: "working",
  ...over,
});

describe("isStatusQuery", () => {
  test("a bare status, in any case, with a question mark or room around it", () => {
    for (const text of ["status", "Status", "status?", "  STATUS ?  ", "status??"]) {
      expect(isStatusQuery(text)).toBe(true);
    }
  });

  test("anything longer is a message for the model", () => {
    for (const text of [
      "",
      "status of the PR?",
      "what is the status",
      "status please",
      "/status",
      "statuses",
      "status\nand then merge",
    ]) {
      expect(isStatusQuery(text)).toBe(false);
    }
  });
});

describe("statusRows", () => {
  test("what needs the user comes first, then what works, the rest keeps its order", () => {
    const rows = statusRows(
      [
        input({ id: "a", state: "done" }),
        input({ id: "b", state: "working" }),
        input({ id: "c", state: "needs" }),
        input({ id: "d", state: "failed" }),
      ],
      0,
    );
    expect(rows.map((row) => row.id)).toEqual(["c", "b", "a", "d"]);
  });

  test("idle time is only told of a working delegate, and never negative", () => {
    const [working, done] = statusRows(
      [input({ lastAt: 90_000 }), input({ id: "t2", state: "done", lastAt: 1_000 })],
      100_000,
    );
    expect(working!.idleMs).toBe(10_000);
    expect(done!.idleMs).toBeUndefined();
    expect(statusRows([input({ lastAt: 200_000 })], 100_000)[0]!.idleMs).toBe(0);
  });
});

describe("what a row says", () => {
  test("the pull request and its checks", () => {
    expect(pullText(undefined)).toBe("—");
    expect(pullText({ pull: null })).toBe("no pull request");
    expect(pullText({ pull: null, note: "no pull request provider for this remote" })).toBe(
      "no pull request provider for this remote",
    );
    expect(pullText({ pull: pull() })).toBe("#3649 · checks passing");
    expect(pullText({ pull: pull({ ci: "pending", pending: 3 }) })).toBe("#3649 · 3 pending");
    expect(pullText({ pull: pull({ ci: "failing", failing: ["test", "lint"] }) })).toBe("#3649 · 2 failing");
    expect(pullText({ pull: pull({ ci: "none", draft: true }) })).toBe("#3649 · draft · no checks");
    expect(pullText({ pull: pull({ merged: true, state: "merged" }) })).toBe("#3649 · merged");
    expect(pullText({ pull: pull({ state: "closed" }) })).toBe("#3649 · closed");
  });

  test("idle time and what it runs on", () => {
    expect(idleText(undefined)).toBe("");
    expect(idleText(20_000)).toBe("active");
    expect(idleText(125_000)).toBe("idle 2m 5s");
    expect(runsOn({ harness: "claude-code", model: "opus", account: "work" })).toBe("opus · work");
    expect(runsOn({ harness: "codex" })).toBe("codex");
    expect(runsOn({})).toBe("");
  });

  test("the summary counts each state once", () => {
    const rows = statusRows(
      [
        input({ id: "a" }),
        input({ id: "b" }),
        input({ id: "c", state: "needs" }),
        input({ id: "d", state: "done" }),
      ],
      0,
    );
    expect(statusSummary(rows)).toBe("2 working · 1 needs you · 1 done");
    expect(statusSummary([])).toBe("no delegates");
  });

  test("the reply is one line a delegate, from the same words", () => {
    const rows = statusRows(
      [
        input({
          lastAt: 40_000,
          lastTool: "Bash",
          harness: "claude-code",
          model: "opus",
          account: "work",
          branch: "feat/x",
          pull: { pull: pull({ ci: "failing", failing: ["test"] }) },
        }),
        input({ id: "t2", title: "Docs", state: "done" }),
      ],
      100_000,
    );
    expect(statusReply(rows)).toBe(
      [
        "1 working · 1 done",
        "",
        "- **Fix composer** — Working · last tool Bash · idle 1m 0s · opus · work · `feat/x` · #3649 · 1 failing",
        "- **Docs** — Done",
      ].join("\n"),
    );
    expect(statusReply([])).toBe("No delegates yet.");
  });
});
