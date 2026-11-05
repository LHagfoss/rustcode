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

const session = (id: string, parent?: { id?: string; title?: string }): SessionSummary => ({
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
