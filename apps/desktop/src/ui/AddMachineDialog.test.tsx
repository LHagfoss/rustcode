import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { AccessChoice, CodeInput, ProgressSteps } from "./AddMachineDialog";
import type { ProgressStep } from "./machineLabels";

describe("the add machine steps", () => {
  const steps: ProgressStep[] = [
    { key: "a", label: "Sign in over SSH", state: "done" },
    { key: "b", label: "Install RustCode", state: "running", detail: "Downloading 1.32.0" },
    { key: "c", label: "Pair", state: "todo" },
    { key: "d", label: "Start its server", state: "failed" },
    { key: "e", label: "Check RustCode there", state: "skipped" },
  ];

  test("lists each step with its state", () => {
    const html = renderToStaticMarkup(<ProgressSteps steps={steps} label="Progress" />);
    for (const state of ["done", "running", "todo", "failed", "skipped"]) {
      expect(html).toContain(`data-state="${state}"`);
    }
    expect(html).toContain('aria-label="Progress"');
    expect(html).toContain("Downloading 1.32.0");
    expect(html).toContain("Check RustCode there · not needed");
  });

  test("renders nothing for no steps", () => {
    expect(renderToStaticMarkup(<ProgressSteps steps={[]} label="Progress" />)).toBe("");
  });
});

describe("the digit boxes", () => {
  test("one box per digit, filled from the value", () => {
    const html = renderToStaticMarkup(<CodeInput length={4} value="12" onChange={() => undefined} />);
    expect(html.split("<input").length - 1).toBe(4);
    expect(html).toContain('aria-label="Digit 1 of 4"');
    expect(html).toContain('value="1"');
    expect(html).toContain('value="2"');
  });
});

describe("the access choice", () => {
  test("says which computer can reach which, with Both ways first", () => {
    const one = renderToStaticMarkup(<AccessChoice mutual={false} host="box" onChange={() => undefined} />);
    expect(one).toContain("One way:");
    expect(one).toContain("box cannot reach this");
    expect(one.indexOf("Both ways")).toBeLessThan(one.indexOf("One way:"));
    expect(one.match(/checked=""/g)?.length).toBe(1);
    const both = renderToStaticMarkup(<AccessChoice mutual host="box" onChange={() => undefined} />);
    expect(both).toContain("each computer can see and run threads");
    expect(both.indexOf('checked=""')).toBeLessThan(both.indexOf("One way:"));
  });
});
