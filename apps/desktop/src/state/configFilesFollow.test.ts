import { describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { MockConfigFiles } from "../acp/mockConfigFiles";
import { copyFiles, describeCopy, planCopy } from "./configFilesFollow";

describe("planCopy", () => {
  test("copies what differs and keeps every listed name, skipped ones too", () => {
    const plan = planCopy(
      [
        { kind: "skills", name: "same", hash: "a" },
        { kind: "skills", name: "changed", hash: "b" },
        { kind: "skills", name: "huge", hash: "", skipped: "too large" },
        { kind: "themes", name: "rain.toml", hash: "t" },
      ],
      [
        { kind: "skills", name: "same", hash: "a" },
        { kind: "skills", name: "changed", hash: "old" },
      ],
      ["skills"],
    );
    expect(plan.copy.map((entry) => entry.name)).toEqual(["changed"]);
    expect(plan.keep.skills).toEqual(["same", "changed", "huge"]);
    expect(plan.keep.themes).toEqual([]);
    expect(plan.skipped).toEqual(["skills/huge (too large)"]);
  });
});

describe("copyFiles", () => {
  const machines = () => {
    const main = new MockConfigFiles({ "skills/github-voice": "v1", "themes/rain.toml": "rain" });
    const follower = new MockConfigFiles({ "skills/mine": "own" });
    const request = async (host: string, method: string, params: JsonObject) =>
      (host === "main" ? main : follower).answer(method, params);
    return { main, follower, request };
  };

  test("copies to the follower, then prunes only the copies from that machine", async () => {
    const { main, follower, request } = machines();
    const first = await copyFiles(request, "main", "follower", "gw-main", ["skills", "themes"]);
    expect(first?.written.sort()).toEqual(["skills/github-voice", "themes/rain.toml"]);

    main.answer("_rustcode/config/files/prune", { source: "", kind: "skills", keep: [] });
    const listing = follower.answer("_rustcode/config/files/list", { kinds: ["skills"] });
    expect(JSON.stringify(listing)).toContain("github-voice");

    const mainNow = new MockConfigFiles({ "themes/rain.toml": "rain" });
    const after = await copyFiles(
      async (host, method, params) => (host === "main" ? mainNow : follower).answer(method, params),
      "main",
      "follower",
      "gw-main",
      ["skills", "themes"],
    );
    expect(after?.removed).toEqual(["skills/github-voice"]);
    const left = follower.answer("_rustcode/config/files/list", { kinds: ["skills"] });
    expect(JSON.stringify(left)).toContain('"mine"');
  });

  test("an engine without the methods reads as too old", async () => {
    const report = await copyFiles(
      async () => {
        throw new Error("Method not found");
      },
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    expect(report).toBeNull();
    expect(describeCopy(report, "VPS", "MacBook")).toContain("too old");
  });

  test("a copy with nothing to do says so", () => {
    expect(describeCopy({ written: [], removed: [], skipped: [] }, "VPS", "MacBook")).toBe(
      "MacBook already has VPS's skills, themes and commands.",
    );
  });
});
