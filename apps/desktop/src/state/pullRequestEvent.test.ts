import { expect, test } from "bun:test";
import { pullRequestEvent } from "./pullRequestEvent";

test("reads a pull request wake line as its event", () => {
  expect(pullRequestEvent("Pull request #4580: squash-merged and the remote branch was deleted")).toBe(
    "merged",
  );
  expect(pullRequestEvent("Pull request #4579: a check failed; nothing was merged")).toBe("failed");
  expect(
    pullRequestEvent(
      "Pull request #4579: Has merge conflicts with its base · Checks failed: test · #4577: Merged",
    ),
  ).toBe("failed");
  expect(pullRequestEvent("Pull request #4579: 1 new review or comment from github-actions")).toBe("comment");
  expect(pullRequestEvent("Continued after compaction")).toBeUndefined();
});
