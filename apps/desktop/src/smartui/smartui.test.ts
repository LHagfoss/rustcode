import { describe, expect, test } from "bun:test";
import { MOCK_CHARTS, MOCK_KIT } from "../acp/mock";
import {
  AutoRun,
  claimLive,
  clearHalt,
  haltOf,
  haltView,
  liveCount,
  releaseLive,
  rememberedHeight,
  rememberHeight,
  SendGate,
  StreamPump,
} from "./host";
import { kitHelpers } from "./kit";
import {
  BASE_CSS,
  buildPage,
  buildStreamPage,
  CHART_THEME_SCRIPT,
  PAGE_CSP,
  pageTheme,
  runtimeScript,
  usesChart,
} from "./page";
import {
  blockProblem,
  blockTitle,
  clampHeight,
  closeOpenTags,
  MAX_LIVE,
  splitReply,
  stripEmptyTags,
  viewMessage,
} from "./parse";

/** Views as SmartUIView drives them: run whenever `ready` and not live, until nothing changes. */
function simulate(count: number) {
  const views = Array.from({ length: count }, (_, i) => ({
    id: `view${i}`,
    gate: new AutoRun(),
    live: false,
    starts: 0,
  }));
  const settle = () => {
    for (let round = 0; round < 1000; round++) {
      const next = views.find((v) => !v.live && v.gate.ready);
      if (!next) return round;
      next.starts++;
      next.live = true;
      claimLive(
        next.id,
        () => {
          next.gate.evicted();
          next.live = false;
        },
        () => next.gate.inView,
      );
    }
    throw new Error("views keep evicting each other");
  };
  const clear = () => {
    for (const v of views) releaseLive(v.id);
  };
  return { views, settle, clear };
}

describe("live views", () => {
  test("more views in sight than may run settles instead of looping", () => {
    const { views, settle, clear } = simulate(MAX_LIVE + 2);
    for (const v of views) v.gate.see(true);
    settle();
    expect(views.map((v) => v.starts)).toEqual(views.map(() => 1));
    expect(liveCount()).toBe(MAX_LIVE);
    // The two that started first gave way, and stay out while still in sight.
    expect(views.filter((v) => !v.live).map((v) => v.id)).toEqual(["view0", "view1"]);
    settle();
    expect(views.reduce((n, v) => n + v.starts, 0)).toBe(MAX_LIVE + 2);
    clear();
  });

  test("an evicted view runs again only after leaving and coming back", () => {
    const { views, settle, clear } = simulate(MAX_LIVE + 1);
    for (const v of views) v.gate.see(true);
    settle();
    const evicted = views[0]!;
    expect(evicted.live).toBe(false);
    expect(evicted.gate.ready).toBe(false);
    evicted.gate.see(false);
    expect(evicted.gate.ready).toBe(false);
    evicted.gate.see(true);
    expect(evicted.gate.ready).toBe(true);
    settle();
    expect(evicted.live).toBe(true);
    // It took the slot of the oldest in sight, which now waits the same way, so this ends too.
    expect(views[1]!.live).toBe(false);
    expect(views[1]!.gate.ready).toBe(false);
    clear();
  });

  test("a view out of sight is evicted first, and may run when it comes back", () => {
    const { views, settle, clear } = simulate(MAX_LIVE + 1);
    for (const v of views.slice(0, MAX_LIVE)) v.gate.see(true);
    settle();
    const hidden = views[2]!;
    hidden.gate.see(false);
    views[MAX_LIVE]!.gate.see(true);
    settle();
    expect(hidden.live).toBe(false);
    expect(views[0]!.live).toBe(true);
    // Evicted while out of sight: it has already left, so nothing holds it back.
    hidden.gate.see(true);
    expect(hidden.gate.ready).toBe(true);
    clear();
  });

  test("a claim replaces the same view's slot and never evicts it", () => {
    let stopped = 0;
    claimLive("same", () => stopped++);
    claimLive("same", () => stopped++);
    expect(liveCount()).toBe(1);
    expect(stopped).toBe(0);
    releaseLive("same");
    expect(liveCount()).toBe(0);
  });

  test("heights are kept per source and bounded", () => {
    rememberHeight("first", 300);
    expect(rememberedHeight("first")).toBe(300);
    rememberHeight("first", 320);
    expect(rememberedHeight("first")).toBe(320);
    for (let i = 0; i < 200; i++) rememberHeight(`other${i}`, i);
    expect(rememberedHeight("first")).toBeUndefined();
    expect(rememberedHeight("other199")).toBe(199);
  });

  test("a halt stays until it is cleared, for a later mount to read", () => {
    haltView("key", { kind: "stopped" });
    expect(haltOf("key")).toEqual({ kind: "stopped" });
    haltView("key", { kind: "failed", reason: "busy" });
    expect(haltOf("key")).toEqual({ kind: "failed", reason: "busy" });
    clearHalt("key");
    expect(haltOf("key")).toBeUndefined();
  });
});

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

  test("a partly arrived fragment renders: its open tags are closed", () => {
    expect(closeOpenTags('<div class="a"><p>hi')).toBe('<div class="a"><p>hi</p></div>');
    expect(closeOpenTags("<table><tr><td>1")).toBe("<table><tr><td>1</td></tr></table>");
    // A tag still being written is dropped, not shown as text.
    expect(closeOpenTags("<p>ok</p><div cla")).toBe("<p>ok</p>");
    expect(closeOpenTags("<p>ok</p><div")).toBe("<p>ok</p>");
    expect(closeOpenTags('<div class="a">x</div')).toBe('<div class="a">x</div>');
    // Nothing that needs closing.
    expect(closeOpenTags("<p>done</p>")).toBe("<p>done</p>");
    expect(closeOpenTags('<br><img src="data:,"><input value="1">')).toBe(
      '<br><img src="data:,"><input value="1">',
    );
    expect(closeOpenTags('<svg><path d="M0 0"/></svg>')).toBe('<svg><path d="M0 0"/></svg>');
    // Raw text runs to its own closing tag, or to the end of what has arrived.
    expect(closeOpenTags("<style>p { color: red")).toBe("<style>p { color: red</style>");
    expect(closeOpenTags("<style>p { color: red</style><p>x")).toBe("<style>p { color: red</style><p>x</p>");
    // A comment or doctype that has not finished yet is dropped with its open tags.
    expect(closeOpenTags("<div><!-- note")).toBe("<div></div>");
    expect(closeOpenTags("<!doctype html><p>x")).toBe("<!doctype html><p>x</p>");
    // A `<` in the text is text, and a stray close tag closes nothing.
    expect(closeOpenTags("<p>a < b")).toBe("<p>a < b</p>");
    expect(closeOpenTags("</div><p>x")).toBe("</div><p>x</p>");
    // The end of a streamed table is a whole block again.
    expect(closeOpenTags("<table><tr><td>1</td></tr></table>")).toBe("<table><tr><td>1</td></tr></table>");
  });

  test("an empty style or script tag is dropped from the page", () => {
    expect(stripEmptyTags("<style></style><p>x</p>")).toBe("<p>x</p>");
    expect(stripEmptyTags("<script>\n </script><p>x</p>")).toBe("<p>x</p>");
    expect(stripEmptyTags("<style>p{margin:0}</style>")).toBe("<style>p{margin:0}</style>");
    expect(stripEmptyTags("<script>let a=1;</script>")).toBe("<script>let a=1;</script>");
    const page = buildPage(
      "<style></style><script></script><p>x</p>",
      "tok",
      pageTheme("light", () => "#000"),
    );
    expect(page).not.toContain("<style></style>");
    expect(page).not.toContain("<script></script>");
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

  test("the kit is in the page's stylesheet, in variables only", () => {
    for (const name of [
      ".rc-title",
      ".rc-lead",
      ".rc-label",
      ".rc-stats",
      ".rc-stat-value",
      ".rc-stat-label",
      ".rc-stat-note",
      ".rc-badge",
      ".rc-ok",
      ".rc-warn",
      ".rc-bad",
      ".rc-info",
      ".rc-progress",
      ".rc-bars",
      ".rc-bar",
      ".rc-table",
      ".rc-list",
      ".rc-item",
      ".rc-steps",
      ".rc-step",
      ".rc-done",
      ".rc-active",
      ".rc-failed",
      ".rc-todo",
      ".rc-details",
      ".rc-tree",
      ".rc-term",
      ".rc-callout",
      ".rc-chip",
      ".rc-tabs",
      ".rc-tip",
      ".rc-chart",
      ".rc-donut-total",
      ".rc-spark",
      ".rc-spark-line",
      "data-rc-sort",
      "data-rc-tip",
      "aria-sort",
    ]) {
      expect(BASE_CSS).toContain(name);
    }
    const kit = BASE_CSS.slice(BASE_CSS.indexOf(".rc-title"));
    expect(kit).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i);
    expect(kit).toContain("prefers-reduced-motion");
  });

  test("the runtime measures the content and carries the kit's behaviour", () => {
    const script = runtimeScript(
      "tok",
      pageTheme("light", () => "#000"),
    );
    expect(script).not.toContain("root.scrollHeight");
    expect(script).toContain("observer.observe(document.body)");
    for (const part of [
      'addEventListener("click"',
      'addEventListener("keydown"',
      'addEventListener("DOMContentLoaded"',
      "[data-rc-send]",
      "[data-rc-copy]",
      "[data-rc-tab]",
      "[data-rc-panel]",
      '"Copied"',
      "sortTable",
      "data-rc-sort",
      "rcSparkValues",
      "rcSparkPoints",
      ".rc-spark[data-values]",
      "aria-sort",
    ]) {
      expect(script).toContain(part);
    }
    // The delegated send goes through `rustcode.send`, which checks the user's gesture.
    expect(script).toContain("rustcode.send(fields ?");
    expect(script.match(/post\(\{ type: "send"/g)).toHaveLength(1);
    // A generated script that does not parse would break every view; check it here.
    expect(() => new Function(script)).not.toThrow();
  });

  test("a form's fields ride along and a context is a JSON fence", () => {
    const script = runtimeScript(
      "tok",
      pageTheme("light", () => "#000"),
    );
    // Both go the one way out, through `rustcode.send`.
    expect(script).toContain("new FormData(form)");
    expect(script).toContain('lines.join("\\n")');
    // A form marked data-rc-send sends on submit, never on a click into it.
    expect(script).toContain('form.hasAttribute("data-rc-send")');
    expect(script).toContain('send.tagName !== "FORM"');
    expect(script).toContain("JSON.stringify(context, null, 2)");
    expect(script).toContain("send(text, context)");
    // A submit never navigates the frame.
    expect(script).toContain('addEventListener("submit"');
    expect(script).toContain("event.preventDefault()");
  });

  test("a streaming fragment is closed, and its script stays inert", () => {
    const theme = pageTheme("light", () => "#000");
    const page = buildStreamPage("tok", theme);
    expect(page).toContain(`content="${PAGE_CSP}"`);
    expect(page).toContain('<div id="rc-stream"></div>');
    expect(page).toContain('"stream"');
    expect(page).toContain("innerHTML = html");
    expect(page).toContain('type: "stream-ready"');
    // The shell is the same policy, stylesheet and runtime as a finished page.
    const full = buildPage("<p>x</p>", "tok", theme);
    for (const part of [PAGE_CSP, BASE_CSS.slice(0, 200), runtimeScript("tok", theme)]) {
      expect(full).toContain(part);
      expect(page).toContain(part);
    }
    // Every inline script in either page parses.
    for (const source of [
      page,
      buildPage('<script src="rustcode-lib:chart.js"></script>', "tok", theme, "var Chart={};"),
    ]) {
      const scripts = [...source.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]!);
      expect(scripts.length).toBeGreaterThan(0);
      for (const code of scripts) expect(() => new Function(code)).not.toThrow();
    }
  });

  test("the mock's kit view fits a block", () => {
    const [part] = splitReply(MOCK_KIT.join("")).filter((p) => p.kind === "ui");
    expect(part?.complete).toBe(true);
    expect(blockProblem(part?.source ?? "")).toBeNull();
    expect(new TextEncoder().encode(part?.source).length).toBeLessThan(64 * 1024);
  });

  test("the mock's chart and hover view fits a block", () => {
    const [part] = splitReply(MOCK_CHARTS.join("")).filter((p) => p.kind === "ui");
    expect(part?.complete).toBe(true);
    expect(blockProblem(part?.source ?? "")).toBeNull();
    // It uses the pieces this change adds, and nothing the page has not got.
    for (const name of ["rc-chart", "rc-donut-total", "rc-spark", "data-rc-sort", "data-rc-tip"]) {
      expect(part?.source).toContain(name);
    }
    expect(new TextEncoder().encode(part?.source).length).toBeLessThan(64 * 1024);
  });

  test("the chart theme follows the variables, in both themes", () => {
    expect(CHART_THEME_SCRIPT).toContain("--rc-series-");
    expect(CHART_THEME_SCRIPT).toContain("prefers-reduced-motion");
    expect(CHART_THEME_SCRIPT).toContain("usePointStyle");
    expect(CHART_THEME_SCRIPT).toContain("hoverBackgroundColor");
    expect(CHART_THEME_SCRIPT).toContain("createLinearGradient");
    expect(CHART_THEME_SCRIPT).toContain("__rcThemeCharts");
    // No raw colour but the tooltip's ink over --rc-text: the lines, bars and
    // slices are painted from a variable.
    expect(CHART_THEME_SCRIPT.replace(/"#000"|"#fff"/g, "")).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(() => new Function(CHART_THEME_SCRIPT)).not.toThrow();
  });

  test("a cell is a measurement only when it is one", () => {
    const { rcMeasure } = kitHelpers();
    expect(rcMeasure("1,234")).toEqual({ value: 1234, unit: "" });
    expect(rcMeasure("41 s")).toEqual({ value: 41, unit: "s" });
    expect(rcMeasure("12.5%")).toEqual({ value: 12.5, unit: "%" });
    expect(rcMeasure("\u22123")).toEqual({ value: -3, unit: "" });
    for (const not of ["n/a", "", "2026-11-02", "beta", "1 m 4 s"]) expect(rcMeasure(not)).toBeNull();
  });

  test("a sortable column compares numbers, then text, and falls back by column", () => {
    const { rcCompare } = kitHelpers();
    expect(rcCompare("9 s", "10 s", "number")).toBeLessThan(0);
    expect(rcCompare("1,234", "999", "number")).toBeGreaterThan(0);
    // A column with a non-measurement in it still sorts, by the locale's order.
    expect(rcCompare("v1.9", "v1.10", "number")).toBeLessThan(0);
    expect(rcCompare("Beta", "alpha", "text")).toBeGreaterThan(0);
    expect(rcCompare("7 s", "alpha", "number")).toBeLessThan(0);
    expect(rcCompare("same", "same", "number")).toBe(0);
  });

  test("a sparkline reads its numbers from the attribute, commas or spaces", () => {
    const { rcSparkValues } = kitHelpers();
    expect(rcSparkValues("1,4,2,8")).toEqual([1, 4, 2, 8]);
    expect(rcSparkValues("1 4 2")).toEqual([1, 4, 2]);
    expect(rcSparkValues("3, n/a, 5")).toEqual([3, 5]);
    expect(rcSparkValues("")).toEqual([]);
  });

  test("a sparkline spans its box, flat when every value matches", () => {
    const { rcSparkPoints } = kitHelpers();
    const points = rcSparkPoints([0, 5, 10], 100, 30);
    const [first, middle, last] = points.split(" ").map((p) => p.split(",").map(Number));
    expect(first?.[0]).toBe(2);
    expect(last?.[0]).toBe(98);
    // The lowest value is at the bottom, the highest at the top, y grows down.
    expect(first?.[1]).toBe(28);
    expect(last?.[1]).toBe(2);
    expect(middle?.[1]).toBe(15);
    expect(rcSparkPoints([3, 3], 100, 30)).toBe("2,15 98,15");
  });

  test("a view sends at most one message every 2 seconds", () => {
    const gate = new SendGate();
    expect(gate.allow(10_000)).toBe(true);
    expect(gate.allow(11_000)).toBe(false);
    expect(gate.allow(12_001)).toBe(true);
  });

  test("a streaming update goes out once per frame and per 100 ms, the last one winning", () => {
    const sent: string[] = [];
    const scheduled: { run: () => void; ms: number }[] = [];
    let now = 0;
    const pump = new StreamPump(
      (html) => sent.push(html),
      100,
      () => now,
      (run, ms) => scheduled.push({ run, ms }),
    );
    pump.push("a");
    pump.push("ab");
    pump.push("abc");
    expect(scheduled.map((s) => s.ms)).toEqual([0]);
    scheduled.shift()!.run();
    expect(sent).toEqual(["abc"]);
    // Within the gap it waits out the rest of it, then sends what has arrived by then.
    now = 50;
    pump.push("abcd");
    expect(scheduled.map((s) => s.ms)).toEqual([50]);
    scheduled.shift()!.run();
    expect(sent).toEqual(["abc", "abcd"]);
    now = 100;
    pump.push("abcde");
    // Half of the gap has passed since the last one, so it waits the other half.
    expect(scheduled.map((s) => s.ms)).toEqual([50]);
    scheduled.shift()!.run();
    expect(sent).toEqual(["abc", "abcd", "abcde"]);
  });
});

test("the app and the shell serve pages under the same policy", async () => {
  const rust = await Bun.file(new URL("../../src-tauri/src/smartui.rs", import.meta.url)).text();
  expect(rust).toContain(`pub const PAGE_CSP: &str = "${PAGE_CSP}";`);
});
