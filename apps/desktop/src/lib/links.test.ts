import { describe, expect, test } from "bun:test";
import { sanitizeMeta } from "./links";

describe("link metadata", () => {
  test("keeps only safe values", () => {
    expect(
      sanitizeMeta({
        title: "  A\n title ",
        icon: "https://evil.example/x.png",
        color: "red; background: url(x)",
      }),
    ).toEqual({ title: "A title", siteName: undefined, icon: undefined, color: undefined });
    expect(sanitizeMeta({ icon: "data:image/png;base64,AAAA", color: "#1e2327" })?.color).toBe("#1e2327");
    expect(sanitizeMeta(null)).toBeNull();
    expect(sanitizeMeta({})).toBeNull();
  });
});
