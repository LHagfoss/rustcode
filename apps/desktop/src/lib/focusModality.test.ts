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
    hasAttribute() {
      return false;
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

describe("Tab in surfaces that are dialogs only for assistive tech", () => {
  const originalElement = globalThis.Element;
  // A minimal tree: `closest` walks up to the first ancestor whose tag the selector names.
  class FakeElement extends EventTarget {
    constructor(
      readonly tag: string,
      readonly parent: FakeElement | null = null,
      readonly attrs: string[] = [],
    ) {
      super();
    }
    hasAttribute(name: string) {
      return this.attrs.includes(name);
    }
    closest(selector: string): FakeElement | null {
      const tags = selector.split(",").map((part) => part.trim());
      for (let node: FakeElement | null = this; node; node = node.parent)
        if (tags.includes(node.tag)) return node;
      return null;
    }
  }

  function tabIn(field: FakeElement, enabled: boolean) {
    const target = new EventTarget();
    const root = fakeRoot();
    installFocusModality(root, target, () => enabled);
    const event = Object.assign(new Event("keydown", { cancelable: true }), { key: "Tab" });
    Object.defineProperty(event, "target", { value: field });
    target.dispatchEvent(event);
    return { blocked: event.defaultPrevented, keyboard: root.attrs.has(KEYBOARD_NAV) };
  }

  test("a tab-free dialog (Settings) honors the setting, in both states", () => {
    globalThis.Element = FakeElement as unknown as typeof Element;
    try {
      const settings = new FakeElement('[role="dialog"]', null, ["data-tab-free"]);
      const button = new FakeElement("button", settings);
      expect(tabIn(button, false)).toEqual({ blocked: true, keyboard: false });
      expect(tabIn(button, true)).toEqual({ blocked: false, keyboard: true });
    } finally {
      globalThis.Element = originalElement;
    }
  });

  test("a form inside a tab-free dialog still lets Tab through", () => {
    globalThis.Element = FakeElement as unknown as typeof Element;
    try {
      const settings = new FakeElement('[role="dialog"]', null, ["data-tab-free"]);
      const form = new FakeElement("form", settings);
      expect(tabIn(new FakeElement("input", form), false).blocked).toBe(false);
    } finally {
      globalThis.Element = originalElement;
    }
  });

  test("an ordinary dialog and the plain app keep their behavior", () => {
    globalThis.Element = FakeElement as unknown as typeof Element;
    try {
      const alert = new FakeElement('[role="alertdialog"]');
      expect(tabIn(new FakeElement("button", alert), false).blocked).toBe(false);
      expect(tabIn(new FakeElement("button"), false).blocked).toBe(true);
      expect(tabIn(new FakeElement("button"), true).blocked).toBe(false);
    } finally {
      globalThis.Element = originalElement;
    }
  });
});

test("the setting persists and announces itself so frames re-read it", async () => {
  const store = new Map<string, string>();
  const originalStorage = globalThis.localStorage;
  const originalWindow = (globalThis as { window?: unknown }).window;
  const target = new EventTarget();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    },
  });
  (globalThis as { window?: unknown }).window = target;
  try {
    const { setTabMovesFocus, tabMovesFocus, TAB_FOCUS_EVENT } = await import("./focusModality");
    let announced = 0;
    target.addEventListener(TAB_FOCUS_EVENT, () => announced++);
    expect(tabMovesFocus()).toBe(false);
    setTabMovesFocus(true);
    expect(tabMovesFocus()).toBe(true);
    setTabMovesFocus(false);
    expect(tabMovesFocus()).toBe(false);
    expect(announced).toBe(2);
  } finally {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, value: originalStorage });
    (globalThis as { window?: unknown }).window = originalWindow;
  }
});
