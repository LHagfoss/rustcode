import { describe, expect, test } from "bun:test";
import { MOCK_KIT } from "../acp/mock";
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
} from "./host";
import { BASE_CSS, buildPage, PAGE_CSP, pageTheme, runtimeScript, usesChart } from "./page";
import { blockProblem, blockTitle, clampHeight, MAX_LIVE, splitReply, viewMessage } from "./parse";

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
    ]) {
      expect(script).toContain(part);
    }
    // The delegated send goes through `rustcode.send`, which checks the user's gesture.
    expect(script).toContain('rustcode.send(send.getAttribute("data-rc-send"))');
    expect(script.match(/post\(\{ type: "send"/g)).toHaveLength(1);
  });

  test("the mock's kit view fits a block", () => {
    const [part] = splitReply(MOCK_KIT.join("")).filter((p) => p.kind === "ui");
    expect(part?.complete).toBe(true);
    expect(blockProblem(part?.source ?? "")).toBeNull();
    expect(new TextEncoder().encode(part?.source).length).toBeLessThan(64 * 1024);
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
