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
  expect(shouldBlockTab("Tab", true, false)).toBe(false);
  expect(shouldBlockTab("Tab", false, false, true)).toBe(false);
});

test("with the setting off a bare Tab is cancelled and shows no rings; Ctrl+Tab is left alone", () => {
  const root = fakeRoot();
  const target = new EventTarget();
  installFocusModality(root, target, () => false);
  const tab = Object.assign(new Event("keydown", { cancelable: true }), { key: "Tab", shiftKey: true });
  target.dispatchEvent(tab);
  expect(tab.defaultPrevented).toBe(true);
  expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
  const chord = Object.assign(new Event("keydown", { cancelable: true }), { key: "Tab", ctrlKey: true });
  target.dispatchEvent(chord);
  expect(chord.defaultPrevented).toBe(false);
});

test("Tab from fields respects the setting and dialog context", () => {
  const originalElement = globalThis.Element;
  class FakeElement extends EventTarget {
    constructor(private inDialog: boolean) {
      super();
    }
    closest(selector: string) {
      expect(selector).not.toContain("textarea");
      expect(selector).not.toContain("input");
      return this.inDialog ? this : null;
    }
  }
  globalThis.Element = FakeElement as unknown as typeof Element;
  try {
    for (const [inDialog, enabled, blocked] of [
      [false, false, true],
      [true, false, false],
      [false, true, false],
    ] as const) {
      const field = new FakeElement(inDialog);
      installFocusModality(fakeRoot(), field, () => enabled);
      const event = Object.assign(new Event("keydown", { cancelable: true }), { key: "Tab" });
      field.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(blocked);
    }
  } finally {
    globalThis.Element = originalElement;
  }
});
