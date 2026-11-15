import { describe, expect, test } from "bun:test";
import { PREF, readPref, SIDEBAR_DENSITIES, sidebarDensity, writePref } from "./sidebarPrefs";

describe("the sidebar's row density", () => {
  test("is detailed unless compact was chosen", () => {
    expect(sidebarDensity(undefined)).toBe("detailed");
    expect(sidebarDensity("roomy")).toBe("detailed");
    expect(sidebarDensity(3)).toBe("detailed");
    expect(sidebarDensity("compact")).toBe("compact");
  });

  test("offers both densities, detailed first", () => {
    expect(SIDEBAR_DENSITIES.map((d) => d.id)).toEqual(["detailed", "compact"]);
  });

  test("is kept with the other sidebar preferences", () => {
    writePref(PREF.density, "compact");
    expect(sidebarDensity(readPref(PREF.density, "detailed"))).toBe("compact");
    writePref(PREF.density, "detailed");
  });
});
