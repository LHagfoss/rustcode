import { describe, expect, test } from "bun:test";
import { roleChanges, roleCommand, roleFor } from "./hubFleet";

describe("roleFor", () => {
  test("the chosen machine is the hub and every other one links to it", () => {
    expect(roleFor("vps", "vps")).toEqual({ kind: "hub" });
    expect(roleFor("laptop", "vps")).toEqual({ kind: "member", hub: "vps" });
    expect(roleFor("laptop", null)).toEqual({ kind: "none" });
  });
});

describe("roleChanges", () => {
  test("a member joins at full access without asking", () => {
    expect(roleChanges({ kind: "member", hub: "vps" })).toEqual([
      { action: "off" },
      { action: "join", machine: "vps", access: "admin", ask: false },
    ]);
    expect(roleChanges({ kind: "hub" })).toEqual([{ action: "leave" }, { action: "on" }]);
    expect(roleChanges({ kind: "none" })).toEqual([{ action: "off" }, { action: "leave" }]);
  });
});

describe("roleCommand", () => {
  test("another machine runs the same changes as one command line", () => {
    expect(roleCommand({ kind: "member", hub: "gw-vps" })).toBe(
      'rc machines hub off && rc machines hub join "gw-vps" --access admin --ask false',
    );
    expect(roleCommand({ kind: "hub" })).toBe("rc machines hub leave && rc machines hub on");
  });
});
