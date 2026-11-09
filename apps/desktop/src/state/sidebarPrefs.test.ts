// The sidebar's stored preferences: the row layout's migration off the two values earlier releases
// wrote, so a user who chose one keeps it and everyone else gets the roomy default.

import { describe, expect, test } from "bun:test";
import { DEFAULT_ROW_LAYOUT, rowLayoutOf } from "./sidebarPrefs";

describe("rowLayoutOf", () => {
  test("keeps the two current values", () => {
    expect(rowLayoutOf("compact")).toBe("compact");
    expect(rowLayoutOf("comfortable")).toBe("comfortable");
  });

  test("migrates both retired two-line values to comfortable", () => {
    expect(rowLayoutOf("current")).toBe("comfortable");
    expect(rowLayoutOf("title-first")).toBe("comfortable");
  });

  test("a store with no value, or one it cannot read, takes the roomy default", () => {
    expect(DEFAULT_ROW_LAYOUT).toBe("comfortable");
    expect(rowLayoutOf(undefined)).toBe("comfortable");
    expect(rowLayoutOf(null)).toBe("comfortable");
    expect(rowLayoutOf(42)).toBe("comfortable");
  });
});
