// design-lint: allow-file (a dev-only overlay, readable over any theme)
// Dev builds only (main.tsx): a frame-rate overlay, toggled with ⌘⌥F. It shows
// the requestAnimationFrame rate, frame-time p95, late frames and long tasks,
// plus what the native side reports (display refresh rate, Low Power Mode,
// WebKit's 60 fps preference), and logs a line to the console every 5 s while
// shown. Plain DOM, outside React, and it runs nothing while hidden.
// docs/desktop.md › Native frame rate.

import { isTauri } from "../acp/transport";

interface DisplayInfo {
  maxFps: number;
  lowPowerMode: boolean;
  near60fps: boolean | null;
}

const WINDOW = 240; // frames kept for the percentiles
const LONG_TASK_MS = 50;

const quantile = (xs: number[], f: number) => {
  if (!xs.length) return 0;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(f * s.length))]!;
};

export function installPerfOverlay() {
  let box: HTMLDivElement | null = null;
  let raf = 0;
  let beat = 0;
  let frames: number[] = [];
  let longTasks: number[] = [];
  let display: DisplayInfo | null = null;

  const readDisplay = async () => {
    if (!isTauri()) return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      display = await invoke<DisplayInfo>("display_info");
    } catch {
      display = null;
    }
  };

  const summary = () => {
    const now = performance.now();
    const recent = frames.slice(-WINDOW);
    const second = (() => {
      let sum = 0;
      let n = 0;
      for (let i = recent.length - 1; i >= 0 && sum < 1000; i--) {
        sum += recent[i]!;
        n++;
      }
      return sum ? (n * 1000) / sum : 0;
    })();
    const median = quantile(recent, 0.5);
    longTasks = longTasks.filter((t) => now - t < 10_000);
    return {
      fps: Math.round(second),
      p95: quantile(recent, 0.95),
      late: recent.filter((f) => f > median * 1.5).length,
      longTasks: longTasks.length,
    };
  };

  const describeDisplay = () => {
    if (!display) return "";
    const parts = [`display ${display.maxFps || "?"} Hz`];
    if (display.lowPowerMode) parts.push("Low Power Mode: WebKit caps at 30 fps");
    if (display.near60fps) parts.push("WebKit near-60 fps preference ON");
    return parts.join(" · ");
  };

  let lastPaint = 0;
  let lastLog = 0;
  let last = 0;
  const loop = (t: number) => {
    if (last) frames.push(t - last);
    if (frames.length > WINDOW * 2) frames = frames.slice(-WINDOW);
    last = t;
    if (box && t - lastPaint > 500) {
      lastPaint = t;
      const s = summary();
      box.textContent = `${s.fps} fps · p95 ${s.p95.toFixed(1)} ms · late ${s.late}/${Math.min(frames.length, WINDOW)} · long tasks ${s.longTasks}/10s${display ? `\n${describeDisplay()}` : ""}`;
      if (t - lastLog > 5000) {
        lastLog = t;
        console.info("[perf]", { ...s, ...display });
      }
    }
    raf = requestAnimationFrame(loop);
  };

  const show = () => {
    box = document.createElement("div");
    box.setAttribute("aria-hidden", "true");
    Object.assign(box.style, {
      position: "fixed",
      right: "8px",
      bottom: "8px",
      zIndex: "2147483647",
      padding: "4px 8px",
      font: "11px/1.4 ui-monospace, monospace",
      whiteSpace: "pre",
      color: "#fff",
      background: "rgba(0,0,0,0.72)",
      borderRadius: "6px",
      pointerEvents: "none",
      contain: "layout paint style",
    } satisfies Partial<CSSStyleDeclaration>);
    document.body.append(box);
    frames = [];
    longTasks = [];
    last = 0;
    void readDisplay();
    raf = requestAnimationFrame(loop);
    let at = performance.now();
    beat = window.setInterval(() => {
      const now = performance.now();
      if (now - at > LONG_TASK_MS + 10) longTasks.push(now);
      at = now;
    }, 10);
  };

  const hide = () => {
    cancelAnimationFrame(raf);
    clearInterval(beat);
    box?.remove();
    box = null;
  };

  window.addEventListener(
    "keydown",
    (e) => {
      if (e.metaKey && e.altKey && e.code === "KeyF") {
        e.preventDefault();
        e.stopPropagation();
        if (box) hide();
        else show();
      }
    },
    true,
  );
}
