import { describe, expect, test } from "bun:test";
import {
  applyCompletion,
  completionToken,
  type PullReference,
  pullPeople,
  rankPeople,
  rankReferences,
} from "./pullCompletion";

describe("completionToken", () => {
  test("finds # and @ at the start, after a space or a bracket", () => {
    expect(completionToken("#12", 3)).toEqual({ trigger: "#", start: 0, end: 3, query: "12" });
    expect(completionToken("see @rev", 8)).toEqual({ trigger: "@", start: 4, end: 8, query: "rev" });
    expect(completionToken("(#", 2)?.trigger).toBe("#");
  });

  test("ignores a trigger inside a word, after a space, mid-word or in code", () => {
    expect(completionToken("a#1", 3)).toBeNull();
    expect(completionToken("me@host", 7)).toBeNull();
    expect(completionToken("#12 now", 7)).toBeNull();
    expect(completionToken("@revie", 3)).toBeNull();
    expect(completionToken("`#1", 3)).toBeNull();
  });
});

describe("pullPeople", () => {
  test("lists the author, reviewers and commenters once each", () => {
    const people = pullPeople({
      author: "LHagfoss",
      activity: [
        { author: "reviewer", kind: "review", body: "" },
        { author: "lhagfoss", kind: "comment", body: "" },
      ],
      reviewComments: [
        { id: 1, author: "maintainer", path: "a", outdated: false, body: "" },
        { id: 2, author: "", path: "a", outdated: false, body: "" },
      ],
    } as never);
    expect(people).toEqual(["LHagfoss", "reviewer", "maintainer"]);
  });
});

describe("rankPeople", () => {
  test("puts logins starting with the query first", () => {
    expect(rankPeople(["maintainer", "reviewer", "ana"], "a").map((i) => i.insert)).toEqual([
      "@ana",
      "@maintainer",
    ]);
    expect(rankPeople(["a", "b"], "").length).toBe(2);
  });
});

describe("rankReferences", () => {
  const refs: PullReference[] = [
    { number: 3768, title: "Reply to pull request comments", kind: "issue", state: "open" },
    { number: 3769, title: "Complete issues and people", kind: "issue", state: "open" },
    { number: 37, title: "Old pull", kind: "pull", state: "merged" },
  ];

  test("matches digits by number, exact first", () => {
    expect(rankReferences(refs, "37").map((i) => i.insert)).toEqual(["#37", "#3769", "#3768"]);
  });

  test("matches words in the title", () => {
    const items = rankReferences(refs, "pull comments");
    expect(items.map((i) => i.insert)).toEqual(["#3768"]);
    expect(items[0]?.detail).toBe("Issue · Reply to pull request comments");
  });

  test("lists the newest first with no query", () => {
    expect(rankReferences(refs, "")[0]?.insert).toBe("#3769");
  });
});

describe("applyCompletion", () => {
  test("replaces the token and adds one space", () => {
    expect(applyCompletion("see #37", { start: 4, end: 7 }, "#3768")).toEqual({
      text: "see #3768 ",
      caret: 10,
    });
    expect(applyCompletion("@re thanks", { start: 0, end: 3 }, "@reviewer")).toEqual({
      text: "@reviewer thanks",
      caret: 9,
    });
  });
});
