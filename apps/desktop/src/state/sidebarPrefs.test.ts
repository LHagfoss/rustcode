import { describe, expect, test } from "bun:test";
import { PREF, readPref, SIDEBAR_DENSITIES, sidebarDensity, writePref } from "./sidebarPrefs";

describe("the sidebar's row density", () => {
  test("is compact unless detailed was chosen", () => {
    expect(sidebarDensity(undefined)).toBe("compact");
    expect(sidebarDensity("roomy")).toBe("compact");
    expect(sidebarDensity(3)).toBe("compact");
    expect(sidebarDensity("detailed")).toBe("detailed");
  });

  test("offers both densities, compact first", () => {
    expect(SIDEBAR_DENSITIES.map((d) => d.id)).toEqual(["compact", "detailed"]);
  });

  test("is kept with the other sidebar preferences", () => {
    writePref(PREF.density, "compact");
    expect(sidebarDensity(readPref(PREF.density, "detailed"))).toBe("compact");
    writePref(PREF.density, "detailed");
  });
});
