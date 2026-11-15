import { expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { mergeResultNotice, noProviderPullList } from "./host";
import { isContinueMessage, noticeLine } from "./thread";

test("background merge results are durable notices with the complete outcome for the agent", () => {
  // Each result and the line a person reads for it, written out: the detail, then the error's
  // message, then the bare state, then a fallback for a result that says nothing.
  const cases: [JsonObject, string][] = [
    [
      { state: "merged", merged: true, detail: "Squash-merged and the remote branch was deleted" },
      "Pull request #3792: Squash-merged and the remote branch was deleted",
    ],
    [
      { state: "failing", merged: false, failing: ["Desktop"], detail: "Checks failed" },
      "Pull request #3792: Checks failed",
    ],
    [
      { error: { code: "failed", message: "The engine did not answer" } },
      "Pull request #3792: The engine did not answer",
    ],
    [{ state: "timed_out", merged: false }, "Pull request #3792: timed_out"],
    [{}, "Pull request #3792: Merge failed"],
  ];
  for (const [result, line] of cases) {
    const text = mergeResultNotice(3792, result);
    expect(isContinueMessage(text)).toBe(true);
    expect(noticeLine(text)).toBe(line);
    expect(text).toContain(JSON.stringify(result));
  }
});

test("unsupported remote returns an empty pull request list with a note", () => {
  expect(noProviderPullList({ code: "no_provider" }, {})).toEqual({
    pull_requests: [],
    note: "no pull request provider for this remote",
  });
  expect(noProviderPullList({ code: "not_connected" }, {})).toBeNull();
});
