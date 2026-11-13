import { describe, expect, test } from "bun:test";
import type { MirrorStatus } from "../state/machineMirror";
import { mirrorLine, parseProjects } from "./ThreadMirrorGroup";

const status = (enabled: boolean, hosts: MirrorStatus["hosts"]): MirrorStatus => ({
  enabled,
  excludedProjects: [],
  hosts,
});

describe("what the thread copy says it holds", () => {
  test("counts threads and environments", () => {
    const hosts = {
      a: { threads: 3, transcripts: 1, bytes: 1 },
      b: { threads: 1, transcripts: 0, bytes: 0 },
    };
    expect(mirrorLine(status(true, hosts))).toBe("Holding 4 threads from 2 environments.");
    expect(mirrorLine(status(true, { a: { threads: 1, transcripts: 0, bytes: 0 } }))).toBe(
      "Holding 1 thread from 1 environment.",
    );
  });

  test("says when nothing is copied yet or the switch is off", () => {
    expect(mirrorLine(status(true, {}))).toBe("Nothing copied yet.");
    expect(mirrorLine(status(false, {}))).toBe("Off. Nothing is copied.");
    expect(mirrorLine(status(false, { a: { threads: 2, transcripts: 0, bytes: 0 } }))).toBe(
      "Off. Still holding 2 threads from 1 environment; they stay readable.",
    );
  });

  test("says when the server does not answer", () => {
    expect(mirrorLine(null)).toContain("did not answer");
  });
});

describe("the projects left out", () => {
  test("is one folder per line without blanks", () => {
    expect(parseProjects(" /a/b \r\n\n/c\n  ")).toEqual(["/a/b", "/c"]);
    expect(parseProjects("")).toEqual([]);
  });
});
