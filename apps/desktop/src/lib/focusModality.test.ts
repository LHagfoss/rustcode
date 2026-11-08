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

function key(target: EventTarget, name: string) {
  target.dispatchEvent(Object.assign(new Event("keydown"), { key: name }));
}

describe("installFocusModality", () => {
  test("Tab turns keyboard navigation on, a pointer press turns it off", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
    target.dispatchEvent(new Event("pointerdown"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    target.dispatchEvent(new Event("mousedown"));
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
  });

  test("other keys leave the mode as it is", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target);
    for (const name of ["Enter", "Escape", "ArrowDown", "a"]) key(target, name);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
    key(target, "Tab");
    for (const name of ["Enter", "Escape", "ArrowDown", "a"]) key(target, name);
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(true);
  });

  test("stopping it removes the listeners", () => {
    const root = fakeRoot();
    const target = new EventTarget();
    installFocusModality(root, target)();
    key(target, "Tab");
    expect(root.attrs.has(KEYBOARD_NAV)).toBe(false);
  });
});
