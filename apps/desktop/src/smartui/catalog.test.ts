import { describe, expect, test } from "bun:test";
import { buildPage, previewable, usesChart } from "./page";
import { BLOCKS, CHART_LIBRARY_TAG, expandShorthand } from "./shorthand";

const theme = {
  mode: "light" as const,
  vars: {},
};

/** The chart config a `:: chart` block wrote into its canvas. */
const chartConfig = (html: string) => {
  const match = /data-rc-chart="([^"]*)"/.exec(html);
  if (!match) throw new Error("no chart");
  return JSON.parse(
    match[1]!
      .replace(/&quot;/g, '"')
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&"),
  );
};

describe("shorthand catalog", () => {
  test("a chart is a table of data, with no script of its own", () => {
    const out = expandShorthand(
      ":: chart line Runs per day | size=lg | line=25 normal | band=10-30 | end\nday | Runs | Failed\nMon | 25 | 1\nTue | 1,027 | 0",
    );
    expect(out.startsWith(CHART_LIBRARY_TAG)).toBe(true);
    expect(out).toContain("<h3>Runs per day</h3>");
    expect(out).toContain('data-size="lg"');
    expect(out).not.toContain("new Chart");
    const config = chartConfig(out);
    expect(config.type).toBe("line");
    expect(config.data.labels).toEqual(["Mon", "Tue"]);
    expect(config.data.datasets).toEqual([
      { label: "Runs", data: [25, 1027] },
      { label: "Failed", data: [1, 0] },
    ]);
    expect(config.options.plugins.legend.display).toBe(true);
    expect(config.options.plugins.rcMarks).toEqual({
      band: [10, 30],
      line: 25,
      lineLabel: "normal",
      end: true,
    });
  });

  test("a chart without a heading row names no series", () => {
    const config = chartConfig(expandShorthand(":: chart bar Slowest\n- engine | 42\n- tui | 10.5"));
    expect(config.data.labels).toEqual(["engine", "tui"]);
    expect(config.data.datasets).toEqual([{ label: "", data: [42, 10.5] }]);
  });

  test("chart kinds: stacked and horizontal bars, and a donut with its total and legend", () => {
    const stacked = chartConfig(expandShorthand(":: chart stacked\nweek | ok | failed\nW1 | 3 | 1"));
    expect(stacked.type).toBe("bar");
    expect(stacked.options.scales.x.stacked).toBe(true);
    expect(stacked.options.plugins.legend.display).toBe(true);
    const hbar = chartConfig(expandShorthand(":: chart hbar\ncrate | s\nengine | 42"));
    expect(hbar.options.indexAxis).toBe("y");
    expect(hbar.options.plugins.legend.display).toBe(false);
    const donut = expandShorthand(":: chart donut Runs by suite\nsuite | runs\n- engine | 64\n- auth | 19");
    expect(chartConfig(donut).type).toBe("doughnut");
    expect(donut).toContain('<div class="rc-donut-total"><b>83</b><span>runs</span></div>');
    expect(donut).toContain('<li style="--rc-tint:var(--rc-series-2)"><span>auth</span><b>19</b></li>');
  });

  test("a chart block loads the bundled library once and still streams", () => {
    const source = "<title>T</title>\n:: chart bar A\nx | y\na | 1\n\n:: chart line B\nx | y\na | 1";
    expect(usesChart(source)).toBe(true);
    expect(previewable(source)).toBe(true);
    expect(previewable('<script src="rustcode-lib:chart.js"></script><canvas></canvas>')).toBe(false);
    const page = buildPage(source, "t", theme as never, "/*chartjs*/");
    expect(page.match(/\/\*chartjs\*\//g)?.length).toBe(1);
    expect(page).not.toContain("rustcode-lib:chart.js");
    expect(page).toContain("canvas[data-rc-chart]");
  });

  test("hero, head, progress and stats with a sparkline", () => {
    expect(expandShorthand(":: head Merge traffic | Updated 20:39")).toBe(
      '<div class="rc-head"><b>Merge traffic</b><span>Updated 20:39</span></div>',
    );
    expect(expandShorthand(":: hero Merged today\n- 503 | 3.4x {ok} | A normal day is **150**.")).toBe(
      '<div class="rc-hero"><span class="rc-label">Merged today</span><span class="rc-hero-value">503</span><span class="rc-hero-delta rc-ok">3.4x</span><p class="rc-lead">A normal day is <b>150</b>.</p></div>',
    );
    const meters = expandShorthand(":: progress\n- EU | 72% | 3 regions {ok}\n- US | 3/5\n- Asia | 140");
    expect(meters).toContain('<div class="rc-meter rc-ok">');
    expect(meters).toContain('<span style="width:72%"></span>');
    expect(meters).toContain('<span style="width:60%"></span>');
    expect(meters).toContain('<span style="width:100%"></span>');
    const stats = expandShorthand(":: stats\n- 1,058 | Last 7 days | -19% {bad} | 3, 5, 4, 8");
    expect(stats).toContain('<div class="rc-stat rc-bad">');
    expect(stats).toContain('<span class="rc-spark" data-values="3,5,4,8"></span>');
  });

  test("rows: key-value, compare marks, a heatmap's shades, a timeline, findings and files", () => {
    expect(expandShorthand(":: kv\n- Branch | `main`\n- CI | green {ok}")).toBe(
      '<dl class="rc-kv"><dt>Branch</dt><dd><code>main</code></dd><dt>CI</dt><dd><span class="rc-badge rc-ok">green</span></dd></dl>',
    );
    const compare = expandShorthand(
      ":: compare\n| Feature | A | B |\n|---|---|---|\n| Streaming | yes | partial slow |\n| Forms | no | ✓ |",
    );
    expect(compare).toContain(
      '<td><b>Streaming</b></td><td><span class="rc-mark rc-ok" aria-label="yes">✓</span></td>',
    );
    expect(compare).toContain(
      '<span class="rc-mark rc-warn" aria-label="partial">◐</span> <span class="rc-muted">slow</span>',
    );
    expect(compare).toContain('<span class="rc-mark rc-bad" aria-label="no">✕</span>');
    const heat = expandShorthand(":: heatmap\nday | 0h | 6h\nMon | 3 | 12");
    expect(heat).toContain('style="--rc-heat:52%">12</td>');
    expect(heat).toContain('style="--rc-heat:6%">3</td>');
    expect(expandShorthand(":: timeline\n- 10:02 | Deploy | started {info}")).toBe(
      '<ol class="rc-steps rc-timeline"><li class="rc-step rc-info"><span class="rc-time">10:02</span><b>Deploy</b><p>started</p></li></ol>',
    );
    expect(
      expandShorthand(":: findings\n- High | `a.ts:21` · Codex | The limiter runs late. | Fixed {ok}"),
    ).toBe(
      '<div class="rc-findings"><div class="rc-finding" data-severity="high"><div><code>a.ts:21</code> · Codex</div><p>The limiter runs late.</p><p class="rc-finding-note rc-ok">Fixed</p></div></div>',
    );
    const review = expandShorthand(
      [
        ":: findings",
        "- Low | `b.ts:4` · Gemini | Flaky timer.",
        "- High | `a.ts:21` · Codex | The limiter runs **late**.",
        "- Medium | `c.ts:9` | Shared bucket. | Set aside: by design",
        "- High | `d.ts:1` | Done already. | Fixed {ok}",
      ].join("\n"),
    );
    expect(review).toContain(
      '<div class="rc-finding-fix"><button class="rc-chip" data-rc-send="Fix this finding: a.ts:21 — The limiter runs late.">Fix</button></div>',
    );
    expect(review.match(/>Fix<\/button>/g)).toHaveLength(2);
    expect(review).toContain(
      'data-rc-send="Fix these open findings:\n- high · a.ts:21: The limiter runs late.\n- low · b.ts:4: Flaky timer.">Fix all open</button>',
    );
    expect(expandShorthand(":: findings\n- High | `a.ts:1` | One.")).not.toContain("Fix all open");
    expect(expandShorthand(":: files\n- src/a.ts | +12 -3 | new")).toContain(
      '<span><span class="rc-add">+12</span> <span class="rc-del">−3</span></span>',
    );
  });

  test("tags, a terminal and code keep their text as written", () => {
    expect(expandShorthand(":: tags\n- rust {info}\n- ts")).toBe(
      '<div class="rc-row rc-gap-1 rc-tags"><span class="rc-badge rc-info">rust</span><span class="rc-badge">ts</span></div>',
    );
    expect(expandShorthand(":: term\n$ cargo check\n<ok> done {ok}")).toBe(
      '<div class="rc-term"><div>$ cargo check</div><div class="rc-ok">&lt;ok&gt; done</div></div>',
    );
    expect(expandShorthand(":: code rust\nfn main() {\n    let a = 1 < 2;\n}\n\n:: p After")).toBe(
      '<pre data-lang="rust"><code>fn main() {\n    let a = 1 &lt; 2;\n}</code></pre>\n\n<p>After</p>',
    );
  });

  test("tabs group the blocks between them, and :: end closes the set", () => {
    const out = expandShorthand(":: tab One\n:: p First\n:: tab Two\n:: p Second\n:: end\n:: p After");
    expect(out).toBe(
      [
        '<div class="rc-tabs"><div role="tablist"><button role="tab" data-rc-tab="t1-0">One</button><button role="tab" data-rc-tab="t1-1">Two</button></div>',
        '<section data-rc-panel="t1-0">',
        "<p>First</p>",
        "</section>",
        '<section data-rc-panel="t1-1">',
        "<p>Second</p>",
        "</section></div>",
        "<p>After</p>",
      ].join("\n"),
    );
    expect(expandShorthand(":: tab One\n:: p First")).toContain('data-rc-tab="t1-0">One</button></div>');
  });

  test("a form sends its message with its named fields", () => {
    const out = expandShorthand(
      ":: form Apply these filters | Apply\n- select range | Range | 7d, *30d, 90d\n- choice kind | Kind | all, failed\n- check flaky | Only flaky | on\n- range n | Threshold | 0-100 = 40\n- text q | Filter | name",
    );
    expect(out).toContain('<form class="rc-form" data-rc-send="Apply these filters">');
    expect(out).toContain("<option>7d</option><option selected>30d</option>");
    expect(out).toContain('<input type="radio" name="kind" value="all" checked>');
    expect(out).toContain('<input type="checkbox" name="flaky" checked>');
    expect(out).toContain('min="0" max="100" value="40" data-rc-out><output class="rc-num">40</output>');
    expect(out).toContain('type="text" name="q" placeholder="name"');
    expect(out).toContain("<button data-primary>Apply</button></div></form>");
    expect(out).toContain('<div class="rc-choice">');
  });

  test("a choice with long options is a list and keeps commas inside parentheses", () => {
    const out = expandShorthand(
      ":: form Go | Start\n- choice who | Who | *delegate waves (faster, more usage), only this thread (the default)",
    );
    expect(out).toContain('<div class="rc-choice rc-choice-list">');
    expect(out).toContain('value="delegate waves (faster, more usage)" checked>');
    expect(out).toContain('value="only this thread (the default)">');
  });

  test("details fold a list or a paragraph", () => {
    expect(expandShorthand(":: details Caveats | 2\nThe numbers are from CI only.")).toBe(
      '<details class="rc-details"><summary><span>Caveats</span><span class="rc-muted">2</span></summary><p>The numbers are from CI only.</p></details>',
    );
  });

  test("the catalog the skill and smart_ui_check name", () => {
    expect([...BLOCKS].sort()).toEqual(
      [
        "actions",
        "bars",
        "callout",
        "cards",
        "chart",
        "code",
        "compare",
        "details",
        "divider",
        "end",
        "files",
        "findings",
        "form",
        "h",
        "head",
        "heatmap",
        "hero",
        "kv",
        "list",
        "p",
        "progress",
        "sources",
        "stats",
        "steps",
        "tab",
        "table",
        "tags",
        "term",
        "timeline",
        "title",
      ].sort(),
    );
  });
});
