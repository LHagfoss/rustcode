import { describe, expect, test } from "bun:test";
import { parseViz } from "./Viz";

describe("rustcode-viz", () => {
  test("parses each type and caps it", () => {
    const bars = parseViz(
      JSON.stringify({
        version: 1,
        type: "bar_chart",
        items: Array.from({ length: 40 }, (_, i) => ({ label: `l${i}`, value: i })),
      }),
    );
    expect("viz" in bars && bars.viz.type === "bar_chart" && bars.viz.items.length).toBe(30);
    const checklist = parseViz(
      JSON.stringify({
        version: 1,
        type: "checklist",
        items: [{ label: "a", status: "done" }, { label: "b" }],
      }),
    );
    expect("viz" in checklist && checklist.viz.type === "checklist" && checklist.viz.done).toBe(1);
    const table = parseViz(
      JSON.stringify({
        version: 1,
        type: "table",
        columns: ["Name", "Time"],
        rows: [["a", 2]],
        sort_by: { column: "time" },
      }),
    );
    expect("viz" in table && table.viz.type === "table" && table.viz.sort?.column).toBe(1);
    expect(
      "viz" in
        parseViz(
          JSON.stringify({ version: 1, type: "line_chart", labels: [1, 2], series: [{ values: [1, 2] }] }),
        ),
    ).toBe(true);
  });

  test("falls back with the reason", () => {
    expect(parseViz("{")).toEqual({ problem: "malformed JSON" });
    expect(parseViz(JSON.stringify({ version: 2, type: "tree" }))).toEqual({ problem: "version is not 1" });
    expect(parseViz(JSON.stringify({ version: 1, type: "pie" }))).toEqual({
      problem: "unsupported type pie",
    });
    expect(
      parseViz(JSON.stringify({ version: 1, type: "checklist", items: [{ label: "x", status: "maybe" }] })),
    ).toEqual({
      problem: "unknown status maybe",
    });
    expect(
      "problem" in
        parseViz(
          JSON.stringify({ version: 1, type: "line_chart", labels: [1], series: [{ values: [null] }] }),
        ),
    ).toBe(true);
  });
});
