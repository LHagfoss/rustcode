// The scenario scripts/perf-native.swift runs in the page once it has loaded
// (`?transport=perf`): an async function returning the numbers per phase.
async () => {
  const P = window.__pn;
  const post = (kind, name) => window.webkit.messageHandlers.perf.postMessage({ kind, name });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const frame = () => new Promise((r) => requestAnimationFrame(r));
  const until = async (test, ms = 60000) => {
    const t0 = performance.now();
    while (performance.now() - t0 < ms) {
      try {
        if (test()) return true;
      } catch {}
      await frame();
    }
    return false;
  };
  const q = (xs, f) => {
    const s = [...xs].sort((a, b) => a - b);
    return s.length ? s[Math.min(s.length - 1, Math.floor(f * s.length))] : 0;
  };
  const r1 = (x) => Math.round(x * 10) / 10;
  const app = () => window.__rustcode;
  const results = {};
  // The display's frame interval, from a quiet second (16.7 at 60 Hz, 8.3 at 120, 33 under Low Power Mode).
  let budget = 16.7;
  async function phase(name, run) {
    post("log", `phase ${name}`);
    P.reset();
    post("start", name);
    const t0 = performance.now();
    await run();
    const seconds = (performance.now() - t0) / 1000;
    post("end", name);
    const frames = P.frames;
    const late = frames.filter((f) => f > budget * 1.5).length;
    results[name] = {
      fps: r1(frames.length / seconds),
      frameP50Ms: r1(q(frames, 0.5)),
      frameP95Ms: r1(q(frames, 0.95)),
      frameMaxMs: r1(q(frames, 1)),
      lateFrames: late,
      workP50Ms: r1(q(P.work, 0.5)),
      workP95Ms: r1(q(P.work, 0.95)),
      longTasks: P.gaps.length,
      longestTaskMs: r1(q(P.gaps, 1)),
    };
  }

  await until(() => app()?.useApp.getState().sessionsLoaded);
  await sleep(1500);
  await phase("idle", () => sleep(3000));
  budget = Math.max(8, q(P.frames, 0.5));
  results.budgetMs = r1(budget);

  // Open the long thread, then scroll it by script, one step a frame: a proxy for a
  // wheel or trackpad, which WebKit scrolls off the main thread and cannot be synthesized here.
  app().selectThread("perf-long");
  await until(() => {
    const t = app().useApp.getState().threads["perf-long"];
    return t?.loaded && document.querySelectorAll("[role=log] [data-index]").length > 3;
  });
  await sleep(1500);
  const log = document.querySelector("[role=log]");
  await phase("scroll", async () => {
    const t0 = performance.now();
    while (performance.now() - t0 < 4000) {
      log.scrollTop += performance.now() - t0 < 2000 ? -24 : 24;
      await frame();
    }
  });

  // Stream into a thread while the composer has focus.
  app().selectThread("perf-stream");
  await until(() => app().useApp.getState().threads["perf-stream"]?.loaded);
  await sleep(800);
  document.querySelector('textarea[aria-label="Message"]')?.focus();
  await phase("stream", async () => {
    void app().runPrompt("perf-stream", "stream");
    await sleep(5000);
  });
  await until(() => !app().useApp.getState().threads["perf-stream"].running, 120000);
  await sleep(500);

  // Popovers and sheets: open and close, and what the frames do meanwhile.
  await phase("switcher", async () => {
    for (let i = 0; i < 3; i++) {
      app().setSwitcher(true);
      await sleep(500);
      app().setSwitcher(false);
      await sleep(400);
    }
  });
  await phase("settings", async () => {
    for (let i = 0; i < 2; i++) {
      app().openSettings("general");
      await sleep(700);
      app().openSettings(null);
      await sleep(500);
    }
  });

  // Collapse and reopen the sidebar and the right panel over the long thread: both change
  // the chat column's width, which every transcript row lays out again for.
  app().selectThread("perf-long");
  await until(() => document.querySelectorAll("[role=log] [data-index]").length > 3);
  await sleep(1200);
  await phase("sidebar", async () => {
    for (let i = 0; i < 4; i++) {
      app().toggleSidebar();
      await sleep(450);
    }
  });
  await phase("panel", async () => {
    for (let i = 0; i < 4; i++) {
      app().toggleRightPanel();
      await sleep(450);
    }
  });
  // The last phase's CPU numbers arrive with its end message.
  await sleep(100);
  return results;
};
