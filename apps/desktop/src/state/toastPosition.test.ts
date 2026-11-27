import { describe, expect, test } from "bun:test";
import { DEFAULT_TOAST_POSITION, toastFromTop, toastPosition } from "./toastPosition";

describe("toast position", () => {
  test("reads a stored position leniently", () => {
    expect(toastPosition("top-center")).toBe("top-center");
    expect(toastPosition("bottom-left")).toBe("bottom-left");
    expect(toastPosition("middle")).toBe(DEFAULT_TOAST_POSITION);
    expect(toastPosition(undefined)).toBe("bottom-right");
  });

  test("top positions stack downward", () => {
    expect(toastFromTop("top-right")).toBe(true);
    expect(toastFromTop("top-center")).toBe(true);
    expect(toastFromTop("bottom-right")).toBe(false);
  });
});
