import { describe, expect, test } from "bun:test";
import { MOCK_RECONNECT_HTML, MOCK_RECONNECT_SHORTHAND } from "../acp/mock";
import { blockTitle, splitReply } from "./parse";
import { cells, expandShorthand, hasShorthand, inline } from "./shorthand";

const body = (chunks: string[]) => {
  const ui = splitReply(chunks.join("")).find((segment) => segment.kind === "ui");
  return ui && ui.kind === "ui" ? ui.source : "";
};

describe("shorthand", () => {
  test("a view without shorthand is returned untouched", () => {
    const html = '<title>T</title>\n<div class="rc-bars">\n- not an item\n</div>';
    expect(hasShorthand(html)).toBe(false);
    expect(expandShorthand(html)).toBe(html);
  });

  test("bars scale to the largest value, keep the shown text and carry the tip", () => {
    const out = expandShorthand(
      ":: bars Why\n- Drop | 75 | up to 75 s | SILENCE_LIMIT\n- Ping | 15\n- Auth | 30 {bad}",
    );
    expect(out).toContain("<h3>Why</h3>");
    expect(out).toContain(
      '<div class="rc-bar" data-rc-tip="SILENCE_LIMIT"><span>Drop</span><i style="width:100%"></i><b>up to 75 s</b></div>',
    );
    expect(out).toContain('<i style="width:20%"></i><b>15</b>');
    expect(out).toContain('<div class="rc-bar rc-bad"><span>Auth</span><i style="width:40%"></i>');
  });

  test("percentages are widths as written", () => {
    expect(expandShorthand(":: bars\n- A | 40%\n- B | 10%")).toContain('<i style="width:40%"></i><b>40%</b>');
  });

  test("stats, steps, list and cards", () => {
    expect(expandShorthand(":: stats\n- 128 | Runs | +12 {ok}")).toBe(
      '<div class="rc-stats"><div class="rc-stat rc-ok"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span><span class="rc-stat-note">+12</span></div></div>',
    );
    expect(expandShorthand(":: steps\n- [x] Built\n- [>] Testing\n- [!] Lint\n- [ ] Ship\n- Note")).toBe(
      '<ol class="rc-steps"><li class="rc-step rc-done">Built</li><li class="rc-step rc-active">Testing</li><li class="rc-step rc-failed">Lint</li><li class="rc-step rc-todo">Ship</li><li class="rc-step">Note</li></ol>',
    );
    expect(expandShorthand(":: list\n- Two tools | Extend the old one.\n- Just prose")).toBe(
      '<ul class="rc-list"><li class="rc-item"><b>Two tools</b><p>Extend the old one.</p></li><li class="rc-item rc-prose">Just prose</li></ul>',
    );
    expect(expandShorthand(":: cards\n- Fast | Small, one PR. {info}")).toBe(
      '<div class="rc-grid rc-cards"><div class="rc-card rc-info"><b>Fast</b><p>Small, one PR.</p></div></div>',
    );
  });

  test("a tint may start or end a title cell, and a third cell is a badge or a value", () => {
    expect(expandShorthand(":: cards\n- Tables {ok} | Copy as CSV. | Small")).toBe(
      '<div class="rc-grid rc-cards"><div class="rc-card rc-ok"><b>Tables</b><span class="rc-badge">Small</span><p>Copy as CSV.</p></div></div>',
    );
    expect(expandShorthand(":: cards\n- {warn} Split import | Static import breaks lazy chunk.")).toBe(
      '<div class="rc-grid rc-cards"><div class="rc-card rc-warn"><b>Split import</b><p>Static import breaks lazy chunk.</p></div></div>',
    );
    expect(expandShorthand(":: list\n- Lazy start {info} | `MAX_LIVE` 6 | done")).toBe(
      '<ul class="rc-list"><li class="rc-item rc-info"><b>Lazy start</b><span class="rc-item-value">done</span><p><code>MAX_LIVE</code> 6</p></li></ul>',
    );
  });

  test("actions are chips that send their message", () => {
    expect(expandShorthand(":: actions\n- Do 1 and 2 | Do **1** and 2\n- Plan 4")).toBe(
      '<div class="rc-row rc-gap-2 rc-actions"><button class="rc-chip" data-rc-send="Do 1 and 2">Do 1 and 2</button><button class="rc-chip" data-rc-send="Plan 4">Plan 4</button></div>',
    );
  });

  test("a table takes its first row as the heading, badges tinted cells and sorts past eight rows", () => {
    const small = expandShorthand(
      ":: table\n| Check | Result | Status |\n|---|---|---|\n| tests | 24 pass | {ok} Ready |\n| lint | 1 warn | Split {warn} |\n| release | <code>{ok|warn|bad}</code> | {bad} |",
    );
    expect(small).toBe(
      '<table class="rc-table"><thead><tr><th>Check</th><th>Result</th><th>Status</th></tr></thead><tbody>' +
        '<tr><td>tests</td><td>24 pass</td><td><span class="rc-badge rc-ok">Ready</span></td></tr>' +
        '<tr><td>lint</td><td>1 warn</td><td><span class="rc-badge rc-warn">Split</span></td></tr>' +
        '<tr><td>release</td><td><code>{ok|warn|bad}</code></td><td><span class="rc-badge rc-bad">bad</span></td></tr>' +
        "</tbody></table>",
    );
    const rows = Array.from({ length: 9 }, (_, i) => `r${i} | ${i}`).join("\n");
    expect(expandShorthand(`:: table\nName | N\n${rows}`)).toContain('<table class="rc-table" data-rc-sort>');
  });

  test("callout, paragraph, heading, title and sources", () => {
    expect(expandShorthand(":: callout warn Careful.\nThe cap is `5|10` s.")).toBe(
      '<div class="rc-callout rc-warn"><b>Careful.</b> The cap is <code>5|10</code> s.</div>',
    );
    expect(expandShorthand(":: p\nOne line\nand **two**.")).toBe("<p>One line and <b>two</b>.</p>");
    expect(expandShorthand(":: title Runs <today>\n:: h Next")).toBe(
      "<title>Runs &lt;today&gt;</title>\n<h3>Next</h3>",
    );
    expect(expandShorthand(':: sources\n- Docs | https://example.com/?a="b"')).toBe(
      '<details class="rc-details rc-sources"><summary>Sources</summary><ol><li><a href="https://example.com/?a=&quot;b&quot;">Docs</a></li></ol></details>',
    );
  });

  test("HTML around and between blocks passes through, and a script body is never read", () => {
    const source = [
      '<div class="rc-dash">',
      ":: stats",
      "- 1 | One",
      "</div>",
      "<script>",
      ":: bars",
      "- not | 1",
      "</script>",
    ].join("\n");
    const out = expandShorthand(source);
    expect(out.split("\n")[0]).toBe('<div class="rc-dash">');
    expect(out).toContain('<span class="rc-stat-value">1</span>');
    expect(out).toContain("</div>\n<script>\n:: bars\n- not | 1\n</script>");
  });

  test("a blank line ends a block and a wrapped item continues", () => {
    const out = expandShorthand(":: list\n- A | first\n  continued\n\nafter");
    expect(out).toBe(
      '<ul class="rc-list"><li class="rc-item"><b>A</b><p>first continued</p></li></ul>\n\nafter',
    );
  });

  test("an unknown name stays text", () => {
    expect(expandShorthand(":: chart\n- a | 1")).toBe(":: chart\n- a | 1");
  });

  test("cells split outside backticks and HTML tags/code only, and inline keeps HTML", () => {
    expect(cells("a | `b|c` | d \\| e", 3)).toEqual(["a", "`b|c`", "d | e"]);
    expect(cells('a | <code>{ok|warn|bad}</code> | <a title="x|y">z</a>', 3)).toEqual([
      "a",
      "<code>{ok|warn|bad}</code>",
      '<a title="x|y">z</a>',
    ]);
    expect(cells("a | b | c", 2)).toEqual(["a", "b | c"]);
    expect(inline("<span class=rc-badge>x</span> `<b>`")).toBe(
      "<span class=rc-badge>x</span> <code>&lt;b&gt;</code>",
    );
  });

  test("the title is read from the shorthand", () => {
    expect(blockTitle(":: title Reconnect timing\n:: p Hi")).toBe("Reconnect timing");
  });

  test("the reconnect fixture draws the same kit as its HTML and costs far less", () => {
    const html = body(MOCK_RECONNECT_HTML);
    const short = body(MOCK_RECONNECT_SHORTHAND);
    const out = expandShorthand(short);
    for (const kit of [
      "rc-bars",
      'data-rc-tip="SILENCE_LIMIT',
      "rc-step rc-active",
      "rc-callout rc-info",
      "rc-card",
      "rc-sources",
    ]) {
      expect(out).toContain(kit);
    }
    expect(short.length).toBeLessThan(html.length * 0.65);
  });
});
