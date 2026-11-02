// A ResizeObserver whose callback runs on the next animation frame, once per
// frame. Writing state (and so layout) inside the observer's own callback is
// what raises "ResizeObserver loop completed with undelivered notifications":
// the write resizes observed elements again within the same delivery.

export function observeSize(el: Element, onResize: () => void): () => void {
  let frame = 0;
  const observer = new ResizeObserver(() => {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      onResize();
    });
  });
  observer.observe(el);
  return () => {
    if (frame) cancelAnimationFrame(frame);
    observer.disconnect();
  };
}
