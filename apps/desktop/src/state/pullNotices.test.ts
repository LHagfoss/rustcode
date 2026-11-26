import { expect, test } from "bun:test";
import { currentNews, parsePullNotice, pullNoticeText, queuePullNotice } from "./pullNotices";
import { pullNotice } from "./pullWatch";
import { PULL_MARK, type QueuedPrompt, withPullMark } from "./thread";

const row = (id: string, text: string): QueuedPrompt => ({ id, text });
const fresh = (text: string) => () => row("new", text);

test("a merge drops the check and conflict news before it", () => {
  expect(currentNews(["Checks failed: lint", "Has merge conflicts with its base", "Merged"])).toEqual([
    "Merged",
  ]);
  expect(pullNotice(4492, ["All checks passed", "Merged"])).toBe(
    `Pull request #4492: Merged\n\n${PULL_MARK}`,
  );
});

test("only the newest check result counts and other news stays", () => {
  expect(currentNews(["Checks failed: lint", "1 new review from ana", "All checks passed"])).toEqual([
    "1 new review from ana",
    "All checks passed",
  ]);
  expect(currentNews(["All checks passed", "Checks failed: test"])).toEqual(["Checks failed: test"]);
});

test("news for a busy thread folds into the queued notice instead of waking it again", () => {
  const failed = pullNotice(4488, ["Checks failed: lint, test"]);
  const typed = row("typed", "also update the docs");
  let queue = queuePullNotice([], failed, fresh(failed));
  queue = queuePullNotice([...queue, typed], pullNotice(4488, ["All checks passed"]), fresh("x"));
  queue = queuePullNotice(queue, pullNotice(4492, ["All checks passed"]), fresh("x"));
  queue = queuePullNotice(queue, pullNotice(4488, ["Merged"]), fresh("x"));
  expect(queue.map((q) => q.id)).toEqual(["new", "typed"]);
  expect(queue[0]?.text).toBe(
    pullNoticeText([
      [4488, ["Merged"]],
      [4492, ["All checks passed"]],
    ]),
  );
  expect(queue[0]?.text.split("\n")[0]).toBe("Pull request #4488: Merged · #4492: All checks passed");
});

test("other messages and a merge result with details are never folded", () => {
  const result = withPullMark('Pull request #7: merged\n\n{"merge":true}');
  expect(parsePullNotice(result)).toBeNull();
  expect(parsePullNotice("Pull request #7: Merged")).toBeNull();
  const queue = queuePullNotice([row("result", result)], pullNotice(7, ["Merged"]), fresh("own"));
  expect(queue.map((q) => q.id)).toEqual(["result", "new"]);
  expect(queuePullNotice([], "hello", fresh("hello")).map((q) => q.text)).toEqual(["hello"]);
});

test("a notice reads back the pull requests and news it was written with", () => {
  const news: [number, string[]][] = [
    [12, ["Checks failed: lint", "2 new reviews or comments from ana, bo"]],
    [14, ["Merged"]],
  ];
  expect(parsePullNotice(pullNoticeText(news))).toEqual(news);
});
