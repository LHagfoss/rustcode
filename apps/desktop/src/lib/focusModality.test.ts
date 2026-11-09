import { describe, expect, test } from "bun:test";
import { installFocusModality, KEYBOARD_NAV } from "./focusModality";

function fakeRoot() {
  const attrs = new Map<string, string>();
  return {
    attrs,
    setAttribute: (name: string, value: string) => void attrs.set(name, value),
    removeAttribute: (name: string) => void attrs.delete(name),
  };
}

function key(target: EventTarget, name: string, shiftKey = false) {
  target.dispatchEvent(Object.assign(new Event("keydown"), { key: name, shiftKey }));
}

class BlurTrackingTarget extends EventTarget {
  blurCapture: boolean | undefined;

  override addEventListener(
    type: string,
    callback: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions,
  ) {
    if (type === "blur")
      this.blurCapture = typeof options === "boolean" ? options : (options?.capture ?? false);
    super.addEventListener(type, callback, options);
  }
}

describe("installFocusModality", () => {
  test("Tab turns keyboard navigation on, a pointer press turns it off", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target, () => true);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
    target.dispatchEvent(new Event("pointerdown"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    target.dispatchEvent(new Event("mousedown"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
  });

  test("Shift+Tab turns keyboard navigation on too", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target, () => true);
    key(target, "Tab", true);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
  });

  test("other keys leave the mode as it is", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target, () => true);
    for (const name of ["Enter", "Escape", "ArrowDown", "a"]) key(target, name);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    for (const name of ["Enter", "Escape", "ArrowDown", "a"]) key(target, name);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
  });

  test("window blur clears keyboard navigation", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target, () => true);
    key(target, "Tab");
    target.dispatchEvent(new Event("blur"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
  });

  test("blur handling does not capture descendant focus changes", () => {
    const target = new BlurTrackingTarget();
    installFocusModality(fakeRoot(), target);
    expect(target.blurCapture).toBe(false);
  });

  test("stopping it removes the listeners", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    const stop = installFocusModality(root, target, () => true);
    stop();
    key(target, "Tab");
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    root.setAttribute(KEYBOARD_NAV, "");
    target.dispatchEvent(new Event("blur"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
  });
});

test("Tab is blocked only outside protected contexts when disabled", async () => {
  const { shouldBlockTab } = await import("./focusModality");
  expect(shouldBlockTab("Tab", false, false)).toBe(true);
  expect(shouldBlockTab("Tab", false, true)).toBe(false);
  expect(shouldBlockTab("Tab", true, false)).toBe(false);
  expect(shouldBlockTab("Enter", false, false)).toBe(false);
});
