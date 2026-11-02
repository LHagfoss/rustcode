// design-lint: allow-file (the sandboxed page's own stylesheet and chart palette, not app chrome)
// The page a Smart UI block runs in: a CSP, the base stylesheet and the
// runtime (`window.rustcode`) before the block, and Chart.js inlined where
// the block names `rustcode-lib:chart.js`. The names here are a contract
// with the engine's `smart-ui` skill (docs/smart-ui.md, "Runtime").

export const PAGE_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; " +
  "connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

export type ThemeMode = "light" | "dark";

/** The eight series colours, Apple's system palette in each mode. */
const SERIES: Record<ThemeMode, string[]> = {
  light: ["#007aff", "#ff9500", "#34c759", "#ff3b30", "#af52de", "#30b0c7", "#ffcc00", "#ff2d55"],
  dark: ["#0a84ff", "#ff9f0a", "#30d158", "#ff453a", "#bf5af2", "#40c8e0", "#ffd60a", "#ff375f"],
};

export interface PageTheme {
  mode: ThemeMode;
  vars: Record<string, string>;
}

/** The page's variables from the app's tokens (`index.css`), read through `token`. */
export function pageTheme(mode: ThemeMode, token: (name: string) => string): PageTheme {
  const vars: Record<string, string> = {
    "--rc-text": token("--rc-text"),
    "--rc-muted": token("--rc-text-muted"),
    "--rc-faint": token("--rc-text-faint"),
    "--rc-accent": token("--rc-accent"),
    "--rc-surface": token("--rc-surface"),
    "--rc-separator": token("--rc-border"),
    "--rc-danger": token("--rc-danger"),
    "--rc-success": token("--rc-success"),
    "--rc-warning": token("--rc-warning"),
    "--rc-font": '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", system-ui, sans-serif',
    "--rc-mono": '"SF Mono", ui-monospace, Menlo, monospace',
    "--rc-radius": "10px",
  };
  SERIES[mode].forEach((c, i) => {
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
html, body { margin: 0; background: transparent; }
body { color: var(--rc-text); font: 13px/20px var(--rc-font); padding: 1px 0 2px; overflow-wrap: anywhere;
  -webkit-font-smoothing: antialiased; }
* { box-sizing: border-box; }
h1, h2, h3, h4 { margin: 0 0 6px; font-weight: 600; line-height: 1.3; }
h1 { font-size: 17px; } h2 { font-size: 15px; } h3, h4 { font-size: 13px; }
p { margin: 0 0 8px; } p:last-child { margin-bottom: 0; }
small, .rc-muted { color: var(--rc-muted); }
a { color: var(--rc-accent); }
table { border-collapse: collapse; width: 100%; font-size: 12px; }
th { text-align: left; font-weight: 600; color: var(--rc-muted); padding: 5px 8px;
  border-bottom: 1px solid var(--rc-separator); }
td { padding: 5px 8px; border-bottom: 1px solid var(--rc-separator); vertical-align: top; }
tr:last-child td { border-bottom: 0; }
input, select, textarea { font: inherit; color: inherit; background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: 8px; padding: 4px 8px; outline: none; }
input:focus, select:focus, textarea:focus { border-color: var(--rc-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--rc-accent) 22%, transparent); }
input[type=checkbox], input[type=radio], input[type=range] { accent-color: var(--rc-accent); }
button { font: inherit; font-weight: 500; color: var(--rc-text); background: var(--rc-surface);
  border: 1px solid var(--rc-separator); border-radius: 999px; padding: 3px 12px; cursor: pointer; }
button:hover { background: color-mix(in srgb, var(--rc-text) 9%, transparent); }
button:active { opacity: 0.75; }
button:focus-visible { outline: 2px solid color-mix(in srgb, var(--rc-accent) 60%, transparent); outline-offset: 1px; }
button.primary, button[data-primary] { background: var(--rc-accent); color: #fff; border-color: transparent; }
button:disabled { opacity: 0.45; cursor: default; }
code, kbd, pre { font-family: var(--rc-mono); font-size: 12px; }
pre { background: var(--rc-surface); padding: 8px 10px; border-radius: var(--rc-radius); overflow: auto; margin: 0 0 8px; }
hr { border: 0; border-top: 1px solid var(--rc-separator); margin: 10px 0; }
canvas { max-width: 100%; }
.rc-row { display: flex; flex-wrap: wrap; align-items: center; }
.rc-col { display: flex; flex-direction: column; }
.rc-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); }
.rc-gap-1 { gap: 4px; } .rc-gap-2 { gap: 8px; } .rc-gap-3 { gap: 12px; } .rc-gap-4 { gap: 16px; }
.rc-mono { font-family: var(--rc-mono); }
.rc-num { font-variant-numeric: tabular-nums; }
td.rc-num, th.rc-num, .rc-num td, .rc-num th { text-align: right; }
.rc-card { background: var(--rc-surface); border-radius: var(--rc-radius); padding: 10px 12px; }
.rc-scroll { max-height: 320px; overflow: auto; }
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
  const rustcode = {
    send(text) {
      if (!(navigator.userActivation ? navigator.userActivation.isActive : true)) return false;
      text = String(text == null ? "" : text).slice(0, 2000);
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
    if (event.source !== parent || !data || data.rc !== token || data.type !== "theme") return;
    const style = document.getElementById("rc-vars");
    if (style) style.textContent = String(data.css || "");
    theme = data.mode === "dark" ? "dark" : "light";
    if (window.__rcThemeCharts) window.__rcThemeCharts();
    for (const callback of listeners) { try { callback(theme); } catch (e) { console.error(e); } }
  });
  let last = "";
  const report = () => {
    const root = document.documentElement;
    const height = Math.max(root.scrollHeight, document.body ? document.body.scrollHeight : 0);
    const elements = document.getElementsByTagName("*").length;
    const next = height + ":" + elements;
    if (next !== last) { last = next; post({ type: "size", height, elements }); }
  };
  // A heartbeat, so the app can tell a busy loop from a quiet page.
  setInterval(() => { post({ type: "alive" }); report(); }, 500);
  window.addEventListener("load", () => { report(); new ResizeObserver(report).observe(document.documentElement); });
  window.addEventListener("error", (e) => post({ type: "error", message: String(e.message || "Script error") }));
  window.addEventListener("unhandledrejection", (e) => post({ type: "error", message: String(e.reason) }));
})();`;
}

/** After Chart.js: defaults and series colours from the variables, again on every theme change. */
export const CHART_THEME_SCRIPT = `(() => {
  if (!window.Chart) return;
  const v = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
  const series = () => [1, 2, 3, 4, 5, 6, 7, 8].map((i) => v("--rc-series-" + i));
  const alpha = (c, a) => "color-mix(in srgb, " + c + " " + Math.round(a * 100) + "%, transparent)";
  const radial = (t) => t === "pie" || t === "doughnut" || t === "polarArea";
  const paint = (chart) => {
    const s = series();
    const type = chart.config.type;
    chart.data.datasets.forEach((ds, i) => {
      if (ds.__rc === undefined) ds.__rc = ds.backgroundColor === undefined && ds.borderColor === undefined;
      if (!ds.__rc) return;
      const kind = ds.type || type;
      if (radial(kind)) {
        ds.backgroundColor = (chart.data.labels || ds.data).map((_, j) => s[j % 8]);
        ds.borderColor = "transparent";
      } else {
        ds.borderColor = s[i % 8];
        ds.backgroundColor = kind === "line" || kind === "radar" ? alpha(s[i % 8], 0.18) : s[i % 8];
      }
    });
  };
  const apply = () => {
    const d = Chart.defaults;
    d.color = v("--rc-muted");
    d.borderColor = v("--rc-separator");
    d.font.family = v("--rc-font");
    d.font.size = 12;
    d.maintainAspectRatio = false;
    if (d.plugins && d.plugins.colors) d.plugins.colors.enabled = false;
    if (d.scale && d.scale.grid) d.scale.grid.color = v("--rc-separator");
    if (d.plugins && d.plugins.tooltip) {
      const ink = getComputedStyle(document.documentElement).colorScheme === "dark" ? "#000" : "#fff";
      d.plugins.tooltip.backgroundColor = v("--rc-text");
      d.plugins.tooltip.titleColor = ink;
      d.plugins.tooltip.bodyColor = ink;
      d.plugins.tooltip.cornerRadius = 8;
    }
    if (d.plugins && d.plugins.legend) { d.plugins.legend.labels.boxWidth = 10; d.plugins.legend.labels.boxHeight = 10; }
    d.elements.line.borderWidth = 2;
    d.elements.point.radius = 0;
    d.elements.point.hoverRadius = 4;
    d.elements.bar.borderRadius = 4;
  };
  apply();
  Chart.register({ id: "rcSeries", beforeUpdate: paint });
  window.__rcThemeCharts = () => {
    apply();
    for (const chart of Object.values(Chart.instances || {})) {
      for (const scale of Object.values(chart.options.scales || {})) {
        if (scale.grid) scale.grid.color = v("--rc-separator");
        if (scale.ticks) scale.ticks.color = v("--rc-muted");
      }
      chart.data.datasets.forEach((ds) => { if (ds.__rc) { delete ds.backgroundColor; delete ds.borderColor; } });
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

/** Inline text inside a `<script>` element must not close it. */
const scriptSafe = (code: string) => code.replace(/<\/script/gi, "<\\/script");

/** The page for a block: CSP, theme, base CSS, runtime, then the block with Chart.js inlined once. */
export function buildPage(source: string, token: string, theme: PageTheme, chartJs?: string): string {
  let first = true;
  const body = source.replace(CHART_TAG, () => {
    if (!first || !chartJs) return "";
    first = false;
    return `<script>${scriptSafe(chartJs)}</script><script>${CHART_THEME_SCRIPT}</script>`;
  });
  return [
    '<!doctype html><html><head><meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${PAGE_CSP}">`,
    '<meta name="viewport" content="width=device-width">',
    `<style id="rc-vars">${varsCss(theme)}</style>`,
    `<style>${BASE_CSS}</style>`,
    `<script>${runtimeScript(token, theme)}</script>`,
    "</head><body>",
    body,
    "</body></html>",
  ].join("");
}
