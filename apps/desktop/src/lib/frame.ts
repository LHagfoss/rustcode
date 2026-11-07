// A frame the browser has actually painted. A store update draws what it changed in the frame that
// follows it, and any work that runs in the same task as that update shares the frame with it: a
// long task there means the update is painted late or, when the work ends in another update, not at
// all. The send path waits on this once, so the message's own row and its entry are drawn before the
// prompt is built and the session is asked for.

/** Resolves after the next frame is painted. `requestAnimationFrame` runs before layout and paint, so
 * the zero timer inside it lands after both. No frames to wait for (tests, workers): resolves at once. */
export function afterPaint(): Promise<void> {
  if (typeof requestAnimationFrame !== "function") return Promise.resolve();
  return new Promise((resolve) => {
    requestAnimationFrame(() => setTimeout(resolve, 0));
  });
}
