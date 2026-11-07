import { describe, expect, test } from "bun:test";
import {
  barLayout,
  buildTrace,
  hasTimeline,
  rowTokens,
  rowValue,
  sortTrace,
  type TraceCall,
  type TraceInput,
  usageDelta,
} from "./trace";

const start = 1_000_000;
const call = (over: Partial<TraceCall> & Pick<TraceCall, "id" | "startedAt">): TraceCall => ({
  label: over.id,
  status: "completed",
  ...over,
});

const input = (over: Partial<TraceInput> = {}): TraceInput => ({
  start,
  end: start + 10_000,
  calls: [],
  ...over,
});

describe("a turn worth tracing", () => {
  test("a turn the app watched run has a start and an end", () => {
    expect(hasTimeline({ startedAt: 1_000, at: 4_000 })).toBe(true);
  });

  test("a replayed turn has no start of its own, and so no trace", () => {
    expect(hasTimeline({ at: 4_000 })).toBe(false);
    expect(hasTimeline({ startedAt: 4_000, at: 4_000 })).toBe(false);
    expect(hasTimeline({})).toBe(false);
  });
});

describe("a turn's trace", () => {
  test("model calls fill the time no tool call was running", () => {
    const trace = buildTrace(
      input({
        calls: [
          call({ id: "read", startedAt: start + 2_000, endedAt: start + 3_000 }),
          call({ id: "bash", startedAt: start + 5_000, endedAt: start + 8_000 }),
        ],
      }),
    );
    expect(
      trace.rows.map((row) => [row.kind, row.label, row.startedAt - start, row.endedAt - start]),
    ).toEqual([
      ["model", "Model 1", 0, 2_000],
      ["tool", "read", 2_000, 3_000],
      ["model", "Model 2", 3_000, 5_000],
      ["tool", "bash", 5_000, 8_000],
      ["model", "Model 3", 8_000, 10_000],
    ]);
    expect(trace.durationMs).toBe(10_000);
  });

  test("two calls that overlap leave no model call between them", () => {
    const trace = buildTrace(
      input({
        calls: [
          call({ id: "one", startedAt: start + 1_000, endedAt: start + 6_000 }),
          call({ id: "two", startedAt: start + 2_000, endedAt: start + 4_000 }),
        ],
      }),
    );
    expect(trace.rows.filter((row) => row.kind === "model")).toHaveLength(2);
  });

  test("a model call's text names what it wrote", () => {
    const trace = buildTrace(
      input({
        text: [
          { at: start + 500, kind: "thought", text: "**Looking** at the composer" },
          { at: start + 9_000, kind: "reply", text: "Fixed the focus ring." },
        ],
        calls: [call({ id: "read", startedAt: start + 2_000, endedAt: start + 3_000 })],
      }),
    );
    const models = trace.rows.filter((row) => row.kind === "model");
    expect(models[0]).toMatchObject({ label: "Model 1", detail: "Thought before its next call" });
    expect(models[0]?.text).toContain("Looking");
    expect(models[1]).toMatchObject({
      label: "Model 2",
      detail: "Wrote the reply",
      text: "Fixed the focus ring.",
    });
  });

  test("a call still running reaches the end of the turn", () => {
    const trace = buildTrace(
      input({ calls: [call({ id: "dev", startedAt: start + 4_000, status: "running" })] }),
    );
    expect(trace.rows.find((row) => row.id === "dev")).toMatchObject({
      status: "running",
      endedAt: start + 10_000,
    });
    expect(trace.rows.filter((row) => row.kind === "model")).toHaveLength(1);
  });

  test("calls outside the turn's own window are not drawn", () => {
    const trace = buildTrace(
      input({
        calls: [
          call({ id: "before", startedAt: start - 5_000, endedAt: start - 4_000 }),
          call({ id: "after", startedAt: start + 20_000, endedAt: start + 21_000 }),
          call({ id: "inside", startedAt: start + 1_000, endedAt: start + 2_000 }),
        ],
      }),
    );
    expect(trace.rows.filter((row) => row.kind === "tool").map((row) => row.id)).toEqual(["inside"]);
  });

  test("the longest row is marked slowest, and one row alone is not", () => {
    const trace = buildTrace(
      input({
        calls: [
          call({ id: "quick", startedAt: start + 1_000, endedAt: start + 1_500 }),
          call({ id: "slow", startedAt: start + 2_000, endedAt: start + 8_000 }),
        ],
      }),
    );
    expect(trace.slowest).toBe("slow");
    // One call that fills the turn is the only row: nothing to beat.
    expect(
      buildTrace(input({ calls: [call({ id: "only", startedAt: start, endedAt: start + 10_000 })] })).slowest,
    ).toBeUndefined();
  });
});

describe("delegates", () => {
  test("fold into one dotted row that spans them all and keeps its calls", () => {
    const trace = buildTrace(
      input({
        calls: [
          call({
            id: "first",
            label: "explorer",
            detail: "Map the composer",
            startedAt: start + 1_000,
            endedAt: start + 4_000,
            delegate: true,
          }),
          call({
            id: "second",
            label: "worker",
            startedAt: start + 3_000,
            endedAt: start + 7_000,
            delegate: true,
            status: "failed",
          }),
        ],
      }),
    );
    const group = trace.rows.find((row) => row.kind === "delegates");
    expect(group).toMatchObject({
      label: "2 delegates",
      startedAt: start + 1_000,
      endedAt: start + 7_000,
      status: "failed",
    });
    expect(group?.children?.map((child) => child.label)).toEqual(["explorer", "worker"]);
    // The calls themselves are not separate rows.
    expect(trace.rows.filter((row) => row.kind === "tool")).toHaveLength(0);
  });

  test("one delegate says so in the singular", () => {
    const trace = buildTrace(
      input({
        calls: [call({ id: "one", startedAt: start + 1_000, endedAt: start + 2_000, delegate: true })],
      }),
    );
    expect(trace.rows.find((row) => row.kind === "delegates")?.label).toBe("1 delegate");
  });
});

describe("the money a turn spent", () => {
  const before = { inputTokens: 90_000, outputTokens: 3_200, cachedTokens: 61_000, cost: 0.01 };
  const samples = [
    {
      at: start + 1_900,
      usage: { inputTokens: 120_000, outputTokens: 4_100, cachedTokens: 66_000, cost: 0.0276 },
    },
    {
      at: start + 4_900,
      usage: { inputTokens: 147_000, outputTokens: 4_900, cachedTokens: 80_000, cost: 0.0341 },
    },
    {
      at: start + 10_000,
      usage: { inputTokens: 214_000, outputTokens: 7_100, cachedTokens: 117_000, cost: 0.0612 },
    },
  ];
  const calls = [
    call({ id: "read", startedAt: start + 2_000, endedAt: start + 3_000 }),
    call({ id: "bash", startedAt: start + 5_000, endedAt: start + 8_000 }),
  ];

  test("a sample minus the one before it prices one model call", () => {
    const trace = buildTrace(input({ calls, before, samples }));
    const usages = trace.rows.filter((row) => row.kind === "model").map((row) => row.usage!);
    expect(usages.map((usage) => [usage.inputTokens, usage.outputTokens, usage.cachedTokens])).toEqual([
      [30_000, 900, 5_000],
      [27_000, 800, 14_000],
      [67_000, 2_200, 37_000],
    ]);
    expect(usages.map((usage) => usage.cost)).toEqual([
      expect.any(Number),
      expect.any(Number),
      expect.any(Number),
    ]);
    expect(usages[0]!.cost!).toBeCloseTo(0.0176, 10);
    expect(usages[1]!.cost!).toBeCloseTo(0.0065, 10);
    expect(usages[2]!.cost!).toBeCloseTo(0.0271, 10);
    expect(trace.unpriced).toBe(0);
    expect(trace.total).toMatchObject({ inputTokens: 124_000, outputTokens: 3_900 });
    expect(trace.total?.cost).toBeCloseTo(0.0512, 10);
  });

  test("a harness that reported fewer samples than rounds prices none of them", () => {
    const trace = buildTrace(input({ calls, before, samples: [samples[0]!, samples[2]!] }));
    expect(trace.rows.every((row) => row.usage === undefined)).toBe(true);
    expect(trace.unpriced).toBe(3);
    // The turn's own totals are still known.
    expect(trace.total?.inputTokens).toBe(124_000);
  });

  test("no baseline leaves the first call unpriced, and never the turn's own total", () => {
    const trace = buildTrace(input({ calls, samples }));
    const models = trace.rows.filter((row) => row.kind === "model");
    expect(models[0]?.usage).toBeUndefined();
    expect(models[1]?.usage?.inputTokens).toBe(27_000);
    expect(trace.unpriced).toBe(1);
    // The turn's own total needs a start: a session total is not the turn's.
    expect(trace.total).toBeUndefined();
  });

  test("a counter going backwards is a new session, not a negative number", () => {
    expect(
      usageDelta(
        { inputTokens: 10, outputTokens: 5, cost: 0.1 },
        { inputTokens: 20, outputTokens: 1, cost: 0.2 },
      ),
    ).toEqual({ outputTokens: 4 });
  });
});

describe("ordering and layout", () => {
  const rows = () =>
    buildTrace(
      input({
        calls: [
          call({ id: "short", startedAt: start + 1_000, endedAt: start + 2_000 }),
          call({ id: "long", startedAt: start + 3_000, endedAt: start + 8_000 }),
        ],
        before: { inputTokens: 0, outputTokens: 0, cachedTokens: 0, cost: 0 },
        samples: [
          { at: start + 1_900, usage: { inputTokens: 25_000, outputTokens: 1_000, cost: 0.01 } },
          { at: start + 8_100, usage: { inputTokens: 40_000, outputTokens: 2_000, cost: 0.03 } },
          { at: start + 10_000, usage: { inputTokens: 60_000, outputTokens: 3_000, cost: 0.06 } },
        ],
      }),
    ).rows;

  test("by duration, longest first", () => {
    expect(sortTrace(rows(), "duration", "cost").map((row) => row.kind)).toEqual([
      "tool",
      "model",
      "model",
      "tool",
      "model",
    ]);
    expect(sortTrace(rows(), "duration", "cost").map((row) => row.id)).toEqual([
      "long",
      "model-3",
      "model-1",
      "short",
      "model-2",
    ]);
  });

  test("by cost, the priciest first and unknown numbers last", () => {
    const ordered = sortTrace(rows(), "cost", "cost");
    expect(ordered.map((row) => row.kind)).toEqual(["model", "model", "model", "tool", "tool"]);
    expect(ordered.slice(0, 3).map((row) => rowValue(row, "cost"))).toEqual([
      expect.any(Number),
      expect.any(Number),
      expect.any(Number),
    ]);
    expect(rowValue(ordered[0]!, "cost")!).toBeCloseTo(0.03, 10);
    expect(rowValue(ordered[1]!, "cost")!).toBeCloseTo(0.02, 10);
    expect(rowValue(ordered[2]!, "cost")!).toBeCloseTo(0.01, 10);
  });

  test("by tokens when the panel shows tokens", () => {
    const ordered = sortTrace(rows(), "cost", "tokens");
    expect(ordered[0]?.kind).toBe("model");
    expect(rowTokens(ordered[0]?.usage)).toBe(26_000);
    expect(ordered.slice(-2).map((row) => rowValue(row, "tokens"))).toEqual([undefined, undefined]);
  });

  test("timeline keeps the order the turn happened in", () => {
    const timeline = rows();
    expect(sortTrace(timeline, "timeline", "cost")).toBe(timeline);
  });

  test("bars sit on the turn's timeline and stay inside the track", () => {
    const trace = { start, end: start + 10_000 };
    const at = (from: number, to: number) => ({
      id: "x",
      kind: "tool" as const,
      label: "x",
      status: "completed" as const,
      startedAt: start + from,
      endedAt: start + to,
    });
    expect(barLayout(at(2_000, 5_000), trace)).toEqual({ left: 20, width: 30 });
    expect(barLayout(at(-1_000, 20_000), trace)).toEqual({ left: 0, width: 100 });
    // A call with no measured time still draws a hairline of a bar.
    expect(barLayout(at(4_000, 4_000), trace)).toEqual({ left: 40, width: 0 });
    expect(barLayout(at(4_000, 10_000), { start, end: start })).toEqual({ left: 0, width: 100 });
  });
});
