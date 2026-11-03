/**
 * Close every open floating layer (popover, menu, dialog, context menu) the way a click
 * somewhere else does. A view change driven by a shortcut — `⌘,` opening Settings, `⌘N` starting
 * a thread — has no click for Radix to see, so a portalled overlay would otherwise stay open
 * over the new view.
 *
 * A pointerdown alone is not enough: popovers and dialogs defer the dismiss until the pointerdown's
 * click lands (so a drag that starts outside does not close them), which is why both go out here.
 * The click lands on `body`, so nothing under the pointer is activated.
 */
export function dismissOverlays() {
  if (typeof document === "undefined") return;
  const pointer = { bubbles: true, cancelable: true, isPrimary: true, button: 0, pointerId: 1 };
  document.body.dispatchEvent(new PointerEvent("pointerdown", { ...pointer, pointerType: "mouse" }));
  document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
}
