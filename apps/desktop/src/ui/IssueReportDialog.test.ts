import { describe, expect, test } from "bun:test";
import { LOG_TAIL_LINES, logSection } from "./IssueReportDialog";

describe("logSection", () => {
  test("folds a redacted log in a fence its lines cannot close", () => {
    const section = logSection(
      "INFO /Users/lars/code/app ok\nkey sk-abcdefghijklmnopqrstuvwxyz123456\n~~~~ end",
      "/Users/lars",
    );
    expect(section).toStartWith(`<details><summary>Thread log (last ${LOG_TAIL_LINES} lines)</summary>`);
    expect(section).toContain("~/code/app");
    expect(section).not.toContain("/Users/lars");
    expect(section).not.toContain("sk-abcdefghijklmnopqrstuvwxyz123456");
    expect(section.split("\n").filter((line) => line.startsWith("~~~~"))).toEqual(["~~~~text", "~~~~"]);
  });
});
