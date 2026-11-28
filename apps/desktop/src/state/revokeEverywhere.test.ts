import { describe, expect, test } from "bun:test";
import { addPending, answerOutcome, pendingFor, removedBothWays, revokeSummary } from "./revokeEverywhere";

describe("revoke everywhere", () => {
  test("an answer is revoked, not paired, or failed", () => {
    expect(answerOutcome({ revoked: 1, devices: ["Laptop"], errors: [] })).toBe("revoked");
    expect(answerOutcome({ revoked: 0, devices: [], errors: [] })).toBe("notPaired");
    expect(answerOutcome({ error: "a machine cannot revoke itself" })).toBe("failed");
    expect(answerOutcome(null)).toBe("failed");
  });

  test("the notice names where it took, what waits and what failed", () => {
    const text = revokeSummary("Laptop", [
      { name: "this computer", outcome: "revoked" },
      { name: "Studio", outcome: "revoked" },
      { name: "Mini", outcome: "waiting" },
      { name: "Old", outcome: "failed" },
      { name: "Pi", outcome: "notPaired" },
    ]);
    expect(text).toContain("Removed Laptop.");
    expect(text).toContain("Revoked on this computer, Studio.");
    expect(text).toContain("Waiting for Mini to connect.");
    expect(text).toContain("Could not revoke on Old");
    expect(text).not.toContain("Pi");
  });

  test("offline machines are remembered once per lost computer", () => {
    let list = addPending([], "gw-laptop", "Laptop", ["gw-mini"]);
    list = addPending(list, "gw-laptop", "Laptop", ["gw-mini", "gw-pi"]);
    expect(list).toEqual([{ hostId: "gw-laptop", name: "Laptop", machines: ["gw-mini", "gw-pi"] }]);
    expect(pendingFor(list, "gw-pi").map((item) => item.name)).toEqual(["Laptop"]);
    expect(pendingFor(list, "gw-studio")).toEqual([]);
    expect(addPending([], "gw-laptop", "Laptop", [])).toEqual([]);
  });

  test("removing a machine says both directions ended", () => {
    const there = "Removed Studio and ended this computer's access there.";
    expect(removedBothWays("Studio", there, "revoked")).toBe(
      `${there} Studio can no longer reach this computer.`,
    );
    expect(removedBothWays("Studio", there, "notPaired")).toBe(there);
    expect(removedBothWays("Studio", null, "notPaired")).toBe("Removed Studio.");
    expect(removedBothWays("Studio", there, "failed")).toContain("may still reach this computer");
  });
});
