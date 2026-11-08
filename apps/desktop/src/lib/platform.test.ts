import { describe, expect, test } from "bun:test";
import { fileManagerName, platformFor } from "./platform";

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
const LINUX =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

describe("platformFor", () => {
  test("names each web view's OS in the app", () => {
    expect(platformFor(MAC, true)).toBe("mac");
    expect(platformFor(WINDOWS, true)).toBe("windows");
    expect(platformFor(LINUX, true)).toBe("linux");
  });

  test("a browser gets the macOS look whatever it runs on", () => {
    expect(platformFor(WINDOWS, false)).toBe("mac");
    expect(platformFor(LINUX, false)).toBe("mac");
  });

  test("an unknown agent falls back to mac", () => {
    expect(platformFor("", true)).toBe("mac");
  });
});

test("names each platform file manager", () => {
  expect(fileManagerName("mac")).toBe("Finder");
  expect(fileManagerName("windows")).toBe("Explorer");
  expect(fileManagerName("linux")).toBe("file manager");
});
