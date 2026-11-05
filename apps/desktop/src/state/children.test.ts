import { describe, expect, test } from "bun:test";
import { childInfo, childState } from "./children";

describe("a child's state", () => {
  test("a thread waiting on you comes before one that works, and the rest are finished", () => {
    expect(childState("needs_input")).toBe("needs");
    expect(childState("running")).toBe("working");
    expect(childState("idle")).toBe("finished");
    expect(childState("review")).toBe("finished");
    expect(childState("done")).toBe("finished");
    expect(childState("idle", true)).toBe("failed");
  });
});

describe("a child read as its parent's stack sees it", () => {
  test("a live link beats a status that has not caught up", () => {
    expect(childInfo("a", { status: "idle", link: "working" }).state).toBe("working");
    expect(childInfo("a", { status: "idle", link: "needs_user" }).state).toBe("needs");
    expect(childInfo("a", { status: "needs_input", link: "done" }).state).toBe("needs");
  });

  test("a finished link leaves the thread's own status, and a failure marks it", () => {
    expect(childInfo("a", { status: "running", link: "done" }).state).toBe("working");
    expect(childInfo("a", { status: "idle", link: "cancelled", failed: true }).state).toBe("failed");
    expect(childInfo("a", {})).toEqual({ id: "a", state: "finished" });
  });
});
