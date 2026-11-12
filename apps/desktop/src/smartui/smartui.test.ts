import { describe, expect, test } from "bun:test";
import { MOCK_CHARTS, MOCK_KIT, MOCK_VIEW } from "../acp/mock";
import {
  AutoRun,
  claimLive,
  clearHalt,
  HoverSize,
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
  previewable,
  runtimeScript,
  usesChart,
  varsCss,
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
    expect(clampHeight(100.01)).toBe(101);
    expect(clampHeight(100)).toBe(100);
    expect(clampHeight(10)).toBe(48);
    expect(clampHeight(5000)).toBe(5000);
    expect(blockTitle("<title>Runs</title><p>")).toBe("Runs");
    expect(viewMessage("  hi ")).toBe("hi");
    expect(viewMessage("x".repeat(2001))).toBeNull();
    expect(viewMessage(42)).toBeNull();
  });

  test("a view that names the library is not previewed, and a plain one is", () => {
    const chart =
      '<title>C</title><div class="rc-chart"><canvas></canvas></div>\n<script src="rustcode-lib:chart.js"></script>';
    expect(previewable(chart)).toBe(false);
    expect(previewable('<title>T</title><table class="rc-table">…</table>')).toBe(true);
    // A tag that is not the bundled name is not the library, and does not stop a preview.
    expect(previewable('<script src="rustcode-lib:other.js"></script>')).toBe(true);
    expect(previewable("```js\nnew Chart(c)\n```")).toBe(true);
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

  test("the page follows the app's radius, fonts and series, on every platform", () => {
    const tokens: Record<string, string> = {
      "--rc-accent": "#ff8800",
      "--rc-radius-md": "4px",
      "--rc-radius-sm": "3px",
      "--radius-full": "9999px",
      "--text-callout": "14px",
      "--text-sub": "12px",
      "--text-caption": "11px",
      "--text-title3": "17px",
      "--text-title": "26px",
    };
    const theme = pageTheme("dark", (name) => tokens[name] ?? "#000", {
      sans: "Inter, sans-serif",
      mono: "JetBrains Mono, monospace",
    });
    expect(theme.vars["--rc-radius"]).toBe("4px");
    expect(theme.vars["--rc-radius-sm"]).toBe("3px");
    expect(theme.vars["--rc-radius-pill"]).toBe("9999px");
    expect(theme.vars["--rc-text-size"]).toBe("14px");
    expect(theme.vars["--rc-text-size-value"]).toBe("26px");
    expect(theme.vars["--rc-font"]).toBe("Inter, sans-serif");
    expect(theme.vars["--rc-mono"]).toBe("JetBrains Mono, monospace");
    // The accent leads the series, so charts follow a custom theme too.
    expect(theme.vars["--rc-series-1"]).toBe("#ff8800");
    expect(theme.vars["--rc-series-2"]).toBe("#ff9f0a");
    // A theme change posts the same variables the page started with.
    const css = varsCss(theme);
    expect(css).toContain("--rc-radius: 4px;");
    expect(css).toContain("--rc-font: Inter, sans-serif;");
    expect(css).toContain("--rc-series-1: #ff8800;");
  });

  test("a hover never changes the size the app reserved", () => {
    const theme = pageTheme("light", () => "#000");
    const script = runtimeScript("tok", theme);
    // Measured from the in-flow root, so an absolutely placed tip or bar label is not counted.
    expect(script).toContain('getElementById("rc-root")');
    expect(script).not.toContain("body.scrollHeight");
    // The page says when the pointer is in it, and the app holds a height that moves then.
    expect(script).toContain('type: "hover"');
    expect(script).toContain('addEventListener("pointerout"');
    const page = buildPage("<p>x</p>", "tok", theme);
    expect(page).toContain('<div id="rc-root"><p>x</p></div>');
    // A reveal, not a layout change: no scrollbar and no display swap under the pointer.
    expect(BASE_CSS).toContain("html { overflow: hidden; }");
    // The gap is between drawn blocks: a leading <title> must not hand the first block a margin
    // that collapses out of the root and leaves the measured height 12px short.
    expect(BASE_CSS).toContain("#rc-root, #rc-stream { display: flow-root; }");
    expect(BASE_CSS).toContain(
      "#rc-root > :not(title, style, script, link, meta, template) ~ :not(title, style, script, link, meta, template)",
    );
    expect(BASE_CSS).not.toContain("#rc-root > * + *");
    expect(BASE_CSS).toContain(
      ".rc-tip:hover > span, .rc-tip:focus > span { visibility: visible; opacity: 1; }",
    );
    expect(BASE_CSS).toContain(".rc-bar[data-rc-tip]:hover::after { visibility: visible; opacity: 1; }");
    expect(BASE_CSS).not.toContain(".rc-tip > span { display: none");
  });

  test("a height measured under the pointer waits for it to leave", () => {
    const gate = new HoverSize(300);
    expect(gate.measure(300)).toBe(300);
    gate.hover(true);
    // A tip near the bottom opens: not drawn until the pointer is gone.
    expect(gate.measure(360)).toBeNull();
    expect(gate.measure(340)).toBeNull();
    expect(gate.value).toBe(300);
    // Moving to another tip still does not move the chat below.
    expect(gate.hover(true)).toBeNull();
    expect(gate.hover(false)).toBeNull();
    expect(gate.value).toBe(300);
    expect(gate.measure(300)).toBe(300);
    // A click may legitimately open a panel under the pointer.
    gate.hover(true);
    expect(gate.measure(420, true)).toBe(420);
    expect(gate.value).toBe(420);
  });

  test("one type scale and one chart height", () => {
    // A view grows to its content: the page has no body cap, so only `.rc-scroll` or the
    // view's own CSS bounds anything.
    expect(BASE_CSS).not.toContain("max-height: 720px");
    expect(BASE_CSS).toContain("html { overflow: hidden; }");
    expect(BASE_CSS).toContain(".rc-scroll { max-height: 320px; overflow: auto; }");
    expect(BASE_CSS).toContain("var(--rc-text-size-value)");
    expect(BASE_CSS).toContain("var(--rc-text-size-xs)");
    expect(BASE_CSS).toContain(".rc-chart { position: relative; height: 200px; }");
    expect(BASE_CSS).toContain(".rc-chart[data-size=sm]");
    expect(BASE_CSS).toContain(".rc-chart[data-size=lg]");
  });

  test("the mock's views are kit only, on the theme and the rhythm", () => {
    for (const lines of [MOCK_VIEW, MOCK_KIT, MOCK_CHARTS]) {
      const source = splitReply(lines.join(""))
        .filter((p) => p.kind === "ui")
        .map((p) => p.source)
        .join("");
      expect(source).not.toMatch(/style="[^"]*(color|font-size|font-family|height)/i);
      expect(source).not.toMatch(/#[0-9a-f]{3,6}\b/i);
    }
  });

  test("list items wrap descriptions and keep badges intact", () => {
    expect(BASE_CSS).toContain(".rc-item:has(> div, > p) { justify-content: flex-start; }");
    expect(BASE_CSS).toContain(".rc-item > div, .rc-item > p { flex: 0 0 100%; margin: 0; }");
    expect(BASE_CSS).toContain(".rc-item > .rc-badge, .rc-item > .rc-chip { flex: none; }");
    expect(BASE_CSS).toContain(".rc-item > * { min-width: 0; overflow-wrap: anywhere; }");
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
    expect(script).toContain('data.type === "focus-modality"');
    expect(script).toContain('event.key === "Tab"');
    expect(script).toContain('addEventListener("pointerdown"');
    expect(script).not.toContain('window.addEventListener("blur"');
    expect(BASE_CSS).toContain(":root[data-keyboard-nav] :focus-visible");
    expect(BASE_CSS).toContain(":root[data-keyboard-nav] :is(input, select, textarea):focus-visible");
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

  test("the page's fields are left to the block, not to the system's text helpers", () => {
    const theme = pageTheme("light", () => "#000");
    // The frame is its own document: the app's sweep (lib/quietNative.ts) never sees it, so the
    // page itself turns the helpers off, where every field the block draws inherits them.
    for (const page of [buildPage("<p>x</p>", "tok", theme), buildStreamPage("tok", theme)]) {
      expect(page).toContain('spellcheck="false"');
      expect(page).toContain('autocorrect="off"');
      expect(page).toContain('autocapitalize="off"');
      expect(page).toContain('writingsuggestions="false"');
      expect(page).toMatch(/<body [^>]*spellcheck="false"/);
    }
  });

  test("a streaming fragment is closed, and its script stays inert", () => {
    const theme = pageTheme("light", () => "#000");
    const page = buildStreamPage("tok", theme);
    expect(page).toContain(`content="${PAGE_CSP}"`);
    expect(page).toContain('<div id="rc-stream"></div>');
    // A streamed fragment runs no script, so a canvas there can never draw.
    expect(page).toContain("#rc-stream .rc-chart > canvas");
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

  test("a page that names the library gets it inlined, before the view's script", async () => {
    const theme = pageTheme("dark", () => "#000");
    const library = "window.Chart = function Chart() {}; // the bundled library";
    const view = 'new Chart(c, { type: "line" });';
    const source = `<title>C</title><div class="rc-chart"><canvas></canvas></div>\n<script src="rustcode-lib:chart.js"></script>\n<script>${view}</script>`;
    const page = buildPage(source, "tok", theme, library);
    // The tag is replaced by the library itself, once, and never left to be fetched.
    expect(page).not.toContain("rustcode-lib:chart.js");
    expect(page.split(library)).toHaveLength(2);
    // Library, then the theme, then the block's own script: the order Chart needs.
    expect(page.indexOf(library)).toBeLessThan(page.indexOf("rcSeries"));
    expect(page.indexOf("rcSeries")).toBeLessThan(page.indexOf(view));
    // Without the library the tag is dropped rather than left as a request the CSP blocks.
    const bare = buildPage(source, "tok", theme);
    expect(bare).not.toContain("rustcode-lib:chart.js");
    expect(bare).not.toContain(library);
    expect(() => new Function(CHART_THEME_SCRIPT)).not.toThrow();
  });

  /** `CHART_THEME_SCRIPT` against a stub Chart, with a canvas that rejects bad colours. */
  function runChartTheme(options: { vars?: (name: string) => string; reduced?: boolean } = {}) {
    const stops: string[] = [];
    const defaults: Record<string, any> = {
      color: "",
      borderColor: "#ffffff0f",
      font: { family: "", size: 0 },
      responsive: false,
      maintainAspectRatio: true,
      resizeDelay: 100,
      animation: { duration: 1000, easing: "easeOutQuart" },
      plugins: {
        colors: { enabled: true },
        tooltip: {},
        legend: { labels: {}, onHover: null, onLeave: null },
      },
      scale: { grid: { color: "" } },
      elements: { line: {}, point: {}, bar: {} },
    };
    const registered: any[] = [];
    const Chart = {
      defaults,
      instances: {} as Record<string, unknown>,
      register: (plugin: unknown) => registered.push(plugin),
    };
    const gradient = {
      addColorStop: (_at: number, colour: string) => {
        // A canvas throws on a colour it cannot parse, which aborts the whole draw.
        expect(colour).toMatch(/^(rgba?\(|transparent$)/);
        stops.push(colour);
      },
    };
    const vars = options.vars ?? (() => "");
    // The page has Chart as a global; so does this.
    new Function(
      "window",
      "document",
      "getComputedStyle",
      "matchMedia",
      "requestAnimationFrame",
      "Chart",
      CHART_THEME_SCRIPT,
    )(
      { Chart },
      { documentElement: {} },
      () => ({ colorScheme: "dark", getPropertyValue: vars }),
      () => ({ matches: options.reduced ?? false }),
      undefined,
      Chart,
    );
    const chart = (type: string, labels: string[], datasets: any[]) => ({
      config: { type, options: {} as Record<string, any> },
      data: { labels, datasets },
      chartArea: { top: 0, bottom: 100 },
      ctx: { createLinearGradient: () => gradient },
      width: 100,
      height: 100,
      options: {} as Record<string, any>,
    });
    return { defaults, stops, gradient, chart, plugin: registered.find((p) => p.id === "rcSeries") };
  }

  test("the chart theme runs against Chart, and fills a line and a donut", () => {
    const { defaults, stops, gradient, chart, plugin } = runChartTheme({
      vars: (name) => (name === "--rc-series-1" ? "#0a84ff" : name === "--rc-series-2" ? "#ff9f0a" : ""),
    });
    // Themed defaults, not the library's own.
    expect(defaults.responsive).toBe(true);
    expect(defaults.maintainAspectRatio).toBe(false);
    expect(defaults.resizeDelay).toBe(0);
    // No tweening: a hover or a revealed chart answers at once.
    expect(defaults.animation).toBe(false);
    expect(defaults.plugins.tooltip.usePointStyle).toBe(true);
    expect(defaults.plugins.legend.labels.usePointStyle).toBe(true);
    expect(defaults.elements.point.hitRadius).toBe(10);
    expect(typeof defaults.plugins.legend.onHover).toBe("function");

    expect(plugin).toBeDefined();
    expect(typeof plugin.beforeUpdate).toBe("function");
    // A chart created while its box is still 0 x 0 is measured again on a later frame.
    expect(typeof plugin.afterRender).toBe("function");
    expect(() => plugin.afterRender({ width: 0, height: 0, resize: () => undefined })).not.toThrow();

    // A line is filled with a gradient of its own colour, from colour to nothing.
    const line = chart("line", ["Mon", "Tue"], [{ label: "Runs", data: [1, 2] }]);
    plugin.beforeUpdate(line);
    const lineSet = line.data.datasets[0] as any;
    expect(lineSet.borderColor).toBe("#0a84ff");
    expect(lineSet.fill).toBe(true);
    expect(typeof lineSet.backgroundColor).toBe("function");
    expect(lineSet.backgroundColor({ chart: line, dataset: lineSet })).toBe(gradient);
    expect(stops).toEqual(["rgba(10, 132, 255, 0.3)", "rgba(10, 132, 255, 0)"]);
    // The view's own interaction wins; otherwise ours answers along the axis.
    expect(line.config.options.interaction).toEqual({ mode: "index", intersect: false });

    // A donut gets one series colour per slice, and answers to the pointer.
    const donut = chart("doughnut", ["a", "b"], [{ data: [1, 2] }]);
    plugin.beforeUpdate(donut);
    expect((donut.data.datasets[0] as any).backgroundColor).toEqual(["#0a84ff", "#ff9f0a"]);
    expect(donut.config.options.interaction).toEqual({ mode: "nearest", intersect: true });

    // A dataset the view coloured itself is left exactly as it is.
    const own = chart("bar", ["a"], [{ data: [1], backgroundColor: "red", borderColor: "blue" }]);
    plugin.beforeUpdate(own);
    expect((own.data.datasets[0] as any).backgroundColor).toBe("red");
    expect((own.data.datasets[0] as any).borderColor).toBe("blue");
  });

  test("a missing series variable never hands a canvas an unparseable colour", () => {
    const { stops, chart, plugin } = runChartTheme();
    const line = chart("line", ["a"], [{ data: [1] }]);
    expect(() => plugin.beforeUpdate(line)).not.toThrow();
    const dataset = line.data.datasets[0] as any;
    // Falls back to the themed border, so the line is still there rather than invisible.
    expect(dataset.borderColor).toBe("#8e8e93");
    expect(() => dataset.backgroundColor({ chart: line, dataset })).not.toThrow();
    // Still a real colour, so the chart is drawn rather than aborting the frame.
    expect(stops).toEqual(["rgba(142, 142, 147, 0.3)", "rgba(142, 142, 147, 0)"]);
  });

  test("reduce motion keeps the chart's animation off", () => {
    const { defaults } = runChartTheme({ reduced: true });
    expect(defaults.animation).toBe(false);
  });

  test("the presets draw with the real Chart.js", async () => {
    // The page's own globals, borrowed for one test: the library is the bundled
    // build, a canvas is a stub, and the animation frames are pumped by hand.
    const g = globalThis as any;
    const saved = new Map<string, unknown>();
    for (const key of [
      "window",
      "document",
      "getComputedStyle",
      "matchMedia",
      "navigator",
      "requestAnimationFrame",
      "cancelAnimationFrame",
      "devicePixelRatio",
      "MutationObserver",
      "ResizeObserver",
      "Chart",
    ])
      saved.set(key, g[key]);
    const realNow = Date.now;
    let clock = realNow();

    const frames: ((at: number) => void)[] = [];
    const stops: string[] = [];
    const vars: Record<string, string> = {
      "--rc-text": "#ededed",
      "--rc-separator": "#ffffff0f",
      "--rc-muted": "#a1a1a1",
      "--rc-font": "sans-serif",
      ...Object.fromEntries(
        [1, 2, 3, 4, 5, 6, 7, 8].map((i) => [
          `--rc-series-${i}`,
          ["#0a84ff", "#ff9f0a", "#30d158", "#ff453a", "#bf5af2", "#40c8e0", "#ffd60a", "#ff375f"][i - 1]!,
        ]),
      ),
    };
    const makeCanvas = () => {
      const ctx: any = {
        canvas: null,
        createLinearGradient: () => ({
          addColorStop: (_at: number, colour: string) => {
            // A canvas rejects a colour it cannot parse, so the preset must not hint one.
            expect(colour).toMatch(/^(rgba?\()/);
            stops.push(colour);
          },
        }),
        measureText: (text: string) => ({ width: String(text).length * 6 }),
        lineWidth: 1,
      };
      for (const method of [
        "save",
        "restore",
        "beginPath",
        "closePath",
        "moveTo",
        "lineTo",
        "bezierCurveTo",
        "quadraticCurveTo",
        "arc",
        "rect",
        "roundRect",
        "clip",
        "fill",
        "stroke",
        "fillRect",
        "strokeRect",
        "clearRect",
        "translate",
        "rotate",
        "scale",
        "setTransform",
        "resetTransform",
        "setLineDash",
        "fillText",
        "strokeText",
        "drawImage",
        "putImageData",
      ])
        ctx[method] = () => undefined;
      ctx.isPointInPath = () => false;
      ctx.isPointInStroke = () => false;
      ctx.getImageData = () => ({ data: [] });
      ctx.createPattern = () => null;
      const parent = {
        style: {},
        ownerDocument: { defaultView: g },
        getBoundingClientRect: () => ({ width: 720, height: 180, top: 0, left: 0, right: 720, bottom: 180 }),
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      };
      const canvas: any = {
        width: 300,
        height: 150,
        style: {},
        clientWidth: 720,
        clientHeight: 180,
        getAttribute: () => null,
        setAttribute: () => undefined,
        removeAttribute: () => undefined,
        hasAttribute: () => false,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        getContext: () => ctx,
        parentNode: parent,
        ownerDocument: { defaultView: g },
      };
      ctx.canvas = canvas;
      return canvas;
    };
    const canvases = [makeCanvas(), makeCanvas()];
    const empty = () => ({
      style: {},
      appendChild: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      setAttribute: () => undefined,
    });

    try {
      g.window = g;
      g.document = {
        documentElement: {},
        createElement: (name: string) => (name === "canvas" ? makeCanvas() : empty()),
        createElementNS: empty,
        getElementById: () => null,
        querySelector: () => null,
        querySelectorAll: () => [],
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      };
      g.navigator = { userAgent: "bun" };
      g.getComputedStyle = () => ({
        colorScheme: "dark",
        display: "block",
        boxSizing: "border-box",
        position: "relative",
        maxWidth: "none",
        maxHeight: "none",
        getPropertyValue: (name: string) => vars[name] ?? (name.includes("max") ? "none" : "0px"),
      });
      g.matchMedia = () => ({ matches: false, addEventListener: () => undefined });
      g.requestAnimationFrame = (fn: (at: number) => void) => frames.push(fn);
      g.cancelAnimationFrame = () => undefined;
      g.devicePixelRatio = 1;
      g.MutationObserver = class {
        observe() {}
        disconnect() {}
      };
      g.ResizeObserver = class {
        observe() {}
        disconnect() {}
        unobserve() {}
      };
      // The animator reads the clock, so a pumped frame has to move it on.
      Date.now = () => clock;

      const library = await Bun.file(
        new URL("../../node_modules/chart.js/dist/chart.umd.js", import.meta.url),
      ).text();
      new Function(library)();
      const Chart = g.Chart;
      expect(typeof Chart).toBe("function");

      new Function(
        "window",
        "document",
        "getComputedStyle",
        "matchMedia",
        "requestAnimationFrame",
        "Chart",
        CHART_THEME_SCRIPT,
      )(g, g.document, g.getComputedStyle, g.matchMedia, g.requestAnimationFrame, Chart);

      const line: any = new Chart(canvases[0], {
        type: "line",
        data: { labels: ["Mon", "Tue", "Wed"], datasets: [{ label: "Runs", data: [1, 2, 3] }] },
      });
      const donut: any = new Chart(canvases[1], {
        type: "doughnut",
        data: { labels: ["a", "b"], datasets: [{ data: [1, 2] }] },
      });

      // A stub canvas is not in a document, so tell Chart.js it has a size and is
      // attached; from here the pipeline is the page's own.
      for (const chart of [line, donut]) {
        chart.attached = true;
        chart.resize(720, 180);
        chart.update();
      }

      // Pump the first frames: a colour tween with no interpolator throws right here.
      for (let at = 0; at < 40 && frames.length; at++) {
        clock += 20;
        frames.shift()?.(clock);
      }

      // Both charts laid out and animated rather than stopping on their first frame.
      expect(line.chartArea?.width ?? 0).toBeGreaterThan(100);
      expect(donut.chartArea?.width ?? 0).toBeGreaterThan(100);
      // The line was filled with a gradient of its own colour.
      expect(stops.length).toBeGreaterThan(0);
      expect(stops[0]).toBe("rgba(10, 132, 255, 0.3)");
      line.destroy();
      donut.destroy();
    } finally {
      Date.now = realNow;
      for (const [key, value] of saved) {
        if (value === undefined) delete g[key];
        else g[key] = value;
      }
    }
  });

  test("the chart theme follows the variables, in both themes", () => {
    expect(CHART_THEME_SCRIPT).toContain("--rc-series-");
    expect(CHART_THEME_SCRIPT).toContain("usePointStyle");
    expect(CHART_THEME_SCRIPT).toContain("hoverBackgroundColor");
    expect(CHART_THEME_SCRIPT).toContain("createLinearGradient");
    expect(CHART_THEME_SCRIPT).toContain("__rcThemeCharts");
    // No raw colour but the tooltip's ink over --rc-text and the neutral a chart
    // falls back to when a variable is missing: everything else is a variable.
    expect(CHART_THEME_SCRIPT.replace(/"#000"|"#fff"|"#8e8e93"/g, "")).not.toMatch(/#[0-9a-f]{3,8}\b/i);
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

  test("a source shows the site it is on", () => {
    const { rcHost } = kitHelpers();
    expect(rcHost("https://www.streamscharts.com/news/q3")).toBe("streamscharts.com");
    expect(rcHost("http://user@Example.org:8080/a?b")).toBe("example.org");
    for (const not of [null, "", "/relative", "mailto:a@b.c", "javascript:alert(1)"])
      expect(rcHost(not)).toBe("");
  });

  test("a link opens in the browser, never in the frame, and a press marks the next size", () => {
    const script = runtimeScript(
      "tok",
      pageTheme("light", () => ""),
    );
    expect(script).toContain('post({ type: "open", url: href.slice(0, 2048) })');
    expect(script).toContain("event.preventDefault();");
    expect(script).toContain('document.addEventListener("auxclick", openLink);');
    expect(script).toContain("interacted: performance.now() - gestureAt < 1000");
    expect(script).toContain('document.addEventListener("toggle"');
    expect(() => new Function(script)).not.toThrow();
  });

  test("sources fold quietly and charts answer the pointer at once", () => {
    expect(BASE_CSS).toContain(".rc-sources { border-bottom: 0;");
    expect(BASE_CSS).toContain(".rc-sources-host {");
    expect(CHART_THEME_SCRIPT).toContain("d.animation = false;");
    expect(CHART_THEME_SCRIPT).not.toContain("duration: 180");
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
