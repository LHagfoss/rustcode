import { afterEach, describe, expect, test } from "bun:test";
import { afterPaint, FRAME_TIMEOUT_MS } from "./frame";

const g = globalThis as Record<string, unknown>;
const saved = { raf: g.requestAnimationFrame, doc: g.document };

afterEach(() => {
  g.requestAnimationFrame = saved.raf;
  g.document = saved.doc;
});

describe("afterPaint", () => {
  test("a hidden page does not wait for a frame it will never paint", async () => {
    g.requestAnimationFrame = () => 0; // never calls back, like a minimized window
    g.document = { visibilityState: "hidden" };
    const start = performance.now();
    await afterPaint();
    expect(performance.now() - start).toBeLessThan(FRAME_TIMEOUT_MS);
  });

  test("a frame that never comes is not waited for past the timeout", async () => {
    g.requestAnimationFrame = () => 0;
    g.document = { visibilityState: "visible" };
    const start = performance.now();
    await afterPaint();
    const waited = performance.now() - start;
    expect(waited).toBeGreaterThanOrEqual(FRAME_TIMEOUT_MS - 5);
    expect(waited).toBeLessThan(FRAME_TIMEOUT_MS + 200);
  });

  test("a painted frame resolves it", async () => {
    g.requestAnimationFrame = (cb: () => void) => {
      setTimeout(cb, 1);
      return 1;
    };
    g.document = { visibilityState: "visible" };
    const start = performance.now();
    await afterPaint();
    expect(performance.now() - start).toBeLessThan(FRAME_TIMEOUT_MS);
  });
});
