import { describe, expect, test } from "bun:test";
import { type FieldGauge, measuredFieldHeight } from "./fieldHeight";

const MIN = 58;
const MAX = 208;

/** A twin textarea: one line per newline plus its padding, whatever box it is in. */
function gaugeOf(line = 22.75, padding = 12): FieldGauge {
  const gauge: FieldGauge = {
    value: "",
    style: { width: "", height: "" },
    get scrollHeight() {
      return this.value.split("\n").length * line + padding;
    },
  };
  return gauge;
}

describe("measured field height", () => {
  test("one line stays at the field's minimum", () => {
    expect(measuredFieldHeight(gaugeOf(), { value: "hello", width: 500 }, MIN, MAX)).toBe(MIN);
  });

  test("a draft grows the field a line at a time, padding included", () => {
    expect(measuredFieldHeight(gaugeOf(), { value: "a\nb\nc", width: 500 }, MIN, MAX)).toBe(80.25);
  });

  test("past the last line the field stops at its maximum and scrolls", () => {
    const draft = Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n");
    expect(measuredFieldHeight(gaugeOf(), { value: draft, width: 500 }, MIN, MAX)).toBe(MAX);
  });

  test("the twin is measured at the field's own width, so a narrow card wraps the same", () => {
    const gauge = gaugeOf();
    measuredFieldHeight(gauge, { value: "hello", width: 321 }, MIN, MAX);
    expect(gauge.style.width).toBe("321px");
  });

  test("the twin is measured at zero height, so the text is what it reports", () => {
    const gauge = gaugeOf();
    gauge.style.height = "180px";
    measuredFieldHeight(gauge, { value: "hello", width: 500 }, MIN, MAX);
    expect(gauge.style.height).toBe("0px");
  });

  test("the twin carries the draft being measured, and nothing of the last one", () => {
    const gauge = gaugeOf();
    expect(measuredFieldHeight(gauge, { value: "a\nb\nc", width: 500 }, MIN, MAX)).toBe(80.25);
    // The field that shrank reports its own box, not its text; the twin still reports the text.
    expect(measuredFieldHeight(gauge, { value: "a", width: 500 }, MIN, MAX)).toBe(MIN);
  });
});
