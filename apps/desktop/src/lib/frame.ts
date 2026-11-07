// A frame the browser has actually painted. A store update draws what it changed in the frame that
// follows it, and any work that runs in the same task as that update shares the frame with it: a
// long task there means the update is painted late or, when the work ends in another update, not at
// all. The send path waits on this once, so the message's own row and its entry are drawn before the
// prompt is built and the session is asked for.
//
// A hidden window (minimized, behind another app, a background tab) paints no frames and never runs
// `requestAnimationFrame`: waiting on one there would hold every send, a wake or a queued message
// included, until the window is shown again. A hidden page does not wait, and a frame that has not
// come within `FRAME_TIMEOUT_MS` is not waited for any longer.

/** The longest a send waits for a frame: a few frames at 60 Hz, never a visible delay. */
export const FRAME_TIMEOUT_MS = 50;

/** Resolves after the next frame is painted, or at once when nothing paints (a hidden page, tests,
 * workers), or after `FRAME_TIMEOUT_MS` when the frame does not come. */
export function afterPaint(): Promise<void> {
  if (typeof requestAnimationFrame !== "function") return Promise.resolve();
  if (typeof document !== "undefined" && document.visibilityState === "hidden") return Promise.resolve();
  return new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(finish, FRAME_TIMEOUT_MS);
    // `requestAnimationFrame` runs before layout and paint, so the zero timer inside it lands after both.
    requestAnimationFrame(() => setTimeout(finish, 0));
  });
}
