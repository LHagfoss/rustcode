// design-lint: allow-file (fake link colours in canned data)
// Canned Smart UI replies and link metadata for the mock engine (acp/mock.ts).

import type { JsonObject } from "./jsonrpc";

/** A Smart UI review: findings by severity, two fixed and one set aside with a Reopen chip. */
export const MOCK_REVIEW = [
  "Two fixed, one set aside; the branch is ready to merge.\n\n",
  "```rustcode-ui v1\n<title>Review of the refunds branch</title>\n",
  '<div class="rc-head"><b>Review</b><span class="rc-muted">Codex sol + Gemini 3 Pro · 2 fixed, 1 set aside · 4m 20s</span></div>\n',
  '<div class="rc-findings">\n',
  '<div class="rc-finding" data-severity="high"><div><code>src/refunds/router.ts:21</code> · Codex and Gemini</div>',
  "<p>The limiter runs after the idempotency lookup, so a replayed request spends budget without doing anything.</p>",
  '<p class="rc-finding-note rc-ok">Fixed by Opus 5.5 in round 2</p></div>\n',
  '<div class="rc-finding" data-severity="medium"><div><code>src/charges/limit.ts:42</code> · Codex</div>',
  "<p>Refunds share the charges bucket, so a busy partner gets refunds refused.</p>",
  '<p class="rc-finding-note">Set aside: the spec says one budget per partner. <button class="rc-chip" data-rc-send="Reopen the finding at src/charges/limit.ts:42">Reopen</button></p></div>\n',
  '<div class="rc-finding" data-severity="low"><div><code>src/refunds/limit.test.ts:52</code> · Gemini</div>',
  "<p>The reset test waits on the wall clock; use fake timers so it cannot flake.</p>",
  '<p class="rc-finding-note rc-ok">Fixed by Opus 5.5 in round 2</p></div>\n',
  "</div>\n```\n",
];

/** A `rustcode-viz` reply: a bar chart and a checklist. */
export const MOCK_VIZ = [
  "Test time by crate, and where the migration stands:\n\n",
  "```rustcode-viz\n",
  '{"version":1,"type":"bar_chart","title":"Test time by crate","summary":"engine dominates the run","unit":"s","items":[{"label":"engine","value":42},{"label":"tui","value":10.5},{"label":"core","value":3}]}\n',
  "```\n\n",
  "```rustcode-viz\n",
  '{"version":1,"type":"checklist","title":"Orchestrator v2","items":[{"label":"Worktrees","status":"done"},{"label":"Checkpoints","status":"in_progress"},{"label":"Fan-out compare","status":"todo","note":"needs the engine"}]}\n',
  "```\n",
];

/** A Smart UI reply with a wide, wrapping table in tabs: long code paths and badges that must fit the reply's width. */
export const MOCK_TABLE = [
  "Here are the open gaps.\n\n",
  "```rustcode-ui v1\n",
  "<title>Open gaps</title>\n",
  '<div class="rc-col rc-gap-3">\n',
  '<div class="rc-tabs">\n',
  '<div role="tablist"><button role="tab" data-rc-tab="a">Open gaps (11)</button><button role="tab" data-rc-tab="b">Closed</button></div>\n',
  '<section data-rc-panel="a">\n',
  '<table class="rc-table" data-rc-sort>\n',
  "<thead><tr><th>#</th><th>What</th><th>Elsewhere</th><th>RustCode today</th><th>Size</th><th>Kind</th></tr></thead>\n<tbody>\n",
  '<tr><td class="rc-num">8</td><td><b>Files in question answers</b></td><td>Attach or paste a file into an agent\'s question</td><td>Question form has no attachments (<code>apps/desktop/src/ui/AttentionCards.tsx</code>)</td><td>S</td><td><span class="rc-badge rc-info">Improve</span></td></tr>\n',
  '<tr><td class="rc-num">11</td><td><b>Harness presets</b></td><td>Cursor, Kiro, Grok, Antigravity as first-class providers</td><td>Codex, Claude Code, Gemini, OpenCode, Pi; others only as a hand-added ACP command</td><td>S each</td><td><span class="rc-badge rc-info">New</span></td></tr>\n',
  "</tbody></table>\n</section>\n",
  '<section data-rc-panel="b"><div class="rc-list"><div class="rc-item">Nothing closed yet</div></div></section>\n',
  "</div></div>\n",
  "```\n",
];

/** A Smart UI reply: a themed chart and a button that asks a follow-up through `rustcode.send`. */
/** A proposal in tabs: a plan, titled points, an item written as one sentence, then prose after the view. */
export const MOCK_PROPOSAL = [
  "The issue is buildable as four PRs, and I would add two tools instead of four.\n\n",
  "```rustcode-ui v1\n<title>Plan and decisions</title>\n",
  '<div class="rc-tabs"><div role="tablist"><button role="tab" data-rc-tab="plan">How I would build it</button>',
  '<button role="tab" data-rc-tab="go">Decisions</button></div>\n',
  '<section data-rc-panel="plan">\n',
  '<p class="rc-lead">Use case: "turn off auto-compact" should be one approved tool call, not a hand edit of <code>settings.json</code>.</p>\n',
  '<ol class="rc-steps">\n',
  '<li class="rc-step rc-todo"><b>PR 1: one action table, reads everywhere.</b> The CLI verbs, the MCP schemas and <code>docs/cli.md</code> are generated from it.</li>\n',
  '<li class="rc-step rc-todo"><b>PR 2: settings writes.</b> <code>settings set/reset</code> with <code>--project</code>, and the before/after approval card.</li>\n',
  '<li class="rc-step rc-todo"><b>PR 3: secrets, CLI only.</b> <code>account add/check/remove</code>. Never as MCP tools.</li>\n',
  "</ol>\n<h3>Where I would deviate from the issue</h3>\n",
  '<ul class="rc-list">\n',
  '<li class="rc-item"><b>Two new tools, not four</b><p>There are already about 45 tools. Add <code>manage_settings</code> and <code>manage_capabilities</code>; extend <code>transfer_session</code> in place.</p></li>\n',
  '<li class="rc-item"><b>Keep <code>list_harnesses</code>.</b> Folding it into <code>manage_harnesses</code> renames a tool every prompt already uses.</li>\n',
  "</ul>\n</section>\n",
  '<section data-rc-panel="go"><p>No prerequisites needed.</p>',
  '<div class="rc-row rc-gap-2"><button data-primary data-rc-send="GO">GO: PR 1 and 2</button><button data-rc-send="Update the issue first">Update the issue first</button></div></section>\n',
  "</div>\n```\n\n",
  "Say GO and I start with PR 1.",
];

export const MOCK_VIEW = [
  "Here are this week's test runs. Pick a day to dig in.\n\n",
  "```rustcode-ui v1\n",
  "<title>Test runs this week</title>\n",
  '<div class="rc-stats">\n',
  '  <div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span></div>\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span></div>\n',
  "</div>\n",
  '<div class="rc-chart"><canvas id="c" role="img" aria-label="Runs and failures per day"></canvas></div>\n',
  '<div class="rc-row rc-gap-2">\n',
  '  <button class="primary" id="ask">Explain Thursday\'s failures</button>\n',
  '  <button class="rc-chip" id="copy">Copy numbers</button>\n',
  "</div>\n",
  '<script src="rustcode-lib:chart.js"></script>\n',
  "<script>\n",
  "new Chart(document.getElementById('c'), { type: 'bar', data: { labels: ['Mon','Tue','Wed','Thu','Fri'],\n",
  "  datasets: [{ label: 'Passed', data: [24, 27, 22, 19, 29] }, { label: 'Failed', data: [1, 0, 2, 4, 0] }] },\n",
  "  options: { scales: { x: { stacked: true }, y: { stacked: true } } } });\n",
  "document.getElementById('ask').onclick = () => window.rustcode?.send(\"Why did 4 tests fail on Thursday?\");\n",
  "document.getElementById('copy').onclick = () => window.rustcode?.copy('Mon 24/1, Tue 27/0, Wed 22/2, Thu 19/4, Fri 29/0');\n",
  "</script>\n",
  "```\n",
];

/** A reply with `h1`-`h4`, for checking the heading scale. */
export const MOCK_HEADINGS = [
  "# Release plan\n\nThe release has three parts; each heading below is one of them.\n\n",
  "## Engine changes\n\nThe engine now sends a shorter Smart UI hint on every prompt.\n\n",
  "### Prompt text\n\nThe harness hint is 2,166 bytes, up 8% from before.\n\n",
  "#### Notes\n\nA heading groups the text under it, so a long reply scans by its headings.\n\n",
  "## Desktop changes\n\n- The kit has a headline layout.\n- Chat headings are bigger.\n",
];

/** A Smart UI dashboard in the kit's headline style: a title and caption, one hero number, quiet stats and a banded line chart. */
export const MOCK_DASH = [
  "Merges are at 3.4x a normal day; the last 24 hours are the busiest of the quarter.\n\n",
  "```rustcode-ui v1\n",
  "<title>Merge traffic</title>\n",
  '<div class="rc-dash">\n',
  '<div class="rc-head"><span class="rc-title">Merge traffic</span><span>Merged PRs on acme/web, UTC. Updated 20:39 UTC.</span></div>\n',
  '<div class="rc-grid rc-gap-4">\n',
  '  <div class="rc-hero"><span class="rc-label">Merged in the last 24 hours</span><span class="rc-hero-value">503</span><span class="rc-hero-delta">3.4x</span><p class="rc-lead">A normal day is <b>150</b>, the median of the last 28 days. Busier than <b>100%</b> of the last 90 days.</p></div>\n',
  '  <div><p class="rc-label">Days by merges, last 90 days</p><div class="rc-chart" data-size="sm"><canvas id="days" role="img" aria-label="Days by number of merges"></canvas></div></div>\n',
  "</div>\n",
  "<hr>\n",
  '<div class="rc-stats rc-flat">\n',
  '  <div class="rc-stat"><span class="rc-stat-value">1,058</span><span class="rc-stat-label">Last 7 days</span><span class="rc-stat-note rc-bad">-19% vs the week before</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">347</span><span class="rc-stat-label">Today so far</span><span class="rc-stat-note">21.3 hours into the UTC day</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">448</span><span class="rc-stat-label">Busiest day</span><span class="rc-stat-note">Tue, Oct 20</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">10,335</span><span class="rc-stat-label">All time</span><span class="rc-stat-note">since Feb 9</span></div>\n',
  "</div>\n",
  '<div class="rc-head"><span class="rc-title">Trailing 24 hours</span><span>Hourly, last 14 days. Shaded band is a normal day.</span></div>\n',
  '<div class="rc-chart" data-size="xl"><canvas id="hours" role="img" aria-label="Merges in the trailing 24 hours, hourly over 14 days"></canvas></div>\n',
  "</div>\n",
  '<script src="rustcode-lib:chart.js"></script>\n',
  "<script>\n",
  "let seed = 11;\n",
  "const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;\n",
  'const day = ["Oct 30", "Oct 31", "Nov 1", "Nov 2", "Nov 3", "Nov 4", "Nov 5", "Nov 6", "Nov 7", "Nov 8", "Nov 9", "Nov 10", "Nov 11", "Nov 12"];\n',
  "const hours = [], labels = [];\n",
  "for (let i = 0; i < 14 * 24; i++) {\n",
  "  const ramp = Math.max(0, i - 14 * 24 + 36) * 11;\n",
  "  hours.push(Math.round(Math.max(20, 150 + 90 * Math.sin(i / 37) + 60 * (rnd() - 0.5) + ramp)));\n",
  '  labels.push(day[Math.floor(i / 24)] + ", " + String(i % 24).padStart(2, "0") + ":00");\n',
  "}\n",
  'new Chart(document.getElementById("hours"), { type: "line",\n',
  '  data: { labels, datasets: [{ label: "Merged, trailing 24 h", data: hours, tension: 0.2 }] },\n',
  '  options: { plugins: { legend: { display: false }, rcMarks: { band: [75, 270], line: 150, lineLabel: "normal", end: true } },\n',
  '    scales: { x: { ticks: { autoSkip: false, maxRotation: 0, callback(v, i) { return i % 48 === 0 ? day[i / 24] : ""; } } }, y: { beginAtZero: true, ticks: { maxTicksLimit: 4 } } } } });\n',
  'const bins = ["0-50", "50-100", "100-150", "150-200", "200-300", "300+"];\n',
  'new Chart(document.getElementById("days"), { type: "bar",\n',
  '  data: { labels: bins, datasets: [{ label: "Days", data: [12, 21, 29, 17, 9, 2] }] },\n',
  "  options: { plugins: { legend: { display: false } }, scales: { x: { ticks: { maxRotation: 0 } }, y: { ticks: { maxTicksLimit: 3 } } } } });\n",
  "</script>\n",
  "```\n",
];

/** A Smart UI reply built from the page's component kit: no script and next to no CSS of its own. */
export const MOCK_KIT = [
  "The release is blocked by one failing suite; everything else is green.\n\n",
  "```rustcode-ui v1\n",
  "<title>Release 1.11 readiness</title>\n",
  '<div class="rc-stats">\n',
  '  <div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Tests run</span><span class="rc-stat-note">+12 this week</span></div>\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span><span class="rc-stat-note">4 on Thursday</span></div>\n',
  '  <div class="rc-stat rc-ok"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span><span class="rc-stat-note">6 s faster</span></div>\n',
  "</div>\n",
  '<div class="rc-callout rc-warn">The auth suite fails on a stale fixture <span class="rc-tip" tabindex="0">1<span>tests/auth/fixtures/token.json expired on 2026-11-01, so every refresh test fails at setup.</span></span>, not on a code change.</div>\n',
  '<div class="rc-tabs">\n',
  '  <div role="tablist" aria-label="Sections"><button role="tab" data-rc-tab="suites">Suites</button><button role="tab" data-rc-tab="plan">Plan</button><button role="tab" data-rc-tab="log">Log</button></div>\n',
  '  <section data-rc-panel="suites">\n',
  '    <div class="rc-bars">\n',
  '      <div class="rc-bar"><span>engine</span><i style="width:100%"></i><b>42 s</b></div>\n',
  '      <div class="rc-bar"><span>desktop</span><i style="width:44%"></i><b>18.5 s</b></div>\n',
  '      <div class="rc-bar rc-bad"><span>auth</span><i style="width:25%"></i><b>10.5 s</b></div>\n',
  '      <div class="rc-bar"><span>core</span><i style="width:7%"></i><b>3 s</b></div>\n',
  "    </div>\n",
  '    <table class="rc-table"><thead><tr><th>Suite</th><th>Status</th><th class="rc-num">Passed</th><th class="rc-num">Failed</th><th class="rc-num">Time</th></tr></thead><tbody>\n',
  '      <tr><td>engine</td><td><span class="rc-badge rc-ok">Passing</span></td><td class="rc-num">64</td><td class="rc-num">0</td><td class="rc-num">42 s</td></tr>\n',
  '      <tr><td>desktop</td><td><span class="rc-badge rc-ok">Passing</span></td><td class="rc-num">38</td><td class="rc-num">0</td><td class="rc-num">18.5 s</td></tr>\n',
  '      <tr><td>auth</td><td><span class="rc-badge rc-bad">Failing</span></td><td class="rc-num">12</td><td class="rc-num">7</td><td class="rc-num">10.5 s</td></tr>\n',
  '      <tr><td>core</td><td><span class="rc-badge rc-info">Skipped 1</span></td><td class="rc-num">7</td><td class="rc-num">0</td><td class="rc-num">3 s</td></tr>\n',
  "    </tbody></table>\n",
  "  </section>\n",
  '  <section data-rc-panel="plan">\n',
  '    <div class="rc-row rc-gap-2"><span class="rc-label">2 of 4 steps</span><div class="rc-progress" style="flex:1"><span style="width:50%"></span></div></div>\n',
  '    <ol class="rc-steps">\n',
  '      <li class="rc-step rc-done">Bump the version <div class="rc-label">Done by the release script</div></li>\n',
  '      <li class="rc-step rc-done">Build the macOS app</li>\n',
  '      <li class="rc-step rc-failed">Run the auth suite <div class="rc-label">7 failures, one cause</div></li>\n',
  '      <li class="rc-step rc-active">Refresh the fixture</li>\n',
  '      <li class="rc-step rc-todo">Publish the beta</li>\n',
  "    </ol>\n",
  "  </section>\n",
  '  <section data-rc-panel="log">\n',
  '    <div class="rc-term"><div>$ cargo test -p auth</div><div class="rc-ok">test refresh::keeps_session ... ok</div><div class="rc-bad">test refresh::rotates_token ... FAILED</div><div>test result: 12 passed; 7 failed</div></div>\n',
  "  </section>\n",
  "</div>\n",
  "<div>\n",
  '  <details class="rc-details"><summary>Owners <span class="rc-label">3</span></summary><ul class="rc-list">\n',
  '    <li class="rc-item"><span>Engine</span><span class="rc-badge rc-ok">Ready</span></li>\n',
  '    <li class="rc-item"><span>Desktop</span><span class="rc-badge rc-ok">Ready</span></li>\n',
  '    <li class="rc-item"><span>Auth</span><span class="rc-badge rc-warn">Waiting</span></li>\n',
  '    <li class="rc-item"><span class="rc-badge">Worth reviewing</span><strong>docs/superpowers/</strong><div>Ignored by the repo. It contains 26 design specs and plans, including provider login and remote turn timing.</div></li>\n',
  "  </ul></details>\n",
  '  <details class="rc-details rc-tree"><summary>Changed files <span class="rc-label">4</span></summary><div>\n',
  '    <details class="rc-details" open><summary>tests/auth</summary><div>\n',
  '      <div class="rc-file"><span>fixtures/token.json</span><span>+2 −2</span></div>\n',
  '      <div class="rc-file"><span>refresh.rs</span><span>+14 −3</span></div>\n',
  "    </div></details>\n",
  '    <div class="rc-file"><span>Cargo.lock</span><span>+1 −1</span></div>\n',
  '    <div class="rc-file"><span>CHANGELOG.md</span><span>+6</span></div>\n',
  "  </div></details>\n",
  "</div>\n",
  '<div class="rc-row rc-gap-2">\n',
  '  <button class="rc-chip" data-rc-send="Refresh the auth fixture">Refresh the fixture</button>\n',
  '  <button class="rc-chip" data-rc-send="Show the failing tests">Show the failures</button>\n',
  '  <button class="rc-chip" data-rc-copy="128 run, 7 failed, 41 s median">Copy numbers</button>\n',
  "</div>\n",
  '<details class="rc-sources"><summary>Source</summary><ol>\n',
  '  <li><a href="https://github.com/LHagfoss/rustcode/pull/3877">Pull request 3877</a></li>\n',
  "</ol></details>\n",
  "```\n",
];

/**
 * A Smart UI reply built from the kit's charts and hover pieces: a themed line
 * chart, a donut with its total in the middle, bars with hover text, an inline
 * sparkline and a table that sorts on a heading click. No CSS of its own.
 */
export const MOCK_CHARTS = [
  "Failures fell to zero after Thursday's fixture fix.\n\n",
  "```rustcode-ui v1\n",
  "<title>Failures this week</title>\n",
  '<p class="rc-lead">Failures fell to zero after Thursday\'s fixture fix.</p>\n',
  '<div class="rc-stats">\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span><span class="rc-stat-note">4 on Thursday</span></div>\n',
  '  <div class="rc-stat rc-ok"><span class="rc-stat-value">0</span><span class="rc-stat-label">Failed since</span><span class="rc-stat-note">3 days clean</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span><span class="rc-stat-note">6 s faster</span></div>\n',
  "</div>\n",
  '<div class="rc-chart"><canvas role="img" aria-label="Runs and failures per day"></canvas></div>\n',
  '<div class="rc-row rc-gap-3">\n',
  '  <div class="rc-chart" data-size="sm" style="width:140px"><canvas role="img" aria-label="Runs by suite"></canvas>',
  '<div class="rc-donut-total"><b>128</b><span>runs</span></div></div>\n',
  '  <div class="rc-bars" style="flex:1">\n',
  '    <div class="rc-bar" data-rc-tip="64 passed, 0 failed, 42 s"><span>engine</span><i style="width:100%"></i><b>42 s</b></div>\n',
  '    <div class="rc-bar" data-rc-tip="38 passed, 0 failed, 18.5 s"><span>desktop</span><i style="width:44%"></i><b>18.5 s</b></div>\n',
  '    <div class="rc-bar rc-bad" data-rc-tip="12 passed, 7 failed, 10.5 s"><span>auth</span><i style="width:25%"></i><b>10.5 s</b></div>\n',
  "  </div>\n",
  "</div>\n",
  '<div class="rc-row rc-gap-2"><span class="rc-label">Duration, last 8 runs</span>',
  '<span class="rc-spark" data-values="86,81,79,74,72,63,55,48" style="width:120px"></span>',
  '<span class="rc-num">48 s</span></div>\n',
  '<table class="rc-table" data-rc-sort><thead><tr><th>Suite</th><th class="rc-num">Time</th><th class="rc-num">Failed</th></tr></thead><tbody>\n',
  '  <tr><td>engine</td><td class="rc-num">42 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>desktop</td><td class="rc-num">18.5 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>auth</td><td class="rc-num">10.5 s</td><td class="rc-num">7</td></tr>\n',
  '  <tr><td>core</td><td class="rc-num">3 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>e2e</td><td class="rc-num">64 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>cli</td><td class="rc-num">2 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>mcp</td><td class="rc-num">1 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>sandbox</td><td class="rc-num">9 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>store</td><td class="rc-num">4 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>tui</td><td class="rc-num">7 s</td><td class="rc-num">0</td></tr>\n',
  "</tbody></table>\n",
  '<script src="rustcode-lib:chart.js"></script>\n',
  "<script>\n",
  "const canvases = document.querySelectorAll('.rc-chart canvas');\n",
  "new Chart(canvases[0], { type: 'line', data: { labels: ['Mon','Tue','Wed','Thu','Fri'],\n",
  "  datasets: [{ label: 'Runs', data: [25,27,24,23,29] }, { label: 'Failed', data: [1,0,2,4,0] }] } });\n",
  "new Chart(canvases[1], { type: 'doughnut', data: { labels: ['engine','desktop','auth','core'],\n",
  "  datasets: [{ label: 'Runs', data: [64,38,19,7] }] }, options: { plugins: { legend: { display: false } } } });\n",
  "</script>\n",
  '<details class="rc-sources"><summary>Sources</summary><ol>\n',
  '  <li><a href="https://github.com/LHagfoss/rustcode/actions">CI runs, this week</a></li>\n',
  '  <li><a href="https://www.example.com/flaky-tests">Flaky test triage notes</a></li>\n',
  "</ol></details>\n",
  "```\n",
];

export const svgIcon = (fill: string, letter: string) =>
  `data:image/svg+xml;base64,${btoa(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="4" fill="${fill}"/><text x="8" y="12" font-size="10" font-family="sans-serif" text-anchor="middle" fill="#fff">${letter}</text></svg>`,
  )}`;

/** Canned link metadata, as the shell's `link_meta` answers. */
export function mockLinkMeta(url: string): JsonObject | null {
  const host = (() => {
    try {
      return new URL(url).host;
    } catch {
      return "";
    }
  })();
  if (host.endsWith("github.com")) {
    const path = new URL(url).pathname.split("/").filter(Boolean);
    return {
      title:
        path.length >= 4
          ? `${path[2] === "pull" ? "Pull request" : "Issue"} #${path[3]} · ${path[0]}/${path[1]}`
          : `${path.join("/") || "GitHub"}`,
      siteName: "GitHub",
      color: "#1e2327",
      icon: svgIcon("#1e2327", "G"),
    };
  }
  if (host.endsWith("docs.rs"))
    return { title: "tokio - Rust", color: "#e57300", icon: svgIcon("#e57300", "D") };
  if (host.endsWith("example.com")) return null;
  return { title: host, icon: svgIcon("#435c6e", host.charAt(0).toUpperCase() || "?") };
}

/** The reconnect view as agents write it in kit HTML: bars with hover tips, a plan, a callout, cards, sources. */
export const MOCK_RECONNECT_HTML = [
  "The wait is mostly the silence limit and the backoff cap.\n\n",
  "```rustcode-ui v1\n<title>Reconnect timing</title>\n",
  "<h3>Why reconnecting takes about a minute</h3>\n",
  '<div class="rc-bars">\n',
  '<div class="rc-bar" data-rc-tip="SILENCE_LIMIT, rustcode/client/src/link.rs"><span>Detecting a silent drop</span><i style="width:100%"></i><b>up to 75 s</b></div>\n',
  '<div class="rc-bar" data-rc-tip="BACKOFF_CAP, rustcode/client/src/link.rs"><span>Backoff after repeated short drops</span><i style="width:80%"></i><b>up to 60 s</b></div>\n',
  '<div class="rc-bar" data-rc-tip="PING_TIMEOUT"><span>Ping answer on a VPS route</span><i style="width:20%"></i><b>15 s</b></div>\n',
  "</div>\n",
  '<p class="rc-lead">The wait ends early only when the network changes, the computer wakes or the app comes to the front.</p>\n',
  "<h3>What I recommend, in order</h3>\n",
  '<ol class="rc-steps">\n',
  '<li class="rc-step rc-active"><b>Fast reconnect.</b> Ping every 10 s and treat 20 s of silence as a drop; cap the backoff at 5 s while the app is in front.</li>\n',
  '<li class="rc-step rc-todo"><b>Settings follow the MacBook.</b> Push config, skills and MCP servers over the existing link.</li>\n',
  '<li class="rc-step rc-todo"><b>Complete thread copies.</b> VPS threads appear in the sidebar with full history.</li>\n',
  "</ol>\n",
  '<div class="rc-callout rc-info">MCP is already in the CLI: <code>rc mcp add|list|remove</code>.</div>\n',
  '<div class="rc-grid">\n',
  '<div class="rc-card"><b>T3 parity after 1–3</b><p>One-way linked machines, quick reconnects, per-machine sign-ins.</p></div>\n',
  '<div class="rc-card"><b>Deliberately not copied</b><p>A cloud relay; the hub plus your own tunnel covers it.</p></div>\n',
  "</div>\n",
  '<details class="rc-details rc-sources"><summary>Sources</summary><ol><li><a href="https://github.com/LHagfoss/rustcode">rustcode</a></li></ol></details>\n',
  "```\n",
];

/** The same view in the shorthand (docs/smart-ui.md › Shorthand): the app writes the kit markup. */
export const MOCK_RECONNECT_SHORTHAND = [
  "The wait is mostly the silence limit and the backoff cap.\n\n",
  "```rustcode-ui v1\n:: title Reconnect timing\n",
  ":: bars Why reconnecting takes about a minute\n",
  "- Detecting a silent drop | 75 | up to 75 s | SILENCE_LIMIT, rustcode/client/src/link.rs\n",
  "- Backoff after repeated short drops | 60 | up to 60 s | BACKOFF_CAP, rustcode/client/src/link.rs\n",
  "- Ping answer on a VPS route | 15 | 15 s | PING_TIMEOUT\n",
  ":: p The wait ends early only when the network changes, the computer wakes or the app comes to the front.\n",
  ":: steps What I recommend, in order\n",
  "- [>] **Fast reconnect.** Ping every 10 s and treat 20 s of silence as a drop; cap the backoff at 5 s while the app is in front.\n",
  "- [ ] **Settings follow the MacBook.** Push config, skills and MCP servers over the existing link.\n",
  "- [ ] **Complete thread copies.** VPS threads appear in the sidebar with full history.\n",
  ":: callout info\n",
  "MCP is already in the CLI: `rc mcp add|list|remove`.\n",
  ":: cards\n",
  "- T3 parity after 1–3 {ok} | One-way linked machines, quick reconnects, per-machine sign-ins.\n",
  "- Deliberately not copied {warn} | A cloud relay; the hub plus your own tunnel covers it.\n",
  ":: sources\n",
  "- rustcode | https://github.com/LHagfoss/rustcode\n",
  "```\n",
];

/** MOCK_CHARTS written with the shorthand catalog: the same view, no script and no class names. */
export const MOCK_CHARTS_SHORTHAND = [
  "Failures fell to zero after Thursday's fixture fix.\n\n",
  "```rustcode-ui v1\n:: title Failures this week\n",
  ":: stats\n",
  "- 7 | Failed | 4 on Thursday {bad} | 1,0,2,4,0\n",
  "- 0 | Failed since | 3 days clean {ok}\n",
  "- 41 s | Median run | 6 s faster | 86,81,79,74,72,63,55,48\n",
  "\n",
  ":: chart line Runs and failures per day\n",
  "day | Runs | Failed\n",
  "Mon | 25 | 1\nTue | 27 | 0\nWed | 24 | 2\nThu | 23 | 4\nFri | 29 | 0\n",
  "\n",
  ":: chart donut Runs by suite\n",
  "suite | runs\n",
  "engine | 64\ndesktop | 38\nauth | 19\ncore | 7\n",
  "\n",
  ":: bars Time by suite\n",
  "- engine | 42 | 42 s | 64 passed, 0 failed\n",
  "- desktop | 18.5 | 18.5 s | 38 passed, 0 failed\n",
  "- auth | 10.5 | 10.5 s | 12 passed, 7 failed {bad}\n",
  ":: sources\n",
  "- CI runs, this week | https://github.com/LHagfoss/rustcode/actions\n",
  "- Flaky test triage notes | https://www.example.com/flaky-tests\n",
  "```\n",
];

/** MOCK_DASH written with the shorthand catalog: head, hero, flat stats and a marked chart. */
export const MOCK_DASH_SHORTHAND = [
  "Merges are at 3.4x a normal day; the last 24 hours are the busiest of the quarter.\n\n",
  "```rustcode-ui v1\n:: title Merge traffic\n",
  ":: head Merge traffic | Merged PRs on acme/web, UTC. Updated 20:39 UTC.\n",
  ":: hero Merged in the last 24 hours\n",
  "- 503 | 3.4x | A normal day is **150**, the median of the last 28 days. Busier than **100%** of the last 90 days.\n",
  ":: stats\n",
  "- 1,058 | Last 7 days | -19% vs the week before {bad}\n",
  "- 347 | Today so far | 21.3 hours into the UTC day\n",
  "- 448 | Busiest day | Tue, Oct 20\n",
  "\n",
  ":: chart line Merged per day, last two weeks | size=lg | band=75-270 | line=150 normal | end\n",
  "day | Merged\n",
  "Oct 30 | 141\nOct 31 | 162\nNov 1 | 98\nNov 2 | 87\nNov 3 | 171\nNov 4 | 188\nNov 5 | 176\n",
  "Nov 6 | 155\nNov 7 | 142\nNov 8 | 104\nNov 9 | 96\nNov 10 | 219\nNov 11 | 348\nNov 12 | 503\n",
  "```\n",
];

/** New catalog blocks in one reply: progress, key-value, timeline, compare, heatmap, files, tags, tabs and a form. */
export const MOCK_CATALOG = [
  "Rollout is at 72% with one region held; the canary is clean.\n\n",
  "```rustcode-ui v1\n:: title Rollout of 1.48\n",
  ":: progress Rollout by region\n",
  "- Europe | 100% | 14 of 14 hosts {ok}\n",
  "- North America | 72% | 18 of 25 hosts\n",
  "- Asia Pacific | 0/9 | held: p95 over budget {warn}\n",
  ":: tab Status\n",
  ":: kv\n",
  "- Release | `v1.48.0`\n",
  "- Canary | clean {ok}\n",
  "- Error budget | 81% left\n",
  "- Owner | release-bot\n",
  ":: timeline What happened\n",
  "- 09:02 | Canary started | 2% of traffic {info}\n",
  "- 09:40 | Europe complete | no regressions {ok}\n",
  "- 10:15 | Asia Pacific held | p95 412 ms against a 300 ms budget {warn}\n",
  ":: tab Harnesses\n",
  ":: compare\n",
  "Feature | Codex | Claude Code | Gemini\n",
  "Streaming views | yes | yes | yes\n",
  "Smart UI check | yes | yes | partial via MCP\n",
  "Preview screenshot | no | no | no\n",
  ":: tab Latency\n",
  ":: heatmap p95 by hour (ms)\n",
  "region | 06h | 09h | 12h | 15h\n",
  "Europe | 180 | 210 | 240 | 205\n",
  "North America | 160 | 230 | 290 | 260\n",
  "Asia Pacific | 250 | 412 | 380 | 300\n",
  ":: end\n",
  ":: files Changed in this release\n",
  "- rustcode/engine/src/smart_ui.rs | +84 -6\n",
  "- apps/desktop/src/smartui/shorthand.ts | +512 -18 | the catalog\n",
  "- docs/smart-ui.md | +96 -12\n",
  ":: tags\n",
  "- desktop {info}\n",
  "- engine\n",
  "- no migration {ok}\n",
  ":: form Resume the Asia Pacific rollout | Resume\n",
  "- choice pace | Pace | slow, *normal, fast\n",
  "- range share | Traffic share (%) | 0-100 = 25\n",
  "- check page | Page on-call if p95 rises\n",
  "```\n",
];

/** A question form with long options, as an agent asks before a large batch. */
export const MOCK_FORM = [
  "Before I change code for a batch this size, I need these answers.\n\n",
  "```rustcode-ui v1\n",
  ":: form Go with these answers | Start\n",
  "- choice merge | After CI is green | merge each PR myself, *open PRs and you merge, merge and cut a beta after each step\n",
  "- choice who | Who does the work | *delegate waves (faster, more usage), only this thread (AGENTS.md default)\n",
  "- choice agent | Built-in agent (2 of 3,600 turns) | *move to its own crate, delete it\n",
  "- choice phone | Phone on ACP | *in scope, I have rustcode-mobile checked out, skip for now\n",
  "- select region | Region | Europe, *North America, Asia Pacific\n",
  "- check branches | Delete merged remote branches (~990) and the 4 stale worktrees\n",
  "- check slots | Delete build slots under ~/.config/rustcode/build (74 GB)\n",
  "```\n",
];

/** The catalog replies by prompt: "catalog dashboard", "catalog trend", "catalog form", or any other "catalog". */
export const catalogReply = (prompt: string): string[] | undefined =>
  /\bcatalog dashboard\b/i.test(prompt)
    ? MOCK_DASH_SHORTHAND
    : /\bcatalog form\b/i.test(prompt)
      ? MOCK_FORM
      : /\bcatalog trend\b/i.test(prompt)
        ? MOCK_CHARTS_SHORTHAND
        : /\bcatalog\b/i.test(prompt)
          ? MOCK_CATALOG
          : undefined;
