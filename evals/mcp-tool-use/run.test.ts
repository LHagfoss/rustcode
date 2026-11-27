import { describe, expect, test } from "bun:test";
import { CASES } from "./cases.ts";
import { type Call, casePassed, grade, matches } from "./grade.ts";
import { answer, summary, systemPrompt } from "./run.ts";

const title: Call = { tool: "update_thread", input: { title: "Fix login test" } };
const settle: Call = { tool: "update_thread", input: { status: "settled" } };
const bash = (command: string): Call => ({ tool: "Bash", input: { command } });

describe("matches", () => {
  test("compares args and checks set and missing keys", () => {
    expect(matches(settle, { tool: "update_thread", args: { status: "settled" }, lacks: ["thread_id"] })).toBe(true);
    expect(matches(settle, { tool: "update_thread", has: ["title"] })).toBe(false);
    expect(matches({ tool: "update_thread", input: { status: "settled", thread_id: "a" } }, {
      tool: "update_thread",
      lacks: ["thread_id"],
    })).toBe(false);
    expect(matches(title, { tool: "list_threads" })).toBe(false);
  });
});

describe("grade", () => {
  test("a sample that titles first and settles passes", () => {
    const failures = grade({ calls: [title, settle], text: "" }, {
      first: { tool: "update_thread", has: ["title"] },
      calls: [{ tool: "update_thread", args: { status: "settled" } }],
      never: [{ tool: "merge_when_green" }],
    });
    expect(failures).toEqual([]);
  });

  test("each broken check names what happened", () => {
    const failures = grade({ calls: [bash("sleep 60 && gh pr checks 12"), settle], text: "paste it here" }, {
      first: { tool: "update_thread", has: ["title"] },
      anyOf: [{ tool: "list_threads" }, { tool: "search_threads" }],
      never: [{ tool: "update_thread", args: { status: "settled" } }],
      bashNever: [/\bsleep\b/],
      bashSome: [/git commit/],
      textNever: [/paste it here/],
    });
    expect(failures).toEqual([
      "first call was Bash, wanted update_thread(title set)",
      "called none of list_threads, search_threads",
      'called update_thread(status="settled")',
      "ran `sleep 60 && gh pr checks 12` (matches /\\bsleep\\b/)",
      "ran nothing matching /git commit/",
      "answer matches /paste it here/",
    ]);
  });

  test("a sample with no calls fails a first-call check", () => {
    expect(grade({ calls: [], text: "" }, { first: { tool: "update_thread" } })).toEqual([
      "first call was none, wanted update_thread",
    ]);
  });
});

test("a case fails when half or more of its samples fail", () => {
  expect(casePassed(0, 3)).toBe(true);
  expect(casePassed(1, 3)).toBe(true);
  expect(casePassed(2, 3)).toBe(false);
  expect(casePassed(1, 2)).toBe(false);
  expect(casePassed(0, 0)).toBe(false);
});

test("case ids are unique and every case names its source", () => {
  const ids = CASES.map((c) => c.id);
  expect(new Set(ids).size).toBe(ids.length);
  for (const c of CASES) expect(c.source.length).toBeGreaterThan(0);
  expect(CASES.some((c) => c.control)).toBe(true);
});

test("canned answers match the shell command or default", () => {
  const commit = CASES.find((c) => c.id === "commit-without-attribution")!;
  expect(answer(commit, bash("git status --short"))).toContain("README.md");
  expect(answer(commit, bash("ls"))).toBe("");
  expect(answer(commit, settle)).toBe('{"ok":true}');
});

test("the system prompt adds AGENTS.md and the harness note only when the case asks", () => {
  const manifest = { instructions: "RULES", tools: [] };
  const commit = CASES.find((c) => c.id === "commit-without-attribution")!;
  const plain = CASES.find((c) => c.id === "title-first")!;
  expect(systemPrompt(manifest, commit, "AGENTS TEXT")).toContain("AGENTS TEXT");
  expect(systemPrompt(manifest, commit, "AGENTS TEXT")).toContain("Co-Authored-By");
  expect(systemPrompt(manifest, plain, "AGENTS TEXT")).not.toContain("AGENTS TEXT");
  expect(systemPrompt(manifest, plain, "")).toContain("## rustcode\nRULES");
});

test("the summary shows each case against its baseline", () => {
  const row = (id: string, failures: string[]) => ({
    case: id, sample: 1, model: "m", failures, calls: [], text: "", usage: { input: 10, output: 2 },
  });
  const text = summary([row("a", []), row("b", ["called x"])], "m", [row("a", ["called y"])]);
  expect(text).toContain("| a | 1/1 pass | 0/1 FAIL |");
  expect(text).toContain("| b | 0/1 FAIL | – |");
  expect(text).toContain("- b #1: called x");
  expect(text).toContain("Tokens: 20 input, 4 output.");
});
