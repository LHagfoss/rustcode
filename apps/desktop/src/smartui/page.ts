// design-lint: allow-file (the sandboxed page's own stylesheet and chart palette, not app chrome)
// The page a Smart UI block runs in: a CSP, the base stylesheet and the
// runtime (`window.rustcode`) before the block, and Chart.js inlined where
// the block names `rustcode-lib:chart.js`. The names here are a contract
// with the engine's `smart-ui` skill (docs/smart-ui.md, "Runtime").

import { GITHUB_REF_URL } from "../lib/links";
import { KIT_SCRIPT } from "./kit";
import { stripEmptyTags } from "./parse";
import { expandShorthand } from "./shorthand";

export const PAGE_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; " +
  "connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

export type ThemeMode = "light" | "dark";

/**
 * The series ramp after the first colour, which is the app's accent (below): fixed
 * hues tuned to the mode, so a chart stays legible whatever the accent is.
 */
const SERIES: Record<ThemeMode, string[]> = {
  light: ["#007aff", "#ff9500", "#34c759", "#ff3b30", "#af52de", "#30b0c7", "#ffcc00", "#ff2d55"],
  dark: ["#0a84ff", "#ff9f0a", "#30d158", "#ff453a", "#bf5af2", "#40c8e0", "#ffd60a", "#ff375f"],
};

/** A colour a canvas can parse: the accent leads the ramp, when the theme sets a plain hex. */
const HEX = /^#[0-9a-f]{3,8}$/i;

export interface PageTheme {
  mode: ThemeMode;
  vars: Record<string, string>;
}

/** The faces a page falls back to when it is drawn without the app's own (`fonts` below). */
const MAC_FONTS = {
  sans: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", system-ui, sans-serif',
  mono: '"SF Mono", ui-monospace, Menlo, monospace',
};

/**
 * The page's variables from the app's tokens (`index.css`), read through `token`. `fonts` are the
 * app's own `--font-sans` / `--font-mono`, which follow the platform (docs/design.md › Platforms).
 */
export function pageTheme(
  mode: ThemeMode,
  token: (name: string) => string,
  fonts: { sans?: string; mono?: string } = {},
): PageTheme {
  const vars: Record<string, string> = {
    "--rc-text": token("--rc-text"),
    "--rc-knob": token("--rc-knob") || "#ffffff",
    "--rc-knob-fg": token("--rc-knob-fg") || "#171717",
    "--rc-bg": token("--rc-bg"),
    "--rc-muted": token("--rc-text-muted"),
    "--rc-faint": token("--rc-text-faint"),
    "--rc-accent": token("--rc-accent"),
    "--rc-accent-text": token("--rc-accent-text"),
    "--rc-surface": token("--rc-surface"),
    "--rc-separator": token("--rc-border"),
    "--rc-danger": token("--rc-danger"),
    "--rc-success": token("--rc-success"),
    "--rc-warning": token("--rc-warning"),
    "--rc-font": fonts.sans || MAC_FONTS.sans,
    "--rc-mono": fonts.mono || MAC_FONTS.mono,
    // The app's own radius and type scale, so a view matches the chrome around it
    // (docs/themes.md, docs/design.md). A view with nothing else still reads.
    "--rc-radius": token("--rc-radius-md") || "10px",
    "--rc-radius-sm": token("--rc-radius-sm") || "8px",
    "--rc-radius-xs": token("--rc-radius-xs") || "6px",
    "--rc-radius-pill": token("--radius-full") || "999px",
    "--rc-text-size": token("--text-callout") || "13px",
    "--rc-text-size-sm": token("--text-sub") || "12px",
    "--rc-text-size-xs": token("--text-caption") || "11px",
    "--rc-text-size-title": token("--text-title3") || "16px",
    "--rc-text-size-value": token("--text-title") || "23px",
  };
  const accent = token("--rc-accent");
  const ramp = SERIES[mode];
  const series = [HEX.test(accent) ? accent : ramp[0]!, ...ramp.slice(1)];
  series.forEach((c, i) => {
    vars[`--rc-series-${i + 1}`] = c;
  });
  return { mode, vars };
}

/** Only plain custom-property values go into the page's stylesheet. */
function safeValue(value: string): string {
  return value.replace(/[;{}<>\\]/g, "");
}

export function varsCss(theme: PageTheme): string {
  const body = Object.entries(theme.vars)
    .map(([k, v]) => `${k}: ${safeValue(v)};`)
    .join(" ");
  return `:root { color-scheme: ${theme.mode}; ${body} }`;
}

export const BASE_CSS = `
/* Nothing in the page scrolls by itself: the document never does, and the body takes its
   content's full height so the app grows the frame to it. A scrollbar would narrow the
   layout and re-wrap the text under the pointer. Scrolling is opt-in (the .rc-scroll block,
   or the view's own overflow), which is what keeps a long block readable without a nested card. */
html { overflow: hidden; }
html, body { margin: 0; background: transparent; }
body { position: relative; color: var(--rc-text); font: var(--rc-text-size)/1.5 var(--rc-font);
  padding: 1px 0 2px; overflow-wrap: anywhere; -webkit-font-smoothing: antialiased; }
* { box-sizing: border-box; }
::selection { background: var(--rc-knob); color: var(--rc-knob-fg); text-shadow: none; }
:focus { outline: none; }
:root[data-keyboard-nav] :focus-visible { outline: 2px solid color-mix(in srgb, var(--rc-accent) 60%, transparent); outline-offset: 1px; }
/* One vertical rhythm for whatever the view writes, kit block or bare tag: the gaps
   between blocks are the page's, not each class's own. The gap goes between drawn blocks only: a leading <title> or <style> is an element too,
   and a plain \`* + *\` gave the first visible block a 12px margin that collapsed out of the
   root, so the measured height came up 12px short and the view's foot was clipped.
   \`flow-root\` keeps any child margin inside the root, where the measurement sees it. */
#rc-root, #rc-stream { display: flow-root; }
/* The browser's own block margins go here, at zero specificity, and nowhere else: a
   \`margin: 0\` on a tag or kit class (h3, p, .rc-steps, .rc-list) outranked the rhythm below, so a
   tab panel ran its lead, its plan and its next heading together with no gap. */
:where(h1, h2, h3, h4, p, ol, ul, pre, hr, blockquote, figure) { margin: 0; }
#rc-root > :not(title, style, script, link, meta, template) ~ :not(title, style, script, link, meta, template),
#rc-stream > :not(title, style, script, link, meta, template) ~ :not(title, style, script, link, meta, template) { margin-top: 16px; }
/* The same rhythm one level down, in the blocks that hold other blocks: a tab panel, a section,
   a gap-less column, a card, an open details. Zero specificity, so a part's own margin (p + p,
   a tree's indent) still wins; the layouts that space their own children are left out. */
:where(section, [data-rc-panel], details, .rc-card, .rc-col:not([class*="rc-gap-"])):where(:not(.rc-row, .rc-grid, .rc-stats, .rc-hero, .rc-head, .rc-bars, .rc-list, .rc-steps, .rc-dash))
  > :where(:not(title, style, script, summary)) ~ :where(:not(title, style, script, summary)) { margin-top: 16px; }
/* A heading opens a new part, so it stands a little further from the block before it. */
:where(section, [data-rc-panel], details, .rc-card, .rc-col:not([class*="rc-gap-"])):where(:not(.rc-row, .rc-grid, .rc-stats, .rc-hero, .rc-head, .rc-bars, .rc-list, .rc-steps, .rc-dash))
  > :where(:not(title, style, script, summary)) ~ :where(h1, h2, h3, h4) { margin-top: 24px; }
/* A label or heading names the block under it, so it sits close to that block. */
:where(section, [data-rc-panel], details, .rc-card, .rc-col:not([class*="rc-gap-"])):where(:not(.rc-row, .rc-grid, .rc-stats, .rc-hero, .rc-head, .rc-bars, .rc-list, .rc-steps, .rc-dash))
  > :where(.rc-label, h1, h2, h3, h4) + :where(:not(title, style, script)) { margin-top: 6px; }
:is(#rc-root, #rc-stream) > :not(title, style, script, link, meta, template) ~ :is(h1, h2, h3, h4) { margin-top: 24px; }
:is(#rc-root, #rc-stream, #rc-root .rc-dash, #rc-stream .rc-dash) > :is(.rc-label, h1, h2, h3, h4) + :not(title, style, script) { margin-top: 6px; }
h1, h2, h3, h4 { font-size: var(--rc-text-size); font-weight: 600; line-height: 1.3; }
h1 { font-size: var(--rc-text-size-title); }
p + p { margin-top: 8px; }
small, .rc-muted { color: var(--rc-muted); }
a { color: var(--rc-accent); }
a.rc-github-chip { display: inline-flex; align-items: center; gap: 4px; vertical-align: middle;
  max-width: 100%; padding: 0 5px; border-radius: 999px; color: var(--rc-text);
  background: var(--rc-surface); box-shadow: inset 0 0 0 1px var(--rc-separator);
  font-size: var(--rc-text-size-sm); text-decoration: none; }
a.rc-github-chip:hover { text-decoration: underline; }
.rc-github-chip > svg { width: 12px; height: 12px; flex: none; }
.rc-github-chip > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
table { border-collapse: collapse; width: 100%; font-size: var(--rc-text-size-sm); }
th { text-align: left; font-weight: 600; color: var(--rc-muted); padding: 5px 8px;
  border-bottom: 1px solid var(--rc-separator); }
td { padding: 5px 8px; border-bottom: 1px solid var(--rc-separator); vertical-align: top; }
tr:last-child td { border-bottom: 0; }
input, select, textarea { font: inherit; color: inherit; background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-sm); padding: 4px 8px; outline: none; }
:root[data-keyboard-nav] :is(input, select, textarea):focus-visible { border-color: var(--rc-accent); outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--rc-accent) 22%, transparent); }
input[type=checkbox], input[type=radio], input[type=range], select { cursor: pointer; }
input[type=checkbox], input[type=radio], input[type=range] { accent-color: var(--rc-accent); }
button { font: inherit; font-weight: 500; color: var(--rc-text); background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-pill); padding: 3px 12px; cursor: pointer; }
button:hover { background: color-mix(in srgb, var(--rc-text) 9%, transparent); }
button { transition: transform 90ms ease-out, background-color 90ms ease-out; }
button:active:not(:disabled) { transform: scale(0.97); opacity: 0.85; }
button.primary, button[data-primary] { background: var(--rc-accent); color: var(--rc-accent-text); border-color: transparent; }
button:disabled { opacity: 0.45; cursor: default; }
code, kbd, pre { font-family: var(--rc-mono); font-size: var(--rc-text-size-sm); }
/* Inline code reads as a token, not a run of mono text: a faint pill that keeps its corners when
   wrapped, and breaks at path/identifier boundaries (after / . _ - : via <wbr>) instead of mid-word. */
:not(pre) > code { padding: 1px 4px; border-radius: var(--rc-radius-xs, 4px); overflow-wrap: break-word;
  background: color-mix(in srgb, var(--rc-text) 7%, transparent);
  -webkit-box-decoration-break: clone; box-decoration-break: clone; }
pre { background: var(--rc-surface); padding: 8px 10px; border-radius: var(--rc-radius); overflow: auto; }
hr { border: 0; border-top: 1px solid var(--rc-separator); }
canvas { max-width: 100%; }
.rc-row { display: flex; flex-wrap: wrap; align-items: center; }
.rc-col { display: flex; flex-direction: column; }
.rc-grid, .rc-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); }
/* Cards hold a sentence or path each: two balanced columns in the reply width (three when there
   are exactly three cards, and up to four in the enlarged dialog), never narrow towers of text. */
.rc-cards { grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 10px; }
.rc-cards:has(> :nth-child(3):last-child) { grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); }
.rc-gap-1 { gap: 4px; } .rc-gap-2 { gap: 8px; } .rc-gap-3 { gap: 12px; } .rc-gap-4 { gap: 16px; }
.rc-mono { font-family: var(--rc-mono); }
.rc-num { font-variant-numeric: tabular-nums; }
td.rc-num, th.rc-num, .rc-num td, .rc-num th { text-align: right; }
/* A card is a quiet neutral tile (docs/design.md): a bold title with its badge at the right, then
   full-width lines a few pixels apart. Status tint stays on the title's leading dot — never a
   coloured card background or coloured outline. */
.rc-card { display: flex; flex-wrap: wrap; align-items: center; align-content: flex-start; gap: 8px; min-width: 0;
  padding: 12px 14px; background: var(--rc-surface); border-radius: var(--rc-radius); }
.rc-card > * { flex: 0 0 100%; min-width: 0; margin: 0; }
.rc-card > :is(b, strong, .rc-badge, .rc-chip) { flex: 0 1 auto; }
.rc-card > :is(b, strong):first-child { font-weight: 600; flex: 1 1 0; }
/* A tinted card, stat or titled item leads with a dot in its tint, so status reads on the title
   while the surface stays calm monochrome. */
:is(.rc-card, .rc-item):is(.rc-ok, .rc-warn, .rc-bad, .rc-info) > :is(b, strong):first-child::before,
.rc-stat:is(.rc-ok, .rc-warn, .rc-bad, .rc-info) > .rc-stat-label::before {
  content: ""; display: inline-block; width: 6px; height: 6px; margin: 0 7px 1px 0; border-radius: 50%;
  background: var(--rc-tint); vertical-align: middle; }
.rc-card > .rc-badge { margin-left: auto; }
.rc-card > .rc-badge:not(.rc-ok, .rc-warn, .rc-bad, .rc-info) { --rc-tint: initial; }
.rc-card > :is(p, .rc-lead) { color: var(--rc-muted); font-size: var(--rc-text-size-sm); }
.rc-card > .rc-stat-value { font-size: var(--rc-text-size-value); font-weight: 600; line-height: 1.2; font-variant-numeric: tabular-nums; }
.rc-scroll { max-height: 320px; overflow: auto; }

.rc-title { font-size: var(--rc-text-size-title); font-weight: 600; line-height: 1.3; }
.rc-lead { color: var(--rc-muted); }
.rc-lead :is(b, strong) { color: var(--rc-text); font-weight: 600; }
/* A title on the left and a muted caption (source, range, time) on the right. */
.rc-head { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 2px 16px; }
.rc-head > :not(:first-child) { margin-left: auto; text-align: right; font-size: var(--rc-text-size-sm); color: var(--rc-muted); }
/* One huge number, an accent multiplier or delta beside it, then a label above and a sentence below. */
.rc-hero { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 12px; }
.rc-hero > :is(.rc-label, p, .rc-lead) { flex: 0 0 100%; margin: 0; }
.rc-hero > .rc-label { font-size: var(--rc-text-size-sm); }
.rc-hero-value { font-size: 64px; line-height: 1.05; font-weight: 600; letter-spacing: -0.035em; font-variant-numeric: tabular-nums; }
.rc-hero-delta { font-size: var(--rc-text-size-value); line-height: 1.2; font-weight: 600; color: var(--rc-tint, var(--rc-accent)); }
.rc-hero > .rc-lead, .rc-hero > p { margin-top: 8px; color: var(--rc-muted); }
/* A dashboard: wider rhythm between its parts. */
#rc-root .rc-dash, #rc-stream .rc-dash { display: flow-root; }
#rc-root .rc-dash > :not(title, style, script) ~ :not(title, style, script),
#rc-stream .rc-dash > :not(title, style, script) ~ :not(title, style, script) { margin-top: 28px; }
.rc-label { font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }
.rc-ok { --rc-tint: var(--rc-success); } .rc-warn { --rc-tint: var(--rc-warning); }
.rc-bad { --rc-tint: var(--rc-danger); } .rc-info { --rc-tint: var(--rc-accent); }

.rc-stats { gap: 10px; }
.rc-stat { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: 12px 14px;
  background: var(--rc-surface); border-radius: var(--rc-radius); }
.rc-stat-label { order: -1; font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }
.rc-flat { gap: 16px 24px; }
.rc-flat > .rc-stat { padding: 0; background: transparent; border-radius: 0; gap: 2px; }
.rc-flat .rc-stat-value { font-size: 28px; line-height: 1.15; letter-spacing: -0.02em; }
.rc-stat-value { font-size: var(--rc-text-size-value); line-height: 1.25; font-weight: 600; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.rc-stats > :not(.rc-stat, title, style, script) { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: 12px 14px;
  background: var(--rc-surface); border-radius: var(--rc-radius); }
.rc-flat > :not(.rc-stat, title, style, script) { padding: 0; background: transparent; border-radius: 0; }
.rc-stats > :not(.rc-stat) > :first-child:not(.rc-label, .rc-stat-label) { font-size: var(--rc-text-size-value); line-height: 1.25;
  font-weight: 600; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.rc-flat > :not(.rc-stat) > :first-child:not(.rc-label, .rc-stat-label) { font-size: 28px; line-height: 1.15; letter-spacing: -0.02em; }
.rc-stats > :not(.rc-stat) > :is(.rc-label, .rc-stat-label) { order: -1; }
.rc-stat-note { font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }

.rc-badge { display: inline-block; padding: 0 7px; font-size: var(--rc-text-size-xs); line-height: 18px; font-weight: 500; white-space: nowrap;
  border-radius: var(--rc-radius-pill); color: var(--rc-tint, var(--rc-muted));
  background: color-mix(in srgb, var(--rc-tint, var(--rc-text)) 13%, transparent); }
.rc-callout { padding: 8px 12px; border-radius: var(--rc-radius);
  background: color-mix(in srgb, var(--rc-tint, var(--rc-text)) 10%, transparent); }
.rc-chip { padding: 2px 10px; font-size: var(--rc-text-size-sm); line-height: 18px; font-weight: 400; color: var(--rc-muted); cursor: pointer;
  background: transparent; border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-pill); }
.rc-chip:hover { color: var(--rc-text); }
[data-rc-send], [data-rc-copy] { cursor: pointer; }
/* A card, stat, item, bar or row that sends a follow-up looks pressable: a wash on hover, a
   small press on click. Buttons and chips have their own states above. */
:is(.rc-card, .rc-stat, .rc-item, .rc-bar, .rc-callout, tr):is([data-rc-send], [data-rc-copy]) {
  border-radius: var(--rc-radius-sm); transition: background-color 90ms ease-out, transform 90ms ease-out; }
:is(.rc-item, .rc-bar, .rc-callout):is([data-rc-send], [data-rc-copy]):hover,
tr:is([data-rc-send], [data-rc-copy]):hover > td { background: color-mix(in srgb, var(--rc-text) 6%, transparent); }
:is(.rc-card, .rc-stat):is([data-rc-send], [data-rc-copy]):hover { background: color-mix(in srgb, var(--rc-text) 6%, var(--rc-surface)); }
:is(.rc-card, .rc-stat, .rc-item, .rc-callout):is([data-rc-send], [data-rc-copy]):active { transform: scale(0.985); }
.rc-item.rc-item:is([data-rc-send], [data-rc-copy]) { padding-left: 8px; padding-right: 8px; }

.rc-progress { height: 8px; overflow: hidden; border-radius: var(--rc-radius-pill); background: color-mix(in srgb, var(--rc-text) 10%, transparent); }
.rc-progress > span { display: block; height: 100%; border-radius: inherit; background: var(--rc-tint, var(--rc-accent)); }

.rc-bars { display: grid; grid-template-columns: fit-content(40%) minmax(32px, 1fr) max-content; gap: 10px 12px; align-items: center; }
.rc-bar { display: grid; grid-column: 1 / -1; grid-template-columns: subgrid; align-items: center; position: relative;
  font-size: var(--rc-text-size-sm); line-height: 20px; }
.rc-bar::before { content: ""; grid-area: 1 / 2; height: 22px; border-radius: 6px; background: color-mix(in srgb, var(--rc-text) 10%, transparent); }
.rc-bar > span { grid-area: 1 / 1; color: var(--rc-muted); }
.rc-bar > i { grid-area: 1 / 2; justify-self: start; min-width: 6px; height: 22px; border-radius: 6px; background: var(--rc-tint, var(--rc-accent)); }
.rc-bar > b { grid-area: 1 / 3; font-weight: 500; font-variant-numeric: tabular-nums; text-align: right; }
.rc-bar:hover::before { background: color-mix(in srgb, var(--rc-text) 16%, transparent); }
.rc-bar:hover > span, .rc-bar:hover > b { color: var(--rc-text); }
.rc-bar:hover > i { background: color-mix(in srgb, var(--rc-tint, var(--rc-accent)) 82%, var(--rc-text)); }
/* Hover text is laid out all along and only revealed, so it never changes the page's size. */
.rc-bar[data-rc-tip]::after { content: attr(data-rc-tip); display: block; position: absolute; right: 0; bottom: calc(100% + 2px);
  z-index: 2; max-width: 100%; padding: 3px 8px; font-size: var(--rc-text-size-xs); line-height: 15px; color: var(--rc-text);
  background: var(--rc-surface); border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-sm); pointer-events: none;
  visibility: hidden; opacity: 0; transition: opacity 80ms ease-out; }
.rc-bar[data-rc-tip]:hover::after { visibility: visible; opacity: 1; }
/* Above the first bar is the view's heading and, at its right, the copy and save buttons the app
   draws over the view: that tip opens below its bar instead, where the next bar is. */
.rc-bar[data-rc-tip]:first-child::after { bottom: auto; top: calc(100% + 2px); }

/* A table wraps prose at spaces only (anywhere lets auto layout squeeze a column to one letter).
   Inline code gets <wbr> at path/identifier boundaries (kit() below) and uses break-word so it
   wraps at / . _ - : before breaking a word; keys and raw URLs break anywhere so a long URL
   never widens the table past the reply. A table that still cannot fit scrolls in its own box. */
.rc-table { overflow-wrap: normal; font-variant-numeric: tabular-nums; }
.rc-table code { overflow-wrap: break-word; }
.rc-table :is(kbd, a) { overflow-wrap: anywhere; }
.rc-table .rc-num { white-space: nowrap; }
.rc-table-box { overflow-x: auto; overscroll-behavior-x: contain; }
.rc-table th { position: sticky; top: 0; z-index: 1; font-size: var(--rc-text-size-sm); font-weight: 500; background: var(--rc-bg);
  border-bottom: 0; box-shadow: inset 0 -1px var(--rc-separator); }
.rc-table tbody tr:hover > td { background: color-mix(in srgb, var(--rc-text) 4.5%, transparent); }
.rc-table[data-rc-sort] th { cursor: pointer; user-select: none; }
.rc-table[data-rc-sort] th:hover { color: var(--rc-text); }
:root[data-keyboard-nav] .rc-table[data-rc-sort] th:focus-visible { outline-offset: -2px; }
.rc-table th[aria-sort] { color: var(--rc-text); }
.rc-table th[aria-sort=ascending]::after { content: "\u2191"; margin-left: 3px; color: var(--rc-muted); }
.rc-table th[aria-sort=descending]::after { content: "\u2193"; margin-left: 3px; color: var(--rc-muted); }

/* A still-streaming fragment runs no script, so its canvases can never draw: a quiet
   box of the same size says "a chart lands here" instead of an empty one. */
#rc-stream .rc-chart { border-radius: var(--rc-radius); background: color-mix(in srgb, var(--rc-text) 4%, transparent); }
#rc-stream .rc-chart > canvas { visibility: hidden; }
/* One chart height, from one class: data-size steps it instead of an inline height. */
.rc-chart { position: relative; height: 240px; }
.rc-chart[data-size=sm] { height: 160px; }
.rc-chart[data-size=lg] { height: 300px; }
.rc-chart[data-size=xl] { height: 380px; }
.rc-chart > canvas { display: block; }
.rc-donut-total { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center;
  pointer-events: none; font-variant-numeric: tabular-nums; }
.rc-donut-total > b { font-size: var(--rc-text-size-value); line-height: 1.25; font-weight: 600; letter-spacing: -0.01em; }
.rc-donut-total > span { font-size: var(--rc-text-size-xs); line-height: 14px; color: var(--rc-muted); }

.rc-spark { display: inline-block; width: 64px; height: 20px; color: var(--rc-tint, var(--rc-accent)); vertical-align: middle; }
.rc-spark > svg { display: block; width: 100%; height: 100%; overflow: visible; }
.rc-spark-line { fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round;
  vector-effect: non-scaling-stroke; }
.rc-spark-area { fill: currentColor; opacity: 0.14; }

.rc-list { display: flex; flex-direction: column; padding: 0; list-style: none; }
.rc-item { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 6px 12px; padding: 6px 0;
  border-bottom: 1px solid var(--rc-separator); }
.rc-item:last-child { border-bottom: 0; }
.rc-item:has(> div, > p) { justify-content: flex-start; }
.rc-item > * { min-width: 0; overflow-wrap: anywhere; }
.rc-item > .rc-badge, .rc-item > .rc-chip { flex: none; }
.rc-item > :last-child:not(:first-child) { font-variant-numeric: tabular-nums; }
.rc-item > div, .rc-item > p { flex: 0 0 100%; margin: 0; }
/* A titled item's detail is the secondary line: smaller and muted, so the titles carry the list. */
.rc-item > p { color: var(--rc-muted); font-size: var(--rc-text-size-sm); }
.rc-item > :is(b, strong):first-child { font-weight: 500; }
.rc-item > .rc-item-value { margin-left: auto; color: var(--rc-tint, var(--rc-muted)); font-size: var(--rc-text-size-sm);
  font-variant-numeric: tabular-nums; white-space: nowrap; }
.rc-item.rc-prose:is(.rc-ok, .rc-warn, .rc-bad, .rc-info) { padding-left: 10px;
  box-shadow: inset 2px 0 color-mix(in srgb, var(--rc-tint) 70%, transparent); }
/* An item written as a sentence (bold lead, words, code) reads as one paragraph: as flex items,
   each bold, code and run of text became its own box, spread across the row. The runtime marks
   any item with bare text in it. A titled point (a <b>, then a <p>) puts its body under the title. */
.rc-item.rc-prose { display: block; }
.rc-item:is(.rc-prose, :has(> div, > p)) { padding: 8px 0; }
.rc-item:has(> div, > p) { row-gap: 2px; }

.rc-steps { padding: 0; list-style: none; }
.rc-step { position: relative; padding: 0 0 12px 20px; }
.rc-step:last-child { padding-bottom: 0; }
.rc-step::before { content: ""; position: absolute; left: 1px; top: calc(0.75em - 4px); width: 8px; height: 8px; border-radius: 50%;
  background: var(--rc-faint); }
.rc-step::after { content: ""; position: absolute; left: 4.5px; top: calc(0.75em + 8px); bottom: -2px; width: 1px; background: var(--rc-separator); }
.rc-step:last-child::after { display: none; }
.rc-done::before { background: var(--rc-success); }
.rc-active::before { background: var(--rc-accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--rc-accent) 22%, transparent); }
.rc-failed::before { background: var(--rc-danger); }
/* A plan that has not started reads at full strength; once some steps are done or running,
   the ones still to do step back. */
.rc-steps:has(.rc-done, .rc-active, .rc-failed) .rc-todo { color: var(--rc-muted); }
.rc-todo::before { background: transparent; box-shadow: inset 0 0 0 1.5px var(--rc-faint); }

/* A review's findings: the severity in its own column, then where (file:line) and who found it,
   the finding, and what the author did about it. */
.rc-findings { display: flex; flex-direction: column; }
.rc-finding { display: grid; grid-template-columns: 64px minmax(0, 1fr); column-gap: 12px; row-gap: 2px; padding: 8px 0;
  border-bottom: 1px solid var(--rc-separator); }
.rc-finding:last-child { border-bottom: 0; }
.rc-finding::before { content: attr(data-severity); grid-row: 1 / span 8; font-weight: 600; text-transform: capitalize; }
.rc-finding[data-severity="high" i]::before, .rc-finding[data-severity="critical" i]::before { color: var(--rc-danger); }
.rc-finding[data-severity="medium" i]::before { color: var(--rc-warning); }
.rc-finding[data-severity="low" i]::before, .rc-finding[data-severity="nit" i]::before { color: var(--rc-muted); }
.rc-finding > * { grid-column: 2; margin: 0; min-width: 0; overflow-wrap: anywhere; }
.rc-finding-note { color: var(--rc-muted); font-size: var(--rc-text-size-sm); }
.rc-finding-note.rc-ok::before { content: "\\2713\\00a0"; color: var(--rc-success); }

.rc-details { border-bottom: 1px solid var(--rc-separator); }
.rc-details:last-child { border-bottom: 0; }
/* Any fold gets the kit's summary: a bare <details> or a sources fold drew the browser's own
   triangle with no pointer, and the count ran into its label ("Sources3"). */
details > summary { display: flex; align-items: center; gap: 8px; padding: 6px 0; font-weight: 500; cursor: pointer; list-style: none;
  user-select: none; }
details > summary::-webkit-details-marker { display: none; }
details > summary::before { content: ""; flex: none; width: 6px; height: 6px; margin: 0 2px;
  border-right: 1.5px solid var(--rc-muted); border-bottom: 1.5px solid var(--rc-muted);
  transform: rotate(-45deg); transition: transform 120ms ease-out; }
details[open] > summary::before { transform: rotate(45deg); }
details > summary:hover::before { border-color: var(--rc-text); }
.rc-details > summary > :last-child:not(:first-child, .rc-sources-count) { margin-left: auto; font-weight: 400; }
:is(.rc-details, .rc-sources) > :not(summary) { margin-bottom: 8px; }
.rc-tree .rc-details { border-bottom: 0; }
.rc-tree .rc-details > :not(summary) { margin: 0 0 0 5px; padding-left: 12px; border-left: 1px solid var(--rc-separator); }
.rc-sources { border-bottom: 0; font-size: var(--rc-text-size-sm); }
.rc-sources > summary { width: fit-content; gap: 6px; padding: 4px 0; font-weight: 400; color: var(--rc-muted); }
.rc-sources > summary::before { width: 5px; height: 5px; }
.rc-sources > summary:focus-visible { outline: 2px solid var(--rc-accent); outline-offset: 2px; }
.rc-sources > summary:hover, .rc-sources[open] > summary { color: var(--rc-text); }
.rc-sources > summary > .rc-sources-count { min-width: 18px; padding: 0 5px; font-size: var(--rc-text-size-xs); line-height: 16px; text-align: center;
  color: var(--rc-muted); font-variant-numeric: tabular-nums; border-radius: var(--rc-radius-pill); background: color-mix(in srgb, var(--rc-text) 9%, transparent); }
.rc-sources :is(ol, ul) { display: grid; gap: 4px; margin: 2px 0 4px; padding-left: 24px; color: var(--rc-muted); }
.rc-sources li::marker { font-variant-numeric: tabular-nums; color: var(--rc-faint); }
.rc-sources a { text-decoration: none; overflow-wrap: anywhere; }
.rc-sources a:hover { text-decoration: underline; }
.rc-sources-host { margin-left: 6px; font-size: var(--rc-text-size-xs); color: var(--rc-faint); }
.rc-file { display: flex; justify-content: space-between; gap: 12px; padding: 2px 0; font: var(--rc-text-size-sm)/18px var(--rc-mono); }
.rc-file > :last-child:not(:first-child) { flex: none; color: var(--rc-muted); }

.rc-term { padding: 10px 12px; overflow-x: auto; font: var(--rc-text-size-sm)/18px var(--rc-mono); background: var(--rc-surface);
  border-radius: var(--rc-radius); }
.rc-term > * { display: block; white-space: pre-wrap; }
pre.rc-term { white-space: pre-wrap; }
.rc-term .rc-ok, .rc-term .rc-warn, .rc-term .rc-bad, .rc-term .rc-info { color: var(--rc-tint); }

.rc-tabs [role=tablist] { display: inline-flex; max-width: 100%; gap: 2px; padding: 2px; overflow-x: auto;
  background: var(--rc-surface); border-radius: var(--rc-radius-sm); }
.rc-tabs [role=tab] { flex: none; padding: 1px 10px; font-size: var(--rc-text-size-sm); line-height: 20px; color: var(--rc-muted);
  background: transparent; border: 0; border-radius: var(--rc-radius-xs); }
.rc-tabs [role=tab]:hover { color: var(--rc-text); background: transparent; }
.rc-tabs [role=tab][aria-selected=true] { color: var(--rc-text); background: var(--rc-bg); }
.rc-tabs [data-rc-panel] { margin-top: 10px; }
.rc-tabs [data-rc-panel][hidden] { display: none; }

.rc-tip { padding: 0 5px; font-size: var(--rc-text-size-xs); line-height: 14px; font-weight: 500; color: var(--rc-muted); cursor: help; vertical-align: 1px;
  border-radius: var(--rc-radius-pill); background: color-mix(in srgb, var(--rc-text) 9%, transparent); }
/* Laid out all along and only revealed: a tip that shows must never grow the page. */
.rc-tip > span { position: absolute; left: 0; right: 0; z-index: 2; width: fit-content; max-width: min(320px, 100%);
  margin: 18px auto 0; padding: 6px 10px; font-size: var(--rc-text-size-sm); line-height: 1.4; font-weight: 400; color: var(--rc-text);
  background: var(--rc-surface); border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-sm);
  visibility: hidden; opacity: 0; transition: opacity 80ms ease-out; }
.rc-tip:hover > span, .rc-tip:focus > span { visibility: visible; opacity: 1; }
/* The shorthand catalog's own parts (shorthand.ts). Each one is built from the kit above: the
   same surfaces, tints, type scale and rhythm, so a block the agent names looks like the rest. */
.rc-meters { display: grid; gap: 12px; }
.rc-meter { display: grid; gap: 4px; min-width: 0; }
.rc-meter-head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; font-size: var(--rc-text-size-sm); }
.rc-meter-head > span { color: var(--rc-muted); min-width: 0; }
.rc-meter-head > b { font-weight: 500; font-variant-numeric: tabular-nums; white-space: nowrap; }
.rc-meter-note { font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }
.rc-meter:is(.rc-ok, .rc-warn, .rc-bad, .rc-info) > .rc-meter-head > span::before { content: ""; display: inline-block; width: 6px; height: 6px;
  margin: 0 7px 1px 0; border-radius: 50%; background: var(--rc-tint); vertical-align: middle; }
.rc-stat > .rc-spark { width: 100%; height: 22px; margin-top: 6px; }
.rc-kv { display: grid; grid-template-columns: fit-content(42%) minmax(0, 1fr); gap: 0; margin: 0; font-size: var(--rc-text-size-sm); }
.rc-kv > dt, .rc-kv > dd { margin: 0; padding: 6px 0; border-bottom: 1px solid var(--rc-separator); min-width: 0; }
.rc-kv > dt { padding-right: 16px; color: var(--rc-muted); }
.rc-kv > dd { font-variant-numeric: tabular-nums; }
.rc-kv > :nth-last-child(-n+2) { border-bottom: 0; }
.rc-timeline .rc-step::before { background: var(--rc-tint, var(--rc-faint)); }
.rc-timeline .rc-step > b { font-weight: 500; }
.rc-timeline .rc-step > p { color: var(--rc-muted); font-size: var(--rc-text-size-sm); }
.rc-time { display: block; font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); font-variant-numeric: tabular-nums; }
.rc-files { display: flex; flex-direction: column; }
.rc-files > .rc-file { align-items: baseline; padding: 4px 0; border-bottom: 1px solid var(--rc-separator); font-family: var(--rc-font); }
.rc-files > .rc-file:last-child { border-bottom: 0; }
.rc-files > .rc-file > span:first-child { min-width: 0; overflow-wrap: anywhere; }
.rc-files code { padding: 0; background: transparent; }
.rc-file:is(.rc-ok, .rc-warn, .rc-bad, .rc-info) > span:first-child::before { content: ""; display: inline-block; width: 6px; height: 6px;
  margin: 0 7px 1px 0; border-radius: 50%; background: var(--rc-tint); vertical-align: middle; }
.rc-file-note { margin-left: 8px; font-size: var(--rc-text-size-xs); color: var(--rc-muted); }
.rc-file > :last-child:not(:first-child) { font: var(--rc-text-size-sm)/18px var(--rc-mono); font-variant-numeric: tabular-nums; }
.rc-add { color: var(--rc-success); } .rc-del { color: var(--rc-danger); }
.rc-mark { font-weight: 600; color: var(--rc-tint); }
.rc-compare td:not(:first-child), .rc-compare th:not(:first-child) { text-align: center; }
.rc-compare td > b { font-weight: 500; }
.rc-heatmap td.rc-heat { background: color-mix(in srgb, var(--rc-accent) var(--rc-heat, 0%), transparent); }
.rc-heatmap tbody tr:hover > td.rc-heat { background: color-mix(in srgb, var(--rc-accent) calc(var(--rc-heat, 0%) + 8%), transparent); }
.rc-heatmap td { border-bottom-color: var(--rc-bg); }
.rc-chart-split { display: flex; flex-wrap: wrap; align-items: center; gap: 12px 20px; }
.rc-chart-split > .rc-chart { flex: 0 0 160px; width: 160px; }
.rc-legend { display: grid; flex: 1 1 160px; gap: 4px; margin: 0; padding: 0; list-style: none; font-size: var(--rc-text-size-sm); }
.rc-legend > li { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
.rc-legend > li > span { color: var(--rc-muted); min-width: 0; }
.rc-legend > li > span::before { content: ""; display: inline-block; width: 8px; height: 8px; margin: 0 7px 0 0; border-radius: 50%;
  background: var(--rc-tint); }
.rc-legend > li > b { font-weight: 500; font-variant-numeric: tabular-nums; }
.rc-form { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 14px 20px; align-items: start; }
.rc-form > .rc-row { grid-column: 1 / -1; padding-top: 2px; }
.rc-field { display: grid; gap: 6px; min-width: 0; }
.rc-field > span:first-child { font-size: var(--rc-text-size-xs); font-weight: 500; line-height: 1.45; color: var(--rc-muted); }
.rc-field > :is(input:not([type=checkbox], [type=radio]), select) { width: 100%; min-width: 0; }
.rc-field.rc-check { grid-column: 1 / -1; display: flex; align-items: flex-start; gap: 10px; cursor: pointer; }
.rc-form > .rc-check + .rc-check { margin-top: -6px; }
.rc-field.rc-check > span { font-size: var(--rc-text-size); line-height: 1.45; color: var(--rc-text); }
.rc-check > input, .rc-choice-list input { appearance: none; flex: none; display: grid; place-content: center; width: 16px; height: 16px;
  margin: 2px 0 0; padding: 0; border: 1.5px solid color-mix(in srgb, var(--rc-text) 35%, transparent); background: transparent; }
.rc-check > input { border-radius: 4px; }
.rc-check > input::after { content: ""; width: 4px; height: 8px; margin-top: -2px; border: solid var(--rc-accent-text);
  border-width: 0 2px 2px 0; transform: rotate(45deg) scale(0); transition: transform 90ms ease-out; }
.rc-check > input:checked { background: var(--rc-accent); border-color: var(--rc-accent); }
.rc-check > input:checked::after { transform: rotate(45deg) scale(1); }
.rc-check:hover > input:not(:checked), .rc-choice-list label:hover input:not(:checked) { border-color: color-mix(in srgb, var(--rc-text) 60%, transparent); }
.rc-range { display: flex; align-items: center; gap: 8px; }
.rc-range > input { flex: 1; min-width: 0; padding: 0; border: 0; background: transparent; }
.rc-range > output { min-width: 3ch; text-align: right; font-size: var(--rc-text-size-sm); }
.rc-choice { display: inline-flex; flex-wrap: wrap; width: fit-content; max-width: 100%; gap: 2px; padding: 2px;
  background: var(--rc-surface); border-radius: var(--rc-radius-sm); }
.rc-choice > label { position: relative; cursor: pointer; }
.rc-choice:not(.rc-choice-list) input { position: absolute; opacity: 0; pointer-events: none; }
.rc-choice:not(.rc-choice-list) span { display: block; padding: 2px 12px; font-size: var(--rc-text-size-sm); line-height: 22px; color: var(--rc-muted);
  white-space: nowrap; border-radius: var(--rc-radius-xs); transition: background-color 90ms ease-out, color 90ms ease-out; }
.rc-choice:not(.rc-choice-list) label:hover span { color: var(--rc-text); }
.rc-choice:not(.rc-choice-list) input:checked + span { color: var(--rc-text); background: var(--rc-bg);
  box-shadow: 0 1px 2px color-mix(in srgb, black 25%, transparent); }
:root[data-keyboard-nav] .rc-choice input:focus-visible + span { outline: 2px solid color-mix(in srgb, var(--rc-accent) 60%, transparent); }
.rc-choice-list { display: grid; width: 100%; gap: 0; padding: 3px; }
.rc-choice-list > label { display: flex; align-items: flex-start; gap: 10px; padding: 6px 9px; border-radius: var(--rc-radius-xs); }
.rc-choice-list > label:hover { background: color-mix(in srgb, var(--rc-text) 5%, transparent); }
.rc-choice-list > label:has(input:checked) { background: color-mix(in srgb, var(--rc-text) 8%, transparent); }
.rc-choice-list input { border-radius: 50%; }
.rc-choice-list input::after { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--rc-accent-text);
  transform: scale(0); transition: transform 90ms ease-out; }
.rc-choice-list input:checked { background: var(--rc-accent); border-color: var(--rc-accent); }
.rc-choice-list input:checked::after { transform: scale(1); }
.rc-choice-list span { font-size: var(--rc-text-size-sm); line-height: 20px; color: var(--rc-muted); }
.rc-choice-list input:checked + span { color: var(--rc-text); }
:root[data-keyboard-nav] .rc-choice-list input:focus-visible { outline: 2px solid color-mix(in srgb, var(--rc-accent) 60%, transparent); outline-offset: 1px; }
pre[data-lang] { position: relative; }
pre[data-lang]::before { content: attr(data-lang); position: absolute; top: 6px; right: 10px; font: var(--rc-text-size-xs)/1 var(--rc-font);
  color: var(--rc-faint); }
@media (prefers-reduced-motion: reduce) { details > summary::before, .rc-bar[data-rc-tip]::after, button, [data-rc-send], [data-rc-copy] { transition: none; }
  :is(button, [data-rc-send], [data-rc-copy]):active { transform: none; } }
`;

/**
 * The runtime, first in the page. It talks to the app only by postMessage
 * with this frame's token; the app checks the message comes from this frame.
 */
export function runtimeScript(token: string, theme: PageTheme): string {
  return `(() => {
  const token = ${JSON.stringify(token)};
  const parent = window.parent;
  let theme = ${JSON.stringify(theme.mode)};
  const listeners = [];
  const post = (message) => parent.postMessage(Object.assign({ rc: token }, message), "*");
  window.open = () => null;
  window.alert = () => undefined;
  window.confirm = () => false;
  window.prompt = () => null;
  // A second argument rides after the text as a JSON fence, so a view can send values back.
  const compose = (text, context) => {
    text = String(text == null ? "" : text).trim();
    if (context && typeof context === "object") {
      try {
        const json = JSON.stringify(context, null, 2);
        if (json) text = (text ? text + "\\n\\n" : "") + "\`\`\`json\\n" + json + "\\n\`\`\`";
      } catch (e) {
        // Not serialisable: the text still goes.
      }
    }
    return text.slice(0, 2000);
  };
  const rustcode = {
    send(text, context) {
      if (!(navigator.userActivation ? navigator.userActivation.isActive : true)) return false;
      text = compose(text, context);
      if (!text.trim()) return false;
      post({ type: "send", text });
      return true;
    },
    copy(text) { post({ type: "copy", text: String(text == null ? "" : text).slice(0, 200000) }); },
    get theme() { return theme; },
    onThemeChange(callback) { if (typeof callback === "function") listeners.push(callback); },
  };
  Object.defineProperty(window, "rustcode", { value: Object.freeze(rustcode) });
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.source !== parent || !data || data.rc !== token) return;
    if (data.type === "focus-modality") {
      document.documentElement.toggleAttribute("data-keyboard-nav", data.keyboard === true);
      tabFocus = data.tabFocus !== false;
      return;
    }
    if (data.type !== "theme") return;
    const style = document.getElementById("rc-vars");
    if (style) style.textContent = String(data.css || "");
    theme = data.mode === "dark" ? "dark" : "light";
    if (window.__rcThemeCharts) window.__rcThemeCharts();
    for (const callback of listeners) { try { callback(theme); } catch (e) { console.error(e); } }
  });
  // One selection with the reply around the view (src/smartui/selection.ts): the app says what
  // part of the page its selection covers, a drag that starts here and leaves the page carries on
  // in the app, and the page reports what it has selected for the app's copy.
  let pressed = false;
  let driven = false;
  const selectionText = () => {
    const selection = document.getSelection();
    return selection ? selection.toString().slice(0, 200000) : "";
  };
  const caretAt = (x, y) => {
    x = Math.max(0, Math.min(window.innerWidth - 1, x));
    y = Math.max(0, Math.min(window.innerHeight - 1, y));
    if (document.caretPositionFromPoint) {
      const p = document.caretPositionFromPoint(x, y);
      if (p) return [p.offsetNode, p.offset];
    }
    if (document.caretRangeFromPoint) {
      const r = document.caretRangeFromPoint(x, y);
      if (r) return [r.startContainer, r.startOffset];
    }
    return null;
  };
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.source !== parent || !data || data.rc !== token || data.type !== "selection") return;
    const selection = document.getSelection();
    if (!selection || !document.body) return;
    if (data.mode === "extend") {
      // A drag from here left the page and the app has the pointer: run on to this edge.
      if (selection.rangeCount > 0) {
        selection.extend(document.body, data.to === "end" ? document.body.childNodes.length : 0);
      }
      return;
    }
    // The app sends a part only after a press of its own: a drag from here has ended.
    pressed = false;
    if (data.mode === "none") {
      if (driven) selection.removeAllRanges();
      driven = false;
      return;
    }
    const range = document.createRange();
    range.selectNodeContents(document.body);
    if (data.mode === "head" || data.mode === "tail") {
      const caret = caretAt(Number(data.x), Number(data.y));
      if (caret && data.mode === "head") range.setEnd(caret[0], caret[1]);
      else if (caret) range.setStart(caret[0], caret[1]);
    }
    selection.removeAllRanges();
    selection.addRange(range);
    driven = true;
  });
  document.addEventListener("selectionchange", () => post({ type: "selection-text", text: selectionText() }));
  document.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    pressed = true;
    driven = false;
    post({ type: "selection-press" });
  }, true);
  window.addEventListener("mouseup", () => { pressed = false; }, true);
  document.addEventListener("mousemove", (event) => {
    if (!(event.buttons & 1)) { pressed = false; return; }
    // A drag from here that may leave the page, or one from the app whose pointer the page got.
    post({ type: pressed ? "drag-out" : "drag-over", x: event.clientX, y: event.clientY });
  }, true);
  document.addEventListener("copy", () => post({ type: "selection-copy" }));
  const setKeyboardNav = (active) => {
    document.documentElement.toggleAttribute("data-keyboard-nav", active);
    post({ type: "focus-modality", keyboard: active });
  };
  // The app's "Tab moves focus between controls" (lib/focusModality.ts), sent with the modality.
  // A bare Tab is cancelled with it off, except inside a form, as in the app.
  let tabFocus = false;
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const bare = !event.ctrlKey && !event.metaKey && !event.altKey;
    const target = event.target;
    const inForm = target instanceof Element && !!target.closest("form");
    if (!tabFocus && bare && !inForm) {
      event.preventDefault();
      setKeyboardNav(false);
      return;
    }
    setKeyboardNav(true);
  }, true);
  const pointer = () => setKeyboardNav(false);
  window.addEventListener("pointerdown", pointer, true);
  window.addEventListener("mousedown", pointer, true);
  // When the user last pressed something in this page. A size that changes right after
  // (a tab, a fold, a sort) is the user's doing and applies at once, even with the pointer
  // inside; an arrow key that switches a tab grants no user activation the app could see.
  let gestureAt = -Infinity;
  const gesture = () => { gestureAt = performance.now(); };
  window.addEventListener("pointerdown", gesture, true);
  window.addEventListener("keydown", gesture, true);
  // The content, not the document: the root holds what the view wrote, in flow, and a
  // hover overlay (a tip, a bar's label) is placed out of it, so revealing one never
  // changes the size the app reserves (issue 3304).
  const contentHeight = () => {
    const root = document.getElementById("rc-root") || document.body;
    const style = getComputedStyle(root);
    const body = getComputedStyle(document.body);
    return root.getBoundingClientRect().height +
      (parseFloat(style.marginTop) || 0) + (parseFloat(style.marginBottom) || 0) +
      (parseFloat(body.paddingTop) || 0) + (parseFloat(body.paddingBottom) || 0);
  };
  let parsed = false;
  let last = "";
  const report = () => {
    // Not while parsing: a half-built body would shrink the height the app reserved.
    if (!parsed) return;
    const height = contentHeight();
    const elements = document.getElementsByTagName("*").length;
    const next = height + ":" + elements;
    if (next !== last) {
      last = next;
      post({ type: "size", height, elements, interacted: performance.now() - gestureAt < 1000 });
    }
  };
  // A heartbeat, so the app can tell a busy loop from a quiet page.
  setInterval(() => { post({ type: "alive" }); report(); }, 500);
  // Whether the pointer is in this page at all: the app holds a height that changes
  // under the pointer until it leaves, in case a hover overlay moved it.
  let hovering = false;
  const setHovering = (next) => {
    if (next === hovering) return;
    hovering = next;
    post({ type: "hover", over: next });
    if (!next) requestAnimationFrame(() => { last = ""; report(); });
  };
  document.addEventListener("pointerover", () => setHovering(true), true);
  document.addEventListener("pointerout", (event) => { if (!event.relatedTarget) setHovering(false); }, true);

  // A chart laid out while hidden (another tab, a closed fold) measured 0 x 0. Resize it
  // now, before the frame paints, rather than leave a blank box for Chart.js's own observer.
  const refitCharts = (scope) => {
    if (!window.Chart || typeof window.Chart.getChart !== "function") return;
    for (const canvas of scope.querySelectorAll("canvas")) {
      const chart = window.Chart.getChart(canvas);
      if (chart) chart.resize();
    }
  };
  // The kit's behaviour, so a view needs no script for it (docs/smart-ui.md).
  const own = (group, selector) => Array.from(group.querySelectorAll(selector)).filter((el) => el.closest(".rc-tabs") === group);
${KIT_SCRIPT}
  // A sortable table sorts a column in place, then says which way in aria-sort.
  const sortTable = (table, index) => {
    const head = table.tHead;
    const body = table.tBodies[0];
    const header = head && head.rows[0] && head.rows[0].cells[index];
    if (!body || !header) return;
    const sort = header.getAttribute("aria-sort") === "ascending" ? "descending" : "ascending";
    const kind = header.getAttribute("data-rc-sort") === "text" ? "text" : "number";
    const cell = (row) => { const c = row.cells[index]; return c ? c.textContent : ""; };
    for (const other of head.querySelectorAll("th")) if (other !== header) other.removeAttribute("aria-sort");
    header.setAttribute("aria-sort", sort);
    const rows = Array.from(body.rows).map((row, at) => ({ row, at }));
    rows.sort((a, b) => (rcCompare(cell(a.row), cell(b.row), kind) || a.at - b.at) * (sort === "ascending" ? 1 : -1));
    // Rows are moved, not rebuilt: a row's ids and the controls in it survive a sort.
    body.append(...rows.map((entry) => entry.row));
    report();
  };
  const selectTab = (group, name, focus) => {
    for (const tab of own(group, "[data-rc-tab]")) {
      const on = tab.getAttribute("data-rc-tab") === name;
      tab.setAttribute("aria-selected", on ? "true" : "false");
      tab.tabIndex = on ? 0 : -1;
      if (on && focus) tab.focus();
    }
    for (const panel of own(group, "[data-rc-panel]")) {
      panel.setAttribute("role", "tabpanel");
      const hidden = panel.getAttribute("data-rc-panel") !== name;
      const shown = panel.hidden && !hidden;
      panel.hidden = hidden;
      if (shown) refitCharts(panel);
    }
    report();
  };
  const flashes = new WeakMap();
  const flashCopied = (el) => {
    const flash = flashes.get(el);
    if (flash) clearTimeout(flash.timer);
    const nodes = flash ? flash.nodes : Array.from(el.childNodes);
    el.replaceChildren("Copied");
    const timer = setTimeout(() => { flashes.delete(el); el.replaceChildren(...nodes); }, 1500);
    flashes.set(el, { nodes, timer });
  };
  // A form's named fields ride along after the prompt, as name: value lines.
  const formText = (form) => {
    const lines = [];
    let data;
    try { data = new FormData(form); } catch (e) { return ""; }
    for (const [name, value] of data) {
      if (!name || typeof value !== "string") continue;
      const v = value.trim().replace(/\\s+/g, " ");
      if (v) lines.push(name + ": " + v.slice(0, 200));
    }
    return lines.join("\\n");
  };
  const sendForm = (form, prompt) => {
    const fields = formText(form);
    rustcode.send(fields ? (prompt ? prompt + "\\n\\n" : "") + fields : prompt);
  };
  document.addEventListener("submit", (event) => {
    // A form never navigates (CSP would not allow it). One marked data-rc-send sends its
    // prompt and its fields when submitted: its submit button, or Enter in a field.
    event.preventDefault();
    const form = event.target instanceof HTMLFormElement ? event.target : null;
    if (form && form.hasAttribute("data-rc-send")) {
      sendForm(form, (form.getAttribute("data-rc-send") || "").trim());
    }
  });
  // A slider the kit drew (\`:: form\` range) shows its value beside it as it moves.
  document.addEventListener("input", (event) => {
    const input = event.target instanceof HTMLInputElement ? event.target : null;
    if (!input || !input.hasAttribute("data-rc-out")) return;
    const out = input.nextElementSibling;
    if (out && out.tagName === "OUTPUT") out.textContent = input.value;
  });
  // A link never navigates the frame, which would replace the view. A web or mail link
  // asks the app to open it in the browser, only from a click.
  const openLink = (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const link = target && target.closest("a[href]");
    if (!link) return false;
    event.preventDefault();
    const href = (link.getAttribute("href") || "").trim();
    if (event.type === "click" && /^(https?:\\/\\/|mailto:)/i.test(href)) post({ type: "open", url: href.slice(0, 2048) });
    return true;
  };
  document.addEventListener("auxclick", openLink);
  // A fold that opens or closes changes the height now, and may reveal a chart.
  document.addEventListener("toggle", (event) => {
    if (event.target instanceof HTMLDetailsElement && event.target.open) refitCharts(event.target);
    report();
  }, true);
  const RC_PRESSABLE = ":is([data-rc-send], [data-rc-copy]):not(button, a, form, input, select, textarea, summary, label)";
  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    if (openLink(event)) return;
    const send = target.closest("[data-rc-send]");
    // A form marked data-rc-send sends on submit (above), never on a click into it. A control
    // marked inside a form sends its own text with that form's fields.
    if (send && send.tagName !== "FORM") {
      const form = send.closest("form");
      const prompt = (send.getAttribute("data-rc-send") || "").trim();
      if (form) sendForm(form, prompt);
      else rustcode.send(prompt);
    }
    const copy = target.closest("[data-rc-copy]");
    if (copy) { rustcode.copy(copy.getAttribute("data-rc-copy")); flashCopied(copy); }
    const th = target.closest("th");
    const sortable = th && th.closest("table.rc-table[data-rc-sort]");
    // A control in a heading does its own thing: sorting it too would be a surprise.
    const control = target.closest("[data-rc-send], [data-rc-copy]");
    if (sortable && !control && sortable.tHead && sortable.tHead.contains(th)) sortTable(sortable, th.cellIndex);
    const tab = target.closest("[data-rc-tab]");
    const group = tab && tab.closest(".rc-tabs");
    if (group) selectTab(group, tab.getAttribute("data-rc-tab"), false);
  });
  document.addEventListener("keydown", (event) => {
    const focused = event.target instanceof Element ? event.target : null;
    // A sortable heading is a control: Enter or Space sorts it, like a click.
    const heading = focused && focused.closest("th");
    const sortable = heading && heading.closest("table.rc-table[data-rc-sort]");
    if (sortable && (event.key === "Enter" || event.key === " ") && sortable.tHead && sortable.tHead.contains(heading)) {
      event.preventDefault();
      sortTable(sortable, heading.cellIndex);
      return;
    }
    // A card, row or bar marked data-rc-send is a button too: Enter or Space presses it.
    if (focused && (event.key === "Enter" || event.key === " ") && focused.matches(RC_PRESSABLE)) {
      event.preventDefault();
      focused.click();
      return;
    }
    const tab = focused ? focused.closest("[data-rc-tab]") : null;
    const group = tab && tab.closest(".rc-tabs");
    if (!group) return;
    const tabs = own(group, "[data-rc-tab]");
    const at = tabs.indexOf(tab);
    const to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: tabs.length - 1 }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    selectTab(group, tabs[(to + tabs.length) % tabs.length].getAttribute("data-rc-tab"), true);
  });
  // The parts of the kit that need the document: the first tab of each group, a box
  // around a wide table so it scrolls by itself on a narrow window (one in .rc-scroll already does),
  // a sortable table's headings, and an inline sparkline for data-values.
  // Idempotent, so a streamed fragment can be set up again as it grows.
  const kit = () => {
    for (const group of document.querySelectorAll(".rc-tabs")) {
      const first = own(group, "[data-rc-tab]")[0];
      if (first) selectTab(group, first.getAttribute("data-rc-tab"), false);
    }
    for (const table of document.querySelectorAll("table.rc-table")) {
      const parent = table.parentElement;
      if (table.closest(".rc-scroll") || (parent && parent.classList.contains("rc-table-box"))) continue;
      const box = document.createElement("div");
      box.className = "rc-table-box";
      table.replaceWith(box);
      box.append(table);
    }
    // An item with bare text in it is a sentence, not a label and a value: lay it out as one.
    for (const item of document.querySelectorAll(".rc-item:not(.rc-prose)")) {
      for (const node of item.childNodes) {
        if (node.nodeType === 3 && node.textContent.trim()) {
          item.classList.add("rc-prose");
          break;
        }
      }
    }
    for (const table of document.querySelectorAll("table.rc-table[data-rc-sort]")) {
      for (const heading of (table.tHead ? table.tHead.rows[0].cells : [])) heading.tabIndex = 0;
    }
    // A card, row, stat or bar that sends or copies is reachable by Tab and named a button.
    for (const el of document.querySelectorAll(RC_PRESSABLE)) {
      if (!el.hasAttribute("tabindex")) el.tabIndex = 0;
      if (!el.hasAttribute("role")) el.setAttribute("role", "button");
    }
    // Insert soft break points (<wbr>) after / . _ - : inside inline <code> so paths wrap at
    // boundaries instead of mid-word, without altering copied text.
    for (const code of document.querySelectorAll(":not(pre) > code:not([data-rc-wbr])")) {
      code.setAttribute("data-rc-wbr", "");
      const walk = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
      const nodes = [];
      let n;
      while ((n = walk.nextNode())) nodes.push(n);
      for (const node of nodes) {
        const parts = rcCodeParts(node.nodeValue || "");
        if (parts.length < 2) continue;
        const frag = document.createDocumentFragment();
        parts.forEach((part, i) => {
          if (i > 0) frag.append(document.createElement("wbr"));
          frag.append(part);
        });
        node.replaceWith(frag);
      }
    }
    for (const link of document.querySelectorAll("a[href]:not(.rc-github-chip)")) {
      const href = link.getAttribute("href");
      const ref = ${GITHUB_REF_URL}.exec(href);
      const number = Number(ref?.[3]);
      if (!ref || !Number.isSafeInteger(number) || number <= 0) continue;
      const pull = ref[2].toLowerCase() === "pull";
      const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      icon.setAttribute("viewBox", "0 0 24 24");
      icon.setAttribute("aria-hidden", "true");
      icon.setAttribute("fill", "none");
      icon.setAttribute("stroke", "currentColor");
      icon.setAttribute("stroke-width", "2");
      icon.innerHTML = pull
        ? '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="18" r="3"/><path d="M6 9v6m7-9h2a3 3 0 0 1 3 3v6"/>'
        : '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2"/>';
      const label = document.createElement("span");
      if (!link.textContent.trim() || link.textContent.trim() === href) {
        label.textContent = (pull ? "PR #" : "Issue #") + number;
      } else {
        label.append(...link.childNodes);
      }
      if (!link.hasAttribute("title")) link.title = href;
      link.replaceChildren(icon, label);
      link.classList.add("rc-github-chip");
    }
    // A sources fold says how many it holds and which site each link is on.
    for (const sources of document.querySelectorAll("details.rc-sources")) {
      const links = sources.querySelectorAll("li a[href]");
      const summary = sources.querySelector(":scope > summary");
      if (summary && links.length) {
        let count = summary.querySelector(":scope > .rc-sources-count");
        if (!count) {
          count = document.createElement("span");
          count.className = "rc-sources-count";
          summary.append(count);
        }
        count.textContent = String(links.length);
      }
      for (const link of links) {
        if (link.nextElementSibling && link.nextElementSibling.classList.contains("rc-sources-host")) continue;
        const host = rcHost(link.getAttribute("href"));
        if (!host || link.textContent.trim().toLowerCase() === host) continue;
        const label = document.createElement("span");
        label.className = "rc-sources-host";
        label.textContent = host;
        link.after(label);
      }
    }
    for (const spark of document.querySelectorAll(".rc-spark[data-values]")) {
      if (spark.firstElementChild) continue;
      const values = rcSparkValues(spark.getAttribute("data-values"));
      if (values.length < 2) continue;
      const ns = "http://www.w3.org/2000/svg";
      const svg = document.createElementNS(ns, "svg");
      svg.setAttribute("viewBox", "0 0 100 30");
      svg.setAttribute("preserveAspectRatio", "none");
      svg.setAttribute("aria-hidden", "true");
      const points = rcSparkPoints(values, 100, 30);
      const area = document.createElementNS(ns, "polygon");
      area.setAttribute("points", "0,30 " + points + " 100,30");
      area.setAttribute("class", "rc-spark-area");
      const line = document.createElementNS(ns, "polyline");
      line.setAttribute("points", points);
      line.setAttribute("class", "rc-spark-line");
      svg.append(area, line);
      spark.prepend(svg);
    }
  };
  // A block that is still streaming: the app injects the fragment as it arrives, already
  // closed so it renders. Injected HTML never runs a script, so the view's own script waits
  // for the finished page (docs/smart-ui.md).
  let streamHtml = null;
  const applyStream = (html) => {
    // Looked up now: this runtime runs in the head, before the box exists.
    const box = document.getElementById("rc-stream");
    if (!box) return;
    if (!parsed) { streamHtml = html; return; }
    box.innerHTML = html;
    kit();
    report();
  };
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.source !== parent || !data || data.rc !== token || data.type !== "stream") return;
    applyStream(String(data.html == null ? "" : data.html));
  });
  document.addEventListener("DOMContentLoaded", () => {
    kit();
    parsed = true;
    if (streamHtml !== null) applyStream(streamHtml);
    report();
    const observer = new ResizeObserver(report);
    observer.observe(document.documentElement);
    observer.observe(document.body);
    const root = document.getElementById("rc-root");
    if (root) observer.observe(root);
    // The frame can take a fragment from here on.
    post({ type: "stream-ready" });
  });
  window.addEventListener("load", report);
  window.addEventListener("error", (e) => post({ type: "error", message: String(e.message || "Script error") }));
  window.addEventListener("unhandledrejection", (e) => post({ type: "error", message: String(e.reason) }));
})();`;
}

/**
 * After Chart.js: the defaults a view would otherwise set by hand. Text, grid
 * lines and the eight series colours come from the variables, tooltips and the
 * legend follow them, a line is filled from its own colour to nothing, and a
 * hover highlights what is under the pointer. A theme change re-applies all of
 * it to the charts already on the page.
 *
 * Chart.js draws on a canvas, which has no font features, so its figures are
 * never tabular; a number the user scans belongs in `.rc-num` text beside the
 * chart (docs/smart-ui.md, "Charts").
 */
export const CHART_THEME_SCRIPT = `(() => {
  if (!window.Chart) return;
  const v = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
  const series = () => [1, 2, 3, 4, 5, 6, 7, 8].map((i) => v("--rc-series-" + i));
  // The series colours are hex; a canvas gradient stop needs rgba().
  const rgb = (colour) => {
    const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(String(colour));
    if (!hex) return null;
    const digits = hex[1].length === 3 ? hex[1].replace(/./g, "$&$&") : hex[1];
    return [0, 2, 4].map((at) => parseInt(digits.slice(at, at + 2), 16));
  };
  // A canvas throws on a colour it cannot parse, which would abort the whole draw,
  // so anything that is not a series hex becomes transparent instead.
  const alpha = (colour, a) => {
    const parts = rgb(colour);
    return parts ? "rgba(" + parts.join(", ") + ", " + a + ")" : "transparent";
  };
  // Only reached when a variable is missing, which a page should make impossible; a
  // chart still has to be visible, and a neutral grey reads in both themes.
  const NEUTRAL = "#8e8e93";
  const solid = (colour, fallback) => (rgb(colour) ? colour : fallback);
  // A line chart is an area chart: filled from the line's colour down to nothing.
  const wash = (context) => {
    const chart = context.chart;
    const area = chart.chartArea;
    if (!area || !chart.ctx.createLinearGradient) return alpha(context.dataset.borderColor, 0.16);
    const gradient = chart.ctx.createLinearGradient(0, area.top, 0, area.bottom);
    gradient.addColorStop(0, alpha(context.dataset.borderColor, 0.3));
    gradient.addColorStop(1, alpha(context.dataset.borderColor, 0));
    return gradient;
  };
  const radial = (t) => t === "pie" || t === "doughnut" || t === "polarArea";
  const paint = (chart) => {
    const s = series();
    const type = chart.config.type;
    chart.data.datasets.forEach((ds, i) => {
      if (ds.__rc === undefined) ds.__rc = ds.backgroundColor === undefined && ds.borderColor === undefined;
      if (!ds.__rc) return;
      const kind = ds.type || type;
      const colour = solid(s[i % 8], solid(Chart.defaults.borderColor, NEUTRAL));
      if (radial(kind)) {
        ds.backgroundColor = (chart.data.labels || ds.data).map((_, j) => solid(s[j % 8], colour));
        ds.borderColor = "transparent";
      } else if (kind === "line") {
        ds.borderColor = colour;
        if (ds.fill === undefined) ds.fill = true;
        ds.backgroundColor = wash;
      } else {
        ds.borderColor = colour;
        ds.backgroundColor = colour;
        // Hover leaves the bar under the pointer a step lighter.
        ds.hoverBackgroundColor = alpha(colour, 0.8);
      }
    });
    // A slice answers to the pointer; everything else answers along its axis,
    // so hovering anywhere in a column shows that column's values.
    if (!(chart.config.options && chart.config.options.interaction)) {
      const interaction = radial(type) ? { mode: "nearest", intersect: true } : { mode: "index", intersect: false };
      // The view's own options win; this reaches the resolver on the next update.
      if (chart.config.options) chart.config.options.interaction = interaction;
      else chart.options.interaction = interaction;
    }
  };
  const apply = () => {
    const d = Chart.defaults;
    const ink = getComputedStyle(document.documentElement).colorScheme === "dark" ? "#000" : "#fff";
    d.color = v("--rc-muted");
    d.borderColor = v("--rc-separator");
    d.font.family = v("--rc-font");
    d.font.size = 12;
    d.responsive = true;
    d.maintainAspectRatio = false;
    d.resizeDelay = 0;
    // No tweening: a hover, a legend toggle or a tab that reveals a chart answers at once.
    // Eased tooltips and points trailed the pointer, which read as slow and jagged.
    d.animation = false;
    // Chart.js tweens colours through interpolators keyed by type, and a gradient is
    // an object with no interpolator: the animator then throws "this._fn is not a
    // function" on its first tick and the chart never draws. Colours apply at once.
    if (d.animations) d.animations.colors = false;
    if (d.plugins && d.plugins.colors) d.plugins.colors.enabled = false;
    if (d.scale && d.scale.grid) d.scale.grid.color = v("--rc-separator");
    if (d.plugins && d.plugins.tooltip) {
      const tooltip = d.plugins.tooltip;
      tooltip.backgroundColor = v("--rc-text");
      tooltip.titleColor = ink;
      tooltip.bodyColor = ink;
      tooltip.cornerRadius = 8;
      tooltip.padding = 8;
      // The key next to each value is a dot in its series' own colour.
      tooltip.usePointStyle = true;
      tooltip.boxPadding = 3;
    }
    if (d.plugins && d.plugins.legend) {
      d.plugins.legend.labels.usePointStyle = true;
      d.plugins.legend.labels.boxWidth = 8;
      d.plugins.legend.labels.boxHeight = 8;
      // A name in the legend toggles its series; say so with the pointer.
      d.plugins.legend.onHover = (event, item, legend) => { legend.chart.canvas.style.cursor = item ? "pointer" : ""; };
      d.plugins.legend.onLeave = (event, item, legend) => { legend.chart.canvas.style.cursor = ""; };
    }
    // Quiet axes: no axis line or tick marks, faint horizontal rules only.
    if (d.scale) {
      if (d.scale.border) d.scale.border.display = false;
      if (d.scale.grid) d.scale.grid.drawTicks = false;
      if (d.scale.ticks) d.scale.ticks.padding = 8;
    }
    // The category type's defaults may carry no grid object yet, so make one.
    if (d.scales && d.scales.category) d.scales.category.grid = Object.assign(d.scales.category.grid || {}, { display: false });
    d.elements.line.borderWidth = 2.5;
    d.elements.point.radius = 0;
    d.elements.point.hoverRadius = 5;
    d.elements.point.hoverBorderWidth = 2;
    // A line drawn without points is still easy to hover.
    d.elements.point.hitRadius = 10;
    d.elements.bar.borderRadius = 6;
    // Wide columns: a few bars fill their slot instead of standing as thin sticks.
    if (d.datasets && d.datasets.bar) {
      d.datasets.bar.categoryPercentage = 0.86;
      d.datasets.bar.barPercentage = 0.92;
      d.datasets.bar.maxBarThickness = 64;
    }
  };
  // A view's script runs while the frame is still settling, and a chart measured at
  // 0 x 0 stays blank until something resizes it. Ask again on the next frames.
  const refit = (chart) => {
    const tries = chart.$rcFit || 0;
    chart.$rcFit = tries + 1;
    if (tries > 10 || (chart.width > 1 && chart.height > 1)) return;
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => chart.resize());
  };
  // Marks a view asks for in options.plugins.rcMarks: band [low, high] shaded on the y
  // scale, line (a y value) with lineLabel, and end (true, or its text) for a dot and
  // the value at a line's last point. All drawn inside the plot, in the text variables.
  const marks = {
    id: "rcMarks",
    beforeDatasetsDraw(chart, args, o) {
      const y = chart.scales && chart.scales.y;
      const a = chart.chartArea;
      if (!o || !y || !a || !Array.isArray(o.band)) return;
      const top = y.getPixelForValue(o.band[1]);
      const bottom = y.getPixelForValue(o.band[0]);
      chart.ctx.save();
      chart.ctx.globalAlpha = 0.07;
      chart.ctx.fillStyle = v("--rc-text");
      chart.ctx.fillRect(a.left, Math.min(top, bottom), a.right - a.left, Math.abs(bottom - top));
      chart.ctx.restore();
    },
    afterDatasetsDraw(chart, args, o) {
      const y = chart.scales && chart.scales.y;
      const a = chart.chartArea;
      if (!o || !y || !a) return;
      const ctx = chart.ctx;
      ctx.save();
      ctx.font = "500 11px " + v("--rc-font");
      ctx.textAlign = "right";
      ctx.textBaseline = "bottom";
      if (typeof o.line === "number") {
        const at = Math.round(y.getPixelForValue(o.line)) + 0.5;
        ctx.strokeStyle = v("--rc-muted");
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(a.left, at);
        ctx.lineTo(a.right, at);
        ctx.stroke();
        if (o.lineLabel) {
          ctx.fillStyle = v("--rc-muted");
          ctx.fillText(o.lineLabel, a.right - 4, at - 4);
        }
      }
      const meta = o.end && chart.getDatasetMeta && chart.getDatasetMeta(0);
      const last = meta && meta.data && meta.data[meta.data.length - 1];
      if (last) {
        const raw = chart.data.datasets[0].data[meta.data.length - 1];
        const value = raw && typeof raw === "object" ? raw.y : raw;
        const colour = solid(chart.data.datasets[0].borderColor, solid(v("--rc-accent"), NEUTRAL));
        ctx.fillStyle = colour;
        ctx.beginPath();
        ctx.arc(last.x, last.y, 4, 0, Math.PI * 2);
        ctx.fill();
        ctx.font = "600 12px " + v("--rc-font");
        ctx.fillStyle = v("--rc-text");
        const text = typeof o.end === "string" ? o.end : Number(value).toLocaleString();
        ctx.fillText(text, last.x - 8, Math.max(a.top + 14, last.y - 8));
      }
      ctx.restore();
    },
  };
  apply();
  Chart.register({ id: "rcSeries", beforeUpdate: paint, afterRender: refit }, marks);
  // A \`:: chart\` block carries its whole config as data (shorthand.ts): no script of the
  // view's own. They are built once the page has parsed, so a canvas after this script counts.
  const build = () => {
    if (typeof document.querySelectorAll !== "function") return;
    for (const canvas of document.querySelectorAll("canvas[data-rc-chart]")) {
      if (canvas.$rcBuilt) continue;
      canvas.$rcBuilt = true;
      try { new Chart(canvas, JSON.parse(canvas.getAttribute("data-rc-chart") || "{}")); } catch (e) { console.error(e); }
    }
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", build);
  else build();
  window.__rcThemeCharts = () => {
    apply();
    for (const chart of Object.values(Chart.instances || {})) {
      for (const scale of Object.values(chart.options.scales || {})) {
        if (scale.grid) scale.grid.color = v("--rc-separator");
        if (scale.ticks) scale.ticks.color = v("--rc-muted");
      }
      for (const ds of chart.data.datasets) {
        if (!ds.__rc) continue;
        delete ds.backgroundColor;
        delete ds.borderColor;
        delete ds.hoverBackgroundColor;
      }
      chart.update("none");
    }
  };
})();`;

const CHART_TAG = /<script\b[^>]*\bsrc\s*=\s*["']rustcode-lib:chart\.js["'][^>]*>\s*<\/script>/gi;

/** A `:: chart` block, which writes the Chart.js tag when it is expanded. */
const CHART_SHORTHAND = /^\s*::\s*chart\b/m;

/** Whether the block names the bundled Chart.js, itself or through a `:: chart` block. */
export const usesChart = (source: string) => {
  CHART_TAG.lastIndex = 0;
  return CHART_TAG.test(source) || CHART_SHORTHAND.test(source);
};

/**
 * Whether a block is worth drawing while it is still arriving. A preview page
 * runs no script and carries no library, so a canvas in it can never draw: a
 * view whose content is a chart waits for its finished page instead of showing
 * empty boxes where the charts will be.
 */
export const previewable = (source: string) => {
  CHART_TAG.lastIndex = 0;
  return !CHART_TAG.test(source);
};

/** Inline text inside a `<script>` element must not close it. */
const scriptSafe = (code: string) => code.replace(/<\/script/gi, "<\\/script");

/**
 * The page around a body: policy, variables, stylesheet and the runtime, first. The `body`
 * carries the system's text helpers off, inherited by every field the block draws: the frame is
 * its own document, which the app's sweep (lib/quietNative.ts) cannot reach
 * (docs/desktop.md › Native text fields).
 */
function pageAt(body: string, token: string, theme: PageTheme): string {
  return [
    '<!doctype html><html><head><meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${PAGE_CSP}">`,
    '<meta name="viewport" content="width=device-width">',
    `<style id="rc-vars">${varsCss(theme)}</style>`,
    `<style>${BASE_CSS}</style>`,
    `<script>${runtimeScript(token, theme)}</script>`,
    '</head><body spellcheck="false" autocorrect="off" autocapitalize="off" writingsuggestions="false">',
    `<div id="rc-root">${body}</div>`,
    "</body></html>",
  ].join("");
}

/**
 * The page a block is drawn in while its fence is still streaming: the runtime
 * and the stylesheet, and an empty container the app fills with the fragment as
 * it arrives. The block's own script stays inert here; the finished block gets
 * its own page (docs/smart-ui.md, "Streaming").
 */
export function buildStreamPage(token: string, theme: PageTheme): string {
  return pageAt('<div id="rc-stream"></div>', token, theme);
}

/** The page for a block: CSP, theme, base CSS, runtime, then the block with Chart.js inlined once. */
export function buildPage(source: string, token: string, theme: PageTheme, chartJs?: string): string {
  let first = true;
  const body = expandShorthand(source).replace(CHART_TAG, () => {
    if (!first || !chartJs) return "";
    first = false;
    return `<script>${scriptSafe(chartJs)}</script><script>${CHART_THEME_SCRIPT}</script>`;
  });
  return pageAt(stripEmptyTags(body), token, theme);
}

/** The file a saved view is offered as: its title in lowercase words, or `view`. */
export function viewFileName(title: string): string {
  const slug = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/, "");
  return `${slug || "view"}.html`;
}
