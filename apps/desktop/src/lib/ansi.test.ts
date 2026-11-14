import { describe, expect, test } from "bun:test";
import { ansiClass, ansiPlainText, applySgr, hasTerminalCodes, nearestColor, parseAnsi } from "./ansi";
import { toolBody } from "./toolBody";

const E = "\u001b";

describe("parseAnsi", () => {
  test("colours coloured test output and resets", () => {
    const lines = parseAnsi(`${E}[32m✓${E}[39m adds ${E}[2m(3 ms)${E}[22m\n${E}[1;31m✗ fails${E}[0m`);
    expect(lines).toEqual([
      [
        { text: "✓", style: { color: "success" } },
        { text: " adds ", style: {} },
        { text: "(3 ms)", style: { dim: true } },
      ],
      [{ text: "✗ fails", style: { bold: true, color: "danger" } }],
    ]);
  });

  test("nests codes and turns each one off on its own", () => {
    const [line] = parseAnsi(`${E}[1m${E}[4mab${E}[31mc${E}[24md${E}[22me${E}[me`);
    expect(line).toEqual([
      { text: "ab", style: { bold: true, underline: true } },
      { text: "c", style: { bold: true, underline: true, color: "danger" } },
      { text: "d", style: { bold: true, color: "danger" } },
      { text: "e", style: { color: "danger" } },
      { text: "e", style: {} },
    ]);
  });

  test("a style carries over line breaks until it is reset", () => {
    const lines = parseAnsi(`${E}[33mone\ntwo${E}[0m\nthree`);
    expect(lines.map((l) => l.map((s) => s.style.color ?? null))).toEqual([["warning"], ["warning"], [null]]);
  });

  test("a progress bar written over with \\r keeps what came last", () => {
    const lines = parseAnsi(`[#   ] 25%\r[##  ] 50%\r[####] 100%\r\ndone\r\n`);
    expect(ansiPlainText(lines)).toBe("[####] 100%\ndone");
  });

  test("drops cursor moves, erases and OSC titles and links", () => {
    const text = `${E}]0;title${E}\\${E}[2K${E}[1Ga ${E}]8;;https://example.com${E}\\link${E}]8;;${E}\\ b${E}]0;t2\u0007`;
    expect(ansiPlainText(parseAnsi(text))).toBe("a link b");
  });

  test("malformed and truncated sequences are dropped, not shown", () => {
    expect(ansiPlainText(parseAnsi(`a${E}b${E}[31`))).toBe("a");
    expect(ansiPlainText(parseAnsi(`x${E}`))).toBe("x");
    expect(ansiPlainText(parseAnsi(`${E}[999;;mok`))).toBe("ok");
    expect(ansiPlainText(parseAnsi(`${E}]8;;never closed`))).toBe("");
  });

  test("keeps empty lines and drops only the final line break", () => {
    expect(parseAnsi("a\n\nb\n").length).toBe(3);
    expect(ansiPlainText(parseAnsi("a\n\nb\n"))).toBe("a\n\nb");
  });

  test("a very long output stays fast", () => {
    const line = `${E}[32mPASS${E}[0m src/file.test.ts ${E}[2m(12 ms)${E}[22m\n`;
    const text = line.repeat(50_000);
    const started = performance.now();
    const lines = parseAnsi(text);
    expect(lines.length).toBe(50_000);
    expect(performance.now() - started).toBeLessThan(1000);
  });
});

describe("colours", () => {
  test("maps 256 and truecolor to the nearest token, or plain", () => {
    expect(applySgr({}, "38;5;196").color).toBe("danger");
    expect(applySgr({}, "38;5;46").color).toBe("success");
    expect(applySgr({}, "38;5;9").color).toBe("danger");
    expect(applySgr({}, "38;2;255;200;0").color).toBe("warning");
    expect(applySgr({}, "38;2;40;120;255").color).toBe("accent");
    expect(applySgr({}, "38;5;240").color).toBe("muted");
    expect(applySgr({ color: "danger" }, "38;2;250;250;250").color).toBeUndefined();
    expect(applySgr({}, "35").color).toBeUndefined();
  });

  test("a background colour is skipped without eating the next code", () => {
    expect(applySgr({}, "48;5;196;31")).toEqual({ color: "danger" });
    expect(applySgr({}, "41;1")).toEqual({ bold: true });
  });

  test("nearestColor reads greys by lightness", () => {
    expect(nearestColor(30, 30, 30)).toBe("muted");
    expect(nearestColor(220, 220, 220)).toBeUndefined();
  });

  test("classes are theme tokens", () => {
    expect(ansiClass({ color: "danger", bold: true })).toBe("text-danger font-semibold");
    expect(ansiClass({ dim: true })).toBe("text-fg-muted");
    expect(ansiClass({ color: "accent", dim: true, italic: true, underline: true })).toBe(
      "text-accent opacity-70 italic underline",
    );
    expect(ansiClass({})).toBe("");
  });
});

describe("a command's output", () => {
  const call = (output: string) => toolBody({ input: { command: "bun test" }, output, diffs: [] }, "execute");

  test("keeps its colours and copies plain text", () => {
    const body = call(`${E}[31mfail${E}[0m\nok\n`);
    expect(body.kind === "command" && body.output).toBe("fail\nok");
    expect(body.kind === "command" && body.styled?.[0]?.[0]?.style.color).toBe("danger");
  });

  test("with only non-colour codes is plain text, not styled", () => {
    const body = call(`${E}]0;title\u0007done\n`);
    expect(body.kind === "command" && body.output).toBe("done");
    expect(body.kind === "command" && body.styled).toBeUndefined();
  });

  test("hasTerminalCodes ignores ordinary line breaks", () => {
    expect(hasTerminalCodes("a\r\nb\n")).toBe(false);
    expect(hasTerminalCodes("a\rb")).toBe(true);
  });
});
