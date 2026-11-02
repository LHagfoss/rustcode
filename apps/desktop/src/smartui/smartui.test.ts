import { describe, expect, test } from "bun:test";
import { SendGate } from "./host";
import { buildPage, PAGE_CSP, pageTheme, usesChart } from "./page";
import { blockProblem, blockTitle, clampHeight, splitReply, viewMessage } from "./parse";

describe("smart ui", () => {
  test("only `rustcode-ui v1` fences run; others stay Markdown", () => {
    const reply =
      "Intro\n\n```rustcode-ui v1\n<b>hi</b>\n```\n\n```html\n<b>no</b>\n```\n```rustcode-ui\n<b>no</b>\n```\nEnd";
    const parts = splitReply(reply);
    expect(parts.map((p) => p.kind)).toEqual(["markdown", "ui", "markdown"]);
    expect(parts[1]).toEqual({ kind: "ui", source: "<b>hi</b>", complete: true, index: 0 });
    expect(parts[2]?.kind === "markdown" && parts[2].text).toContain("```html");
  });

  test("an unfinished block is marked so", () => {
    const parts = splitReply("```rustcode-ui v1\n<div>");
    expect(parts).toEqual([{ kind: "ui", source: "<div>", complete: false, index: 0 }]);
  });

  test("a fence inside another fence is not a block", () => {
    const parts = splitReply("````md\n```rustcode-ui v1\nx\n```\n````");
    expect(parts.map((p) => p.kind)).toEqual(["markdown"]);
  });

  test("limits", () => {
    expect(blockProblem("  ")).toMatch(/empty/);
    expect(blockProblem("x".repeat(64 * 1024 + 1))).toMatch(/64 KB/);
    expect(blockProblem("<p>ok</p>")).toBeNull();
    expect(clampHeight(10)).toBe(48);
    expect(clampHeight(5000)).toBe(720);
    expect(blockTitle("<title>Runs</title><p>")).toBe("Runs");
    expect(viewMessage("  hi ")).toBe("hi");
    expect(viewMessage("x".repeat(2001))).toBeNull();
    expect(viewMessage(42)).toBeNull();
  });

  test("the page carries the policy, the variables, the runtime and Chart.js once", () => {
    const theme = pageTheme("dark", (name) => (name === "--rc-text" ? "rgb(255 255 255 / 0.88)" : "#123456"));
    const source =
      '<script src="rustcode-lib:chart.js"></script><script src="rustcode-lib:chart.js"></script><p>x</p>';
    expect(usesChart(source)).toBe(true);
    const page = buildPage(source, "tok", theme, "var Chart={}; // </script> inside");
    expect(page).toContain(`content="${PAGE_CSP}"`);
    expect(page).toContain("--rc-text: rgb(255 255 255 / 0.88);");
    expect(page).toContain("--rc-series-8: #ff375f;");
    expect(page).toContain("color-scheme: dark");
    expect(page.match(/var Chart=\{\}/g)).toHaveLength(1);
    expect(page).not.toContain("// </script> inside");
    expect(page).toContain('"tok"');
    for (const name of [
      ".rc-row",
      ".rc-col",
      ".rc-grid",
      ".rc-gap-4",
      ".rc-muted",
      ".rc-mono",
      ".rc-num",
      ".rc-card",
      ".rc-scroll",
    ]) {
      expect(page).toContain(name);
    }
  });

  test("a view sends at most one message every 2 seconds", () => {
    const gate = new SendGate();
    expect(gate.allow(10_000)).toBe(true);
    expect(gate.allow(11_000)).toBe(false);
    expect(gate.allow(12_001)).toBe(true);
  });
});

test("the app and the shell serve pages under the same policy", async () => {
  const rust = await Bun.file(new URL("../../src-tauri/src/smartui.rs", import.meta.url)).text();
  expect(rust).toContain(`pub const PAGE_CSP: &str = "${PAGE_CSP}";`);
});
