import { expect, test } from "bun:test";
import { type JsonObject, obj, str } from "../acp/jsonrpc";
import { mergeResultNotice, noProviderPullList } from "./host";
import { isContinueMessage, noticeLine } from "./thread";

test("background merge results are durable notices with the complete outcome for the agent", () => {
  const results: JsonObject[] = [
    { state: "merged", merged: true, detail: "Squash-merged and the remote branch was deleted" },
    { state: "failing", merged: false, failing: ["Desktop"], detail: "Checks failed" },
    { error: { code: "failed", message: "The engine did not answer" } },
  ];
  for (const result of results) {
    const text = mergeResultNotice(3792, result);
    expect(isContinueMessage(text)).toBe(true);
    expect(noticeLine(text)).toBe(
      `Pull request #3792: ${str(result.detail) || str(obj(result.error)?.message)}`,
    );
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
