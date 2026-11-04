import { describe, expect, test } from "bun:test";
import { highlight, langForPath, peekHighlight } from "./highlight";

describe("highlight cache", () => {
  test("a block is highlighted once; later renders read it back synchronously", async () => {
    const code = "const answer = 42;";
    expect(peekHighlight(code, "ts")).toBeUndefined();
    const html = await highlight(code, "ts");
    expect(html).toContain("answer");
    expect(peekHighlight(code, "ts")).toBe(html);
    expect(await highlight(code, "typescript")).toBe(html);
  });

  test("plain text is never highlighted or cached", async () => {
    expect(peekHighlight("x", "text")).toBeNull();
    expect(await highlight("x", undefined)).toBeNull();
  });
});

describe("langForPath", () => {
  test("reads the extension or the file's name", () => {
    expect(langForPath("src/a.mjs")).toBe("javascript");
    expect(langForPath("db/init.SQL")).toBe("sql");
    expect(langForPath("deploy/Dockerfile")).toBe("dockerfile");
    expect(langForPath("Dockerfile.dev")).toBe("dockerfile");
    expect(langForPath("v1.2/notes")).toBeUndefined();
    expect(langForPath("a.unknown")).toBeUndefined();
  });
});
