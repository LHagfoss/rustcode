import { describe, expect, test } from "bun:test";
import { materialFor } from "./material";

describe("materialFor", () => {
  test("only the macOS app window has a material", () => {
    expect(materialFor("mac", true)).toBe("vibrancy");
    // The mock and dev:web are `mac` too, with no window behind the page.
    expect(materialFor("mac", false)).toBe("none");
    expect(materialFor("windows", true)).toBe("none");
    expect(materialFor("linux", true)).toBe("none");
  });
});
