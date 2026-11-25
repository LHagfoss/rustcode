import { describe, expect, test } from "bun:test";
import { showsPage } from "./viewActions";

describe("showsPage", () => {
  test("Dashboard, Jobs, the inbox and the board replace the thread", () => {
    for (const kind of ["dashboard", "loops", "inbox", "board"] as const)
      expect(showsPage({ kind })).toBe(true);
  });

  test("the chat, a review and a comparison still belong to a thread", () => {
    expect(showsPage({ kind: "chat" })).toBe(false);
    expect(showsPage({ kind: "review", threadId: "t1" })).toBe(false);
    expect(showsPage({ kind: "compare", fanOut: "f1" })).toBe(false);
  });
});
