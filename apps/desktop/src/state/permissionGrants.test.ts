import { describe, expect, test } from "bun:test";
import { delegateAccountKey, emptyBook, grant } from "./orchestration";
import { describeGrantKey, revokeThreadGrant, threadGrants } from "./permissionGrants";

const titles: Record<string, string> = { t1: "Fix the build" };
const titleOf = (id: string) => titles[id];

describe("remembered approvals", () => {
  test("say what each key allows", () => {
    expect(describeGrantKey("start@local", titleOf)).toBe("Start threads");
    expect(describeGrantKey("start@m2:/work/app", titleOf)).toBe(
      "Start threads on another computer in /work/app",
    );
    expect(describeGrantKey("start:/work/app", titleOf)).toBe("Start threads in /work/app");
    expect(describeGrantKey("t1", titleOf)).toBe("Read and message “Fix the build”");
    expect(describeGrantKey("merge:t1:42", titleOf)).toBe("Merge “Fix the build”'s pull request");
    expect(describeGrantKey(delegateAccountKey("local", "claude", "work"), titleOf)).toBe(
      "Delegate on claude with account “work”",
    );
    expect(describeGrantKey(delegateAccountKey("m2", "codex", null), titleOf)).toBe(
      "Delegate on codex with the default sign-in on another computer",
    );
  });

  test("list and revoke one without touching the rest", () => {
    let book = grant(emptyBook(), "a", "start@local");
    book = grant(book, "a", "t1");
    expect(threadGrants(book, titleOf).map((g) => g.key)).toEqual(["start@local", "t1"]);
    book = revokeThreadGrant(book, "a", "start@local");
    expect(book.grants).toEqual({ a: ["t1"] });
    book = revokeThreadGrant(book, "a", "t1");
    expect(book.grants).toEqual({});
    expect(revokeThreadGrant(book, "a", "missing")).toBe(book);
  });
});
