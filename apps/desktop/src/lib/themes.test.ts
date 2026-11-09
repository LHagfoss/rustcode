import { describe, expect, test } from "bun:test";
import { BUILTIN_THEMES, cssName, parseThemes, resolveTheme } from "./themes";

describe("themes", () => {
  test("parses the engine snapshot, keeping only valid colours", () => {
    const snap = parseThemes({
      active: "x",
      themes: [
        { name: "x", mode: "light", radius: "round", source: "user", tokens: { bg: "#123456", text: "red" } },
      ],
    });
    const x = snap.themes[0]!;
    expect(x.tokens.bg).toBe("#123456");
    // An invalid value falls back to the built-in theme of that mode.
    expect(x.tokens.text).toBe(BUILTIN_THEMES.find((t) => t.name === "light")!.tokens.text);
    expect(x.radius).toBe("round");
    expect(cssName("surface_2")).toBe("--rc-surface-2");
  });

  test("paints the active theme only in its own mode", () => {
    const snap = parseThemes({ active: "night", themes: [{ name: "night", mode: "dark", tokens: {} }] });
    expect(resolveTheme(snap, "night", "dark").name).toBe("night");
    expect(resolveTheme(snap, "night", "light").name).toBe("light");
    expect(resolveTheme(null, "missing", "dark").name).toBe("rustcode");
  });
});
