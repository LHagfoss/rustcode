import { describe, expect, test } from "bun:test";
import type { Json } from "../acp/jsonrpc";
import { parseUsageReport } from "../acp/rustcode";
import type { SessionSummary } from "../acp/types";
import { addTotals, emptyTotals, harnessTotals, threadReport, usedTokens } from "./statistics";

const report = (rows: Json[], totals?: Json, truncated = false) =>
  parseUsageReport({
    group_by: "model",
    rows,
    totals: totals ?? {},
    truncated,
  });

const row = (over: Record<string, unknown>) => ({
  harness: "codex",
  model: "gpt-6-luna",
  model_harness: "codex",
  input_tokens: 100,
  output_tokens: 10,
  cached_tokens: 50,
  cache_write_tokens: null,
  cost_usd: null,
  turns: 1,
  turns_without_tokens: 0,
  turns_without_any_tokens: 0,
  turns_without_input_tokens: 0,
  turns_without_output_tokens: 0,
  turns_without_cached_tokens: 0,
  turns_without_cache_write_tokens: 0,
  turns_without_cost: 1,
  ...over,
});

const session = (id: string, parent?: SessionSummary["parent"]): SessionSummary => ({
  id,
  cwd: "/repo",
  title: id,
  parent,
});

describe("recorded usage parsing", () => {
  test("a token or cost the harness never reported stays absent", () => {
    const parsed = report([
      row({ harness: "claude-code", input_tokens: null, output_tokens: null, cost_usd: null }),
      row({ harness: "codex", cost_usd: 1.25, turns_without_cost: 0 }),
    ]);
    expect(parsed.rows[0]).toMatchObject({
      harness: "claude-code",
      input: undefined,
      output: undefined,
      cached: 50,
      cost: undefined,
      turnsWithoutTokens: 0,
      turnsWithoutCost: 1,
    });
    expect(parsed.rows[1]).toMatchObject({ cost: 1.25, turnsWithoutCost: 0 });
  });

  test("totals keep a missing cost absent, not zero", () => {
    const parsed = report([], { input_tokens: 300, turns: 2, cost_usd: null });
    expect(parsed.totals.input).toBe(300);
    expect(parsed.totals.cost).toBeUndefined();
    expect(parsed.totals.turns).toBe(2);
  });

  test("a thread row carries the thread's model, so it can be priced", () => {
    const parsed = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "t", model: "gpt-6-luna" })],
      totals: {},
    });
    expect(parsed.rows[0]!.thread).toBe("t");
    expect(parsed.rows[0]!.model).toBe("gpt-6-luna");
    expect(parsed.rows[0]!.modelHarness).toBe("codex");
  });

  test("a thread row with no single harness and model carries neither", () => {
    const parsed = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "t", model: null, model_harness: null })],
      totals: {},
    });
    expect(parsed.rows[0]!.model).toBeUndefined();
    expect(parsed.rows[0]!.modelHarness).toBeUndefined();
  });
});

describe("by harness and model", () => {
  test("groups models under their harness and adds each column", () => {
    const parsed = report([
      row({ model: "gpt-6-luna", input_tokens: 100, output_tokens: 10 }),
      row({ model: "gpt-6-astra", input_tokens: 50, output_tokens: 5, cost_usd: 0.5 }),
      row({ harness: "claude-code", model: "sonnet", input_tokens: 25, output_tokens: 2 }),
    ]);
    const groups = harnessTotals(parsed.rows);
    expect(groups.map((g) => g.harness)).toEqual(["codex", "claude-code"]);
    expect(groups[0]!.totals).toMatchObject({ input: 150, output: 15, cost: 0.5, turns: 2 });
    expect(groups[0]!.models.map((m) => m.model)).toEqual(["gpt-6-luna", "gpt-6-astra"]);
    expect(usedTokens(groups[0]!.totals)).toBe(165);
  });

  test("a turn with no model is its own row, never dropped", () => {
    const groups = harnessTotals(report([row({ model: null })]).rows);
    expect(groups[0]!.models).toHaveLength(1);
    expect(groups[0]!.models[0]!.model).toBe("");
  });
});

describe("thread totals including delegates", () => {
  const threadRows = () =>
    parseUsageReport({
      group_by: "thread",
      rows: [
        row({ thread: "parent", title: "Parent", input_tokens: 100, output_tokens: 10, turns: 3 }),
        row({ thread: "child", title: "Child", input_tokens: 50, output_tokens: 5, turns: 2 }),
        row({ thread: "grandchild", title: "Grandchild", input_tokens: 25, output_tokens: 2, turns: 1 }),
        row({ thread: "sibling", title: "Sibling", input_tokens: 10, output_tokens: 1, turns: 1 }),
      ],
      totals: {},
    });

  test("a parent includes every thread it started and each thread appears once", () => {
    const answer = threadReport(
      threadRows(),
      [
        session("parent"),
        session("child", { id: "parent" }),
        session("grandchild", { id: "child" }),
        session("sibling"),
      ],
      [],
    );
    expect(answer.threads.map((t) => t.id)).toEqual(["parent", "sibling"]);
    const parent = answer.threads[0]!;
    expect(parent.own).toMatchObject({ input: 100, output: 10, turns: 3 });
    expect(parent.total).toMatchObject({ input: 175, output: 17, turns: 6 });
    expect(parent.delegateCount).toBe(2);
    // No thread is counted in two top-level rows.
    const shown = answer.threads.reduce((sum, thread) => sum + usedTokens(thread.total), 0);
    expect(shown).toBe(203);
  });

  test("a fork and a side chat are threads of their own, never rolled into the parent", () => {
    const sessions = [
      session("root"),
      session("fork", { id: "root", relationship: "fork" }),
      session("side", { id: "root", relationship: "side_chat" }),
      session("delegate", { id: "root", relationship: "delegate" }),
    ];
    const rows = (withParents: boolean) =>
      parseUsageReport({
        group_by: "thread",
        rows: [
          row({
            thread: "root",
            input_tokens: 1,
            output_tokens: 0,
            ...(withParents ? { parent_id: null } : {}),
          }),
          row({
            thread: "fork",
            input_tokens: 5,
            output_tokens: 0,
            ...(withParents ? { parent_id: null } : {}),
          }),
          row({
            thread: "side",
            input_tokens: 7,
            output_tokens: 0,
            ...(withParents ? { parent_id: null } : {}),
          }),
          row({
            thread: "delegate",
            input_tokens: 3,
            output_tokens: 0,
            ...(withParents ? { parent_id: "root" } : {}),
          }),
        ],
        totals: {},
      });
    // Whether the rows report their parents (the store) or the app reads the session list.
    for (const withParents of [true, false]) {
      const answer = threadReport(rows(withParents), sessions, []);
      expect(answer.threads.map((t) => t.id).sort()).toEqual(["fork", "root", "side"]);
      const root = answer.threads.find((t) => t.id === "root")!;
      expect(root.total).toMatchObject({ input: 4 });
      expect(root.delegateCount).toBe(1);
    }
  });

  test("a link that started a thread stands in for metadata the book outlived", () => {
    const answer = threadReport(
      threadRows(),
      [session("parent"), session("child"), session("grandchild", { id: "child" })],
      [
        {
          id: "parent/1",
          orchestrator: "parent",
          orchestratorTitle: "Parent",
          target: "child",
          targetTitle: "Child",
          message: "",
          started: true,
          state: "done",
          createdAt: 1,
        },
      ],
    );
    expect(answer.threads.map((t) => t.id)).toEqual(["parent", "sibling"]);
    expect(answer.threads[0]!.delegateCount).toBe(2);
  });

  test("a title-only parent is listed on its own, and says so", () => {
    const answer = threadReport(
      threadRows(),
      [
        session("parent"),
        session("child", { title: "A thread that is gone" }),
        session("grandchild", { id: "child" }),
        session("sibling"),
      ],
      [],
    );
    expect(answer.unlinked).toBe(1);
    const child = answer.threads.find((t) => t.id === "child")!;
    expect(child.unlinked).toBe(true);
    // Grandchild still follows its parent, so it is not another top-level row.
    expect(child.total).toMatchObject({ input: 75, output: 7 });
  });

  test("a parent with no recorded turns cannot hold a child", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "child", input_tokens: 10, output_tokens: 1 })],
      totals: {},
    });
    const answer = threadReport(rows, [session("child", { id: "gone" })], []);
    expect(answer.threads.map((t) => t.id)).toEqual(["child"]);
    expect(answer.unlinked).toBe(0);
  });

  test("a loop in the parent records is broken, not followed", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({ thread: "a", title: "A", input_tokens: 10, output_tokens: 1 }),
        row({ thread: "b", title: "B", input_tokens: 20, output_tokens: 2 }),
      ],
      totals: {},
    });
    const answer = threadReport(rows, [session("a", { id: "b" }), session("b", { id: "a" })], []);
    expect(answer.brokenParents).toBe(1);
    expect(answer.threads).toHaveLength(1);
    expect(answer.threads[0]!.total).toMatchObject({ input: 30, output: 3 });
  });

  test("a truncated read says so", () => {
    const rows = report([], {}, true);
    expect(threadReport(rows, [], []).truncated).toBe(true);
  });
});

describe("thread hierarchy source", () => {
  test("a thread's cost is its reported cost, or the estimate when none was reported", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "reported",
          title: "Reported",
          parent_id: null,
          parent_title: null,
          model: "gpt-6-astra",
          cost_usd: 2.5,
          turns: 1,
          turns_without_cost: 0,
        }),
        row({
          thread: "estimated",
          title: "Estimated",
          parent_id: null,
          parent_title: null,
          model: "gpt-6-luna",
          input_tokens: 1_000_000,
          output_tokens: 0,
          cached_tokens: 0,
          cost_usd: null,
          turns: 2,
          turns_without_cost: 2,
        }),
      ],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    expect(answer.threads.find((t) => t.id === "reported")!.cost).toMatchObject({
      usd: 2.5,
      reportedUsd: 2.5,
      unknownTurns: 0,
    });
    const estimated = answer.threads.find((t) => t.id === "estimated")!.cost;
    expect(estimated.reportedUsd).toBeUndefined();
    expect(estimated.estimatedUsd).toBeCloseTo(0.1, 10);
    expect(estimated.usd).toBeCloseTo(0.1, 10);
  });

  test("a parent's cost includes its delegates, and says part of it is estimated", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "parent",
          title: "Parent",
          parent_id: null,
          parent_title: null,
          model: "gpt-6-luna",
          input_tokens: 1_000_000,
          output_tokens: 0,
          cached_tokens: 0,
          cost_usd: null,
          turns: 2,
          turns_without_cost: 2,
        }),
        row({
          thread: "child",
          title: "Child",
          parent_id: "parent",
          parent_title: "Parent",
          model: "gpt-6-astra",
          cost_usd: 2.5,
          turns: 1,
          turns_without_cost: 0,
        }),
      ],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    const parent = answer.threads[0]!;
    expect(parent.ownCost.estimatedUsd).toBeCloseTo(0.1, 10);
    expect(parent.cost.reportedUsd).toBe(2.5);
    expect(parent.cost.estimatedUsd).toBeCloseTo(0.1, 10);
    expect(parent.cost.usd).toBeCloseTo(2.6, 10);
    expect(parent.delegateCount).toBe(1);
  });

  test("a thread is priced by the harness its turns share, not its display harness", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "rustcode-turns",
          // The store's display harness is the latest one; the turns agreed on rustcode.
          harness: "claude-code",
          model_harness: "rustcode",
          model: "gpt-6-luna",
          input_tokens: 1_000_000,
          cached_tokens: 400_000,
          output_tokens: 0,
          cost_usd: null,
          turns: 1,
          turns_without_cost: 1,
        }),
      ],
      totals: {},
    });
    const cost = threadReport(rows, [], []).threads[0]!.cost;
    // rustcode counts cached input inside input_tokens: 600k at $0.10 plus
    // 400k at $0.01, not 1M uncached at $0.10 (which the display harness would give).
    expect(cost.estimatedUsd).toBeCloseTo(0.064, 10);
  });

  test("a thread row without one shared harness and model is not estimated", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "switched",
          model: "gpt-6-luna",
          model_harness: null,
          input_tokens: 1_000_000,
          cached_tokens: 0,
          output_tokens: 0,
          cost_usd: null,
          turns: 2,
          turns_without_cost: 2,
        }),
      ],
      totals: {},
    });
    const cost = threadReport(rows, [], []).threads[0]!.cost;
    expect(cost.usd).toBeUndefined();
    expect(cost.unknownTurns).toBe(2);
  });

  test("a reported thread counts the turns that reported no cost", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "reported",
          cost_usd: 11.55,
          turns: 192,
          turns_without_cost: 31,
        }),
      ],
      totals: {},
    });
    const cost = threadReport(rows, [], []).threads[0]!.cost;
    expect(cost).toEqual({ usd: 11.55, reportedUsd: 11.55, unknownTurns: 31 });
  });

  test("a thread with no model, or with no price and no reported cost, is unknown", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({
          thread: "no-model",
          model: null,
          cost_usd: null,
          input_tokens: 1_000,
          output_tokens: 100,
          turns: 3,
        }),
        row({ thread: "no-price", model: "gpt-9-unknown", cost_usd: null, turns: 4 }),
      ],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    const noModel = answer.threads.find((t) => t.id === "no-model")!.cost;
    const noPrice = answer.threads.find((t) => t.id === "no-price")!.cost;
    expect(noModel.usd).toBeUndefined();
    expect(noModel.unknownTurns).toBe(3);
    expect(noPrice.usd).toBeUndefined();
    expect(noPrice.unknownTurns).toBe(4);
  });

  test("parent ids on the rows roll up without a session list", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({ thread: "parent", parent_id: null, parent_title: null, input_tokens: 100, output_tokens: 10 }),
        row({
          thread: "child",
          parent_id: "parent",
          parent_title: "Parent",
          input_tokens: 50,
          output_tokens: 5,
        }),
      ],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    expect(answer.parentsFromStore).toBe(true);
    expect(answer.threads.map((t) => t.id)).toEqual(["parent"]);
    expect(answer.threads[0]!.total).toMatchObject({ input: 150, output: 15 });
    expect(answer.missingSessions).toBe(2);
    expect(answer.hierarchyIncomplete).toBe(false);
  });

  test("a title-only parent on the row is counted on its own", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "child", parent_id: null, parent_title: "A thread that is gone" })],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    expect(answer.unlinked).toBe(1);
    expect(answer.threads[0]!.unlinked).toBe(true);
  });

  test("a row with no store parent falls back to the link book", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({ thread: "parent", parent_id: null, parent_title: null, input_tokens: 100 }),
        row({ thread: "child", parent_id: null, parent_title: null, input_tokens: 50 }),
      ],
      totals: {},
    });
    const answer = threadReport(
      rows,
      [],
      [
        {
          id: "parent/1",
          orchestrator: "parent",
          orchestratorTitle: "Parent",
          target: "child",
          targetTitle: "Child",
          message: "",
          started: true,
          state: "done",
          createdAt: 1,
        },
      ],
    );
    expect(answer.parentsFromStore).toBe(true);
    expect(answer.threads.map((t) => t.id)).toEqual(["parent"]);
    expect(answer.threads[0]!.total).toMatchObject({ input: 150 });
  });

  test("a store parent wins over a conflicting link", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [
        row({ thread: "store-parent", parent_id: null, parent_title: null, input_tokens: 1 }),
        row({ thread: "link-parent", parent_id: null, parent_title: null, input_tokens: 100 }),
        row({
          thread: "child",
          parent_id: "store-parent",
          parent_title: "Store parent",
          input_tokens: 10,
        }),
      ],
      totals: {},
    });
    const answer = threadReport(
      rows,
      [],
      [
        {
          id: "link-parent/1",
          orchestrator: "link-parent",
          orchestratorTitle: "Link parent",
          target: "child",
          targetTitle: "Child",
          message: "",
          started: true,
          state: "done",
          createdAt: 1,
        },
      ],
    );
    expect(answer.threads.find((t) => t.id === "store-parent")!.total.input).toBe(11);
    expect(answer.threads.find((t) => t.id === "link-parent")!.total.input).toBe(100);
  });

  test("a capped session list is called out when rows have no parent", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "child", input_tokens: 10, output_tokens: 1 })],
      totals: {},
    });
    const answer = threadReport(rows, [], [], true);
    expect(answer.parentsFromStore).toBe(false);
    expect(answer.sessionsCapped).toBe(true);
    expect(answer.hierarchyIncomplete).toBe(true);
  });

  test("a missing session is called out when rows have no parent", () => {
    const rows = parseUsageReport({
      group_by: "thread",
      rows: [row({ thread: "child", input_tokens: 10, output_tokens: 1 })],
      totals: {},
    });
    const answer = threadReport(rows, [], []);
    expect(answer.missingSessions).toBe(1);
    expect(answer.hierarchyIncomplete).toBe(true);
  });
});

describe("adding totals", () => {
  test("keeps absent counts absent and adds the rest", () => {
    expect(addTotals(emptyTotals(), { ...emptyTotals(), input: 10, cost: 0.2 })).toMatchObject({
      input: 10,
      output: undefined,
      cost: 0.2,
    });
  });
});
