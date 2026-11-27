import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { pullDiffFiles } from "./pullReview";
import {
  excerptLines,
  GUIDE_STORAGE,
  guidePrompt,
  LEFTOVER_TITLE,
  parseGuide,
  savedGuide,
  saveGuide,
  stopQuestion,
} from "./reviewGuide";

const FILES = pullDiffFiles(
  [
    "diff --git a/src/a.ts b/src/a.ts",
    "--- a/src/a.ts",
    "+++ b/src/a.ts",
    "@@ -10,3 +10,4 @@",
    " one",
    "-two",
    "+TWO",
    "+three",
    " four",
    "diff --git a/docs/b.md b/docs/b.md",
    "--- a/docs/b.md",
    "+++ b/docs/b.md",
    "@@ -1 +1 @@",
    "-old",
    "+new",
  ].join("\n"),
);

const answer = (stops: unknown[]) => JSON.stringify({ stops });

describe("guidePrompt", () => {
  test("numbers new lines by their number and removed ones as old", () => {
    const prompt = guidePrompt({ number: 7, title: "Rename two" }, "Why", FILES);
    expect(prompt).toContain("Pull request #7: Rename two");
    expect(prompt).toContain("=== src/a.ts [M, +2 −1]");
    expect(prompt).toContain("10   one");
    expect(prompt).toContain("old 11 - two");
    expect(prompt).toContain("11 + TWO");
    expect(prompt).toContain("12 + three");
  });
});

describe("excerptLines", () => {
  test("takes the removed lines between the cited new ones", () => {
    const lines = excerptLines(FILES[0]!, "new", 10, 11);
    expect(lines.map((l) => l.text)).toEqual(["one", "two", "TWO"]);
  });

  test("cites removed lines on the old side and nothing the diff lacks", () => {
    expect(excerptLines(FILES[0]!, "old", 11, 11).map((l) => l.text)).toEqual(["two"]);
    expect(excerptLines(FILES[0]!, "new", 90, 95)).toEqual([]);
  });
});

describe("parseGuide", () => {
  test("keeps checked excerpts and gathers uncovered files into a last stop", () => {
    const guide = parseGuide(
      "Here it is:\n```json\n" +
        answer([
          {
            title: "Rename two",
            summary: "Renames the constant.",
            lookClosely: "Callers outside the repo break.",
            excerpts: [
              { path: "src/a.ts", side: "new", start: 12, end: 11 },
              { path: "src/missing.ts", side: "new", start: 1, end: 2 },
              { path: "src/a.ts", side: "new", start: 400, end: 401 },
            ],
            files: ["src/a.ts", "nowhere.ts"],
            questions: ["Is anything else named two?", "", "b", "c"],
          },
        ]) +
        "\n```",
      FILES,
      "abc",
      5,
    );
    expect(guide.headSha).toBe("abc");
    expect(guide.writtenAt).toBe(5);
    const [stop, rest] = guide.stops;
    expect(stop!.lookClosely).toBe("Callers outside the repo break.");
    expect(stop!.excerpts).toHaveLength(1);
    expect(stop!.excerpts[0]).toMatchObject({ path: "src/a.ts", start: 11, end: 12 });
    expect(stop!.files).toEqual(["src/a.ts"]);
    expect(stop!.questions).toEqual(["Is anything else named two?", "b"]);
    expect(rest).toMatchObject({ title: LEFTOVER_TITLE, files: ["docs/b.md"] });
  });

  test("drops empty stops and refuses an answer with none", () => {
    expect(() => parseGuide("no json here", FILES, "abc")).toThrow("not JSON");
    expect(() => parseGuide('{"stops": 3}', FILES, "abc")).toThrow("no stops");
    const guide = parseGuide(answer([{ title: "" }, { title: "Only a title" }, null]), FILES, "abc");
    expect(guide.stops.map((s) => s.title)).toEqual([LEFTOVER_TITLE]);
    expect(() => parseGuide(answer([]), [], "abc")).toThrow("no stops");
  });
});

describe("stopQuestion", () => {
  test("names the pull request, the stop and its lines", () => {
    const guide = parseGuide(
      answer([{ title: "Core", summary: "s", excerpts: [{ path: "src/a.ts", start: 11, end: 12 }] }]),
      FILES,
      "abc",
    );
    const draft = stopQuestion({ number: 7, title: "T", url: "https://x/7" }, guide.stops[0]!, "Why?");
    expect(draft).toBe(
      'About pull request #7 (T): https://x/7\nReview guide stop "Core", at src/a.ts:11-12.\n\nWhy?',
    );
  });
});

describe("saved guides", () => {
  const data = new Map<string, string>();
  const scope = globalThis as unknown as Record<string, unknown>;
  beforeEach(() => {
    data.clear();
    scope.localStorage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
    };
  });
  afterEach(() => delete scope.localStorage);

  test("are kept per pull request and the newest win", () => {
    expect(savedGuide("/repo", 1)).toBeNull();
    for (let n = 0; n < 35; n++) saveGuide("/repo", n, { headSha: `s${n}`, writtenAt: n, stops: [] });
    expect(savedGuide("/repo", 34)?.headSha).toBe("s34");
    expect(savedGuide("/repo", 0)).toBeNull();
    expect(Object.keys(JSON.parse(data.get(GUIDE_STORAGE)!))).toHaveLength(30);
  });
});
