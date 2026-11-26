import { describe, expect, test } from "bun:test";
import { type JsonObject, RpcError } from "../acp/jsonrpc";
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
        throw new RpcError(-32601, "Method not found");
      },
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    expect(report).toBeNull();
    expect(describeCopy(report, "VPS", "MacBook")).toContain("too old");
  });

  test("any other listing failure is thrown, not read as too old", async () => {
    const failing = copyFiles(
      async () => {
        throw new Error("connection closed");
      },
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    await expect(failing).rejects.toThrow("connection closed");
  });

  test("an entry the follower already holds is recorded as a copy and pruned later", async () => {
    const main = new MockConfigFiles({ "skills/shared": "same" });
    const follower = new MockConfigFiles({ "skills/shared": "same" });
    const first = await copyFiles(
      async (host, method, params) => (host === "main" ? main : follower).answer(method, params),
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    expect(first?.written).toEqual([]);
    const emptyMain = new MockConfigFiles({});
    const after = await copyFiles(
      async (host, method, params) => (host === "main" ? emptyMain : follower).answer(method, params),
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    expect(after?.removed).toEqual(["skills/shared"]);
  });

  test("a failed cleanup is reported and does not lose the copies", async () => {
    const { main, follower } = machines();
    const report = await copyFiles(
      async (host, method, params) => {
        if (method === "_rustcode/config/files/prune") throw new Error("disk full");
        return (host === "main" ? main : follower).answer(method, params);
      },
      "main",
      "follower",
      "gw-main",
      ["skills"],
    );
    expect(report?.written).toEqual(["skills/github-voice"]);
    expect(report?.skipped).toEqual(["skills cleanup (disk full)"]);
  });

  test("a copy with nothing to do says so", () => {
    expect(describeCopy({ written: [], removed: [], skipped: [] }, "VPS", "MacBook")).toBe(
      "MacBook already has VPS's skills, themes and commands.",
    );
  });
});
