import { expect, test } from "bun:test";
import { noProviderPullList } from "./host";

test("unsupported remote returns an empty pull request list with a note", () => {
  expect(noProviderPullList({ code: "no_provider" }, {})).toEqual({
    pull_requests: [],
    note: "no pull request provider for this remote",
  });
  expect(noProviderPullList({ code: "not_connected" }, {})).toBeNull();
});
