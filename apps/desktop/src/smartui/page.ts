// design-lint: allow-file (the sandboxed page's own stylesheet and chart palette, not app chrome)
// The page a Smart UI block runs in: a CSP, the base stylesheet and the
// runtime (`window.rustcode`) before the block, and Chart.js inlined where
// the block names `rustcode-lib:chart.js`. The names here are a contract
// with the engine's `smart-ui` skill (docs/smart-ui.md, "Runtime").

import { KIT_SCRIPT } from "./kit";
import { MAX_HEIGHT, stripEmptyTags } from "./parse";

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
/* The document itself never scrolls, so no scrollbar can appear, narrow the layout and
   re-wrap the text under the pointer. Only a view past the app's 720 px clamp scrolls,
   inside the body, where that scrollbar is the one that lets it be read. */
html { overflow: hidden; }
html, body { margin: 0; background: transparent; }
body { position: relative; max-height: ${MAX_HEIGHT}px; overflow-y: auto; color: var(--rc-text); font: var(--rc-text-size)/1.5 var(--rc-font);
  padding: 1px 0 2px; overflow-wrap: anywhere; -webkit-font-smoothing: antialiased; }
* { box-sizing: border-box; }
:focus { outline: none; }
:root[data-keyboard-nav] :focus-visible { outline: 2px solid color-mix(in srgb, var(--rc-accent) 60%, transparent); outline-offset: 1px; }
/* One vertical rhythm for whatever the view writes, kit block or bare tag: the gaps
   between blocks are the page's, not each class's own. The gap goes between drawn blocks only: a leading <title> or <style> is an element too,
   and a plain \`* + *\` gave the first visible block a 12px margin that collapsed out of the
   root, so the measured height came up 12px short and the view's foot was clipped.
   \`flow-root\` keeps any child margin inside the root, where the measurement sees it. */
#rc-root, #rc-stream { display: flow-root; }
#rc-root > :not(title, style, script, link, meta, template) ~ :not(title, style, script, link, meta, template),
#rc-stream > :not(title, style, script, link, meta, template) ~ :not(title, style, script, link, meta, template) { margin-top: 12px; }
h1, h2, h3, h4 { margin: 0; font-size: var(--rc-text-size); font-weight: 600; line-height: 1.3; }
h1 { font-size: var(--rc-text-size-title); }
p { margin: 0; } p + p { margin-top: 8px; }
small, .rc-muted { color: var(--rc-muted); }
a { color: var(--rc-accent); }
table { border-collapse: collapse; width: 100%; font-size: var(--rc-text-size-sm); }
th { text-align: left; font-weight: 600; color: var(--rc-muted); padding: 5px 8px;
  border-bottom: 1px solid var(--rc-separator); }
td { padding: 5px 8px; border-bottom: 1px solid var(--rc-separator); vertical-align: top; }
tr:last-child td { border-bottom: 0; }
input, select, textarea { font: inherit; color: inherit; background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-sm); padding: 4px 8px; outline: none; }
:root[data-keyboard-nav] :is(input, select, textarea):focus-visible { border-color: var(--rc-accent); outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--rc-accent) 22%, transparent); }
input[type=checkbox], input[type=radio], input[type=range] { accent-color: var(--rc-accent); }
button { font: inherit; font-weight: 500; color: var(--rc-text); background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-pill); padding: 3px 12px; cursor: pointer; }
button:hover { background: color-mix(in srgb, var(--rc-text) 9%, transparent); }
button:active { opacity: 0.75; }
button.primary, button[data-primary] { background: var(--rc-accent); color: var(--rc-accent-text); border-color: transparent; }
button:disabled { opacity: 0.45; cursor: default; }
code, kbd, pre { font-family: var(--rc-mono); font-size: var(--rc-text-size-sm); }
pre { background: var(--rc-surface); padding: 8px 10px; border-radius: var(--rc-radius); overflow: auto; margin: 0; }
hr { border: 0; border-top: 1px solid var(--rc-separator); margin: 0; }
canvas { max-width: 100%; }
.rc-row { display: flex; flex-wrap: wrap; align-items: center; }
.rc-col { display: flex; flex-direction: column; }
.rc-grid, .rc-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); }
.rc-gap-1 { gap: 4px; } .rc-gap-2 { gap: 8px; } .rc-gap-3 { gap: 12px; } .rc-gap-4 { gap: 16px; }
.rc-mono { font-family: var(--rc-mono); }
.rc-num { font-variant-numeric: tabular-nums; }
td.rc-num, th.rc-num, .rc-num td, .rc-num th { text-align: right; }
.rc-card { background: var(--rc-surface); border-radius: var(--rc-radius); padding: 10px 12px; }
.rc-scroll { max-height: 320px; overflow: auto; }

.rc-title { font-size: var(--rc-text-size-title); font-weight: 600; line-height: 1.3; margin: 0; }
.rc-lead { color: var(--rc-muted); margin: 0; }
.rc-label { font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }
.rc-ok { --rc-tint: var(--rc-success); } .rc-warn { --rc-tint: var(--rc-warning); }
.rc-bad { --rc-tint: var(--rc-danger); } .rc-info { --rc-tint: var(--rc-accent); }

.rc-stats { gap: 8px; }
.rc-stat { display: flex; flex-direction: column; min-width: 0; padding: 10px 12px;
  border: 1px solid var(--rc-separator); border-radius: var(--rc-radius); }
.rc-stat-label { order: -1; font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-muted); }
.rc-stat-value { font-size: var(--rc-text-size-value); line-height: 1.25; font-weight: 600; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.rc-stat-note { font-size: var(--rc-text-size-xs); line-height: 1.45; color: var(--rc-tint, var(--rc-muted)); }

.rc-badge { display: inline-block; padding: 0 7px; font-size: var(--rc-text-size-xs); line-height: 18px; font-weight: 500; white-space: nowrap;
  border-radius: var(--rc-radius-pill); color: var(--rc-tint, var(--rc-muted));
  background: color-mix(in srgb, var(--rc-tint, var(--rc-text)) 13%, transparent); }
.rc-callout { padding: 8px 12px; border-radius: var(--rc-radius);
  background: color-mix(in srgb, var(--rc-tint, var(--rc-text)) 10%, transparent); }
.rc-chip { padding: 2px 10px; font-size: var(--rc-text-size-sm); line-height: 18px; font-weight: 400; color: var(--rc-muted); cursor: pointer;
  background: transparent; border: 1px solid var(--rc-separator); border-radius: var(--rc-radius-pill); }
.rc-chip:hover { color: var(--rc-text); }
[data-rc-send], [data-rc-copy] { cursor: pointer; }

.rc-progress { height: 6px; overflow: hidden; border-radius: var(--rc-radius-pill); background: color-mix(in srgb, var(--rc-text) 10%, transparent); }
.rc-progress > span { display: block; height: 100%; border-radius: inherit; background: var(--rc-tint, var(--rc-accent)); }

.rc-bars { display: grid; grid-template-columns: fit-content(40%) minmax(32px, 1fr) max-content; gap: 6px 10px; align-items: center; }
.rc-bar { display: grid; grid-column: 1 / -1; grid-template-columns: subgrid; align-items: center; position: relative;
  font-size: var(--rc-text-size-sm); line-height: 16px; }
.rc-bar::before { content: ""; grid-area: 1 / 2; height: 6px; border-radius: var(--rc-radius-pill); background: color-mix(in srgb, var(--rc-text) 10%, transparent); }
.rc-bar > span { grid-area: 1 / 1; color: var(--rc-muted); }
.rc-bar > i { grid-area: 1 / 2; justify-self: start; min-width: 2px; height: 6px; border-radius: var(--rc-radius-pill); background: var(--rc-tint, var(--rc-accent)); }
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

.rc-table { overflow-wrap: normal; font-variant-numeric: tabular-nums; }
.rc-table .rc-num { white-space: nowrap; }
.rc-table-box { overflow-x: auto; }
.rc-table th { position: sticky; top: 0; z-index: 1; font-size: var(--rc-text-size-sm); font-weight: 500; white-space: nowrap; background: var(--rc-bg);
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
.rc-chart { position: relative; height: 200px; }
.rc-chart[data-size=sm] { height: 140px; }
.rc-chart[data-size=lg] { height: 260px; }
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

.rc-list { display: flex; flex-direction: column; margin: 0; padding: 0; list-style: none; }
.rc-item { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 6px 12px; padding: 6px 0;
  border-bottom: 1px solid var(--rc-separator); }
.rc-item:last-child { border-bottom: 0; }
.rc-item:has(> div, > p) { justify-content: flex-start; }
.rc-item > * { min-width: 0; overflow-wrap: anywhere; }
.rc-item > .rc-badge, .rc-item > .rc-chip { flex: none; }
.rc-item > :last-child:not(:first-child) { font-variant-numeric: tabular-nums; }
.rc-item > div, .rc-item > p { flex: 0 0 100%; margin: 0; }

.rc-steps { margin: 0; padding: 0; list-style: none; }
.rc-step { position: relative; padding: 0 0 12px 20px; }
.rc-step:last-child { padding-bottom: 0; }
.rc-step::before { content: ""; position: absolute; left: 1px; top: 6px; width: 8px; height: 8px; border-radius: 50%;
  background: var(--rc-faint); }
.rc-step::after { content: ""; position: absolute; left: 4.5px; top: 18px; bottom: -2px; width: 1px; background: var(--rc-separator); }
.rc-step:last-child::after { display: none; }
.rc-done::before { background: var(--rc-success); }
.rc-active::before { background: var(--rc-accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--rc-accent) 22%, transparent); }
.rc-failed::before { background: var(--rc-danger); }
.rc-todo { color: var(--rc-muted); }
.rc-todo::before { background: transparent; box-shadow: inset 0 0 0 1.5px var(--rc-faint); }

.rc-details { border-bottom: 1px solid var(--rc-separator); }
.rc-details:last-child { border-bottom: 0; }
.rc-details > summary { display: flex; align-items: center; gap: 8px; padding: 6px 0; font-weight: 500; cursor: pointer; list-style: none; }
.rc-details > summary::-webkit-details-marker { display: none; }
.rc-details > summary::before { content: ""; flex: none; width: 6px; height: 6px; margin: 0 2px;
  border-right: 1.5px solid var(--rc-muted); border-bottom: 1.5px solid var(--rc-muted);
  transform: rotate(-45deg); transition: transform 120ms ease-out; }
.rc-details[open] > summary::before { transform: rotate(45deg); }
.rc-details > summary > :last-child:not(:first-child) { margin-left: auto; font-weight: 400; }
.rc-details > :not(summary) { margin-bottom: 8px; }
.rc-tree .rc-details { border-bottom: 0; }
.rc-tree .rc-details > :not(summary) { margin: 0 0 0 5px; padding-left: 12px; border-left: 1px solid var(--rc-separator); }
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
@media (prefers-reduced-motion: reduce) { .rc-details > summary::before, .rc-bar[data-rc-tip]::after { transition: none; } }
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
      return;
    }
    if (data.type !== "theme") return;
    const style = document.getElementById("rc-vars");
    if (style) style.textContent = String(data.css || "");
    theme = data.mode === "dark" ? "dark" : "light";
    if (window.__rcThemeCharts) window.__rcThemeCharts();
    for (const callback of listeners) { try { callback(theme); } catch (e) { console.error(e); } }
  });
  const setKeyboardNav = (active) => {
    document.documentElement.toggleAttribute("data-keyboard-nav", active);
    post({ type: "focus-modality", keyboard: active });
  };
  window.addEventListener("keydown", (event) => { if (event.key === "Tab") setKeyboardNav(true); }, true);
  const pointer = () => setKeyboardNav(false);
  window.addEventListener("pointerdown", pointer, true);
  window.addEventListener("mousedown", pointer, true);
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
    if (next !== last) { last = next; post({ type: "size", height, elements }); }
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
      panel.hidden = panel.getAttribute("data-rc-panel") !== name;
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
  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
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
    for (const table of document.querySelectorAll("table.rc-table[data-rc-sort]")) {
      for (const heading of (table.tHead ? table.tHead.rows[0].cells : [])) heading.tabIndex = 0;
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
  const calm = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
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
    // A second of animation in a transcript is noise; reduce motion asks for none.
    d.animation = calm() ? false : { duration: 180 };
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
    d.elements.line.borderWidth = 2;
    d.elements.point.radius = 0;
    d.elements.point.hoverRadius = 4;
    d.elements.point.hoverBorderWidth = 2;
    // A line drawn without points is still easy to hover.
    d.elements.point.hitRadius = 10;
    d.elements.bar.borderRadius = 4;
  };
  // A view's script runs while the frame is still settling, and a chart measured at
  // 0 x 0 stays blank until something resizes it. Ask again on the next frames.
  const refit = (chart) => {
    const tries = chart.$rcFit || 0;
    chart.$rcFit = tries + 1;
    if (tries > 10 || (chart.width > 1 && chart.height > 1)) return;
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => chart.resize());
  };
  apply();
  Chart.register({ id: "rcSeries", beforeUpdate: paint, afterRender: refit });
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

/** Whether the block names the bundled Chart.js. */
export const usesChart = (source: string) => {
  CHART_TAG.lastIndex = 0;
  return CHART_TAG.test(source);
};

/**
 * Whether a block is worth drawing while it is still arriving. A preview page
 * runs no script and carries no library, so a canvas in it can never draw: a
 * view whose content is a chart waits for its finished page instead of showing
 * empty boxes where the charts will be.
 */
export const previewable = (source: string) => !usesChart(source);

/** Inline text inside a `<script>` element must not close it. */
const scriptSafe = (code: string) => code.replace(/<\/script/gi, "<\\/script");

/** The page around a body: policy, variables, stylesheet and the runtime, first. */
function pageAt(body: string, token: string, theme: PageTheme): string {
  return [
    '<!doctype html><html><head><meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${PAGE_CSP}">`,
    '<meta name="viewport" content="width=device-width">',
    `<style id="rc-vars">${varsCss(theme)}</style>`,
    `<style>${BASE_CSS}</style>`,
    `<script>${runtimeScript(token, theme)}</script>`,
    "</head><body>",
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
  const body = source.replace(CHART_TAG, () => {
    if (!first || !chartJs) return "";
    first = false;
    return `<script>${scriptSafe(chartJs)}</script><script>${CHART_THEME_SCRIPT}</script>`;
  });
  return pageAt(stripEmptyTags(body), token, theme);
}
