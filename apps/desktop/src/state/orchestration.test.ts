import { describe, expect, test } from "bun:test";
import {
  checkpointsByTurn,
  parseTurnDiff,
  parseWorktree,
  removeRefusal,
  suggestBranch,
  validBranch,
} from "../acp/v2";
import {
  capped,
  checkMessage,
  checkStart,
  emptyBook,
  forget,
  interruptActive,
  isFailure,
  type Link,
  type LinkBook,
  listSessions,
  parseCall,
  type RuleThread,
  sameProject,
} from "./orchestration";

const thread = (id: string, folder = "/repo", extra: Partial<RuleThread> = {}): RuleThread => ({
  id,
  title: `Thread ${id}`,
  folder,
  running: false,
  ...extra,
});

const link = (orchestrator: string, target: string, state: Link["state"] = "working"): Link => ({
  id: `${orchestrator}/${target}`,
  orchestrator,
  target,
  orchestratorTitle: orchestrator,
  targetTitle: target,
  message: "hi",
  state,
  createdAt: Date.now(),
});

const book = (links: Link[]): LinkBook => ({ ...emptyBook(), links });

describe("orchestration rules", () => {
  test("parses the v2 calls and refuses bad arguments", () => {
    expect(parseCall({ tool: "read_session", args: { session_id: "b", last_turns: 2 } })).toEqual({
      tool: "read_session",
      target: "b",
      lastTurns: 2,
    });
    expect(isFailure(parseCall({ tool: "read_session", args: { session_id: "b", last_turns: 11 } }))).toBe(
      true,
    );
    const fan = parseCall({
      tool: "fan_out",
      args: { prompt: "do it", harnesses: [{ harness: "rustcode" }, { harness: "codex", model: "m" }] },
    });
    expect(!isFailure(fan) && fan.tool === "fan_out" && fan.harnesses.length).toBe(2);
    expect(
      isFailure(parseCall({ tool: "fan_out", args: { prompt: "x", harnesses: [{ harness: "a" }] } })),
    ).toBe(true);
    expect(isFailure(parseCall({ tool: "start_session", args: { prompt: "  " } }))).toBe(true);
    expect(
      isFailure(parseCall({ tool: "message_session", args: { session_id: "b", message: "x".repeat(9000) } })),
    ).toBe(true);
    const wait = parseCall({ tool: "wait_session", args: { link_id: "a/1", timeout_ms: 9e9 } });
    expect(!isFailure(wait) && wait.tool === "wait_session" && wait.timeoutMs).toBe(3_600_000);
  });

  test("same project, no chains, four links, one orchestrator per target", () => {
    const a = thread("a");
    expect(checkMessage(a, thread("b", "/other"), emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, a, emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, undefined, emptyBook())?.code).toBe("not_found");
    // A worktree thread belongs to its repository's project.
    const wt = thread("w", "/wt/x", { worktree: { path: "/wt/x", branch: "b", repo: "/repo" } });
    expect(checkMessage(a, wt, emptyBook())).toBeNull();
    expect(checkMessage(a, thread("b"), book([link("z", "a")]))?.code).toBe("not_allowed");
    expect(checkMessage(a, thread("b"), book([link("z", "b")]))?.code).toBe("target_linked");
    const four = ["c", "d", "e", "f"].map((t) => link("a", t));
    expect(checkMessage(a, thread("b"), book(four))?.code).toBe("busy");
    expect(checkStart(a, book(four.slice(0, 2)), 3)?.code).toBe("busy");
    expect(checkStart(a, book(four.slice(0, 2)), 2)).toBeNull();
    expect(checkStart(a, book([link("z", "a")]), 1)?.code).toBe("not_allowed");
  });

  test("lists only the caller's project, with harness, status and worktree", () => {
    const wt = { path: "/wt/x", branch: "feat/x", repo: "/repo" };
    const out = listSessions(
      [thread("a"), thread("b", "/wt/x", { worktree: wt, harness: "codex" }), thread("c", "/elsewhere")],
      thread("a"),
      "",
    );
    const sessions = out.sessions as { session_id: string; harness: string; worktree?: unknown }[];
    expect(sessions.map((x) => x.session_id)).toEqual(["b"]);
    expect(sessions[0]?.harness).toBe("codex");
    expect(sessions[0]?.worktree).toEqual(wt);
  });

  test("caps on a character boundary, interrupts after restart, forgets deleted threads", () => {
    const { text, truncated } = capped("é".repeat(10), 5);
    expect(text).toBe("éé");
    expect(truncated).toBe(true);
    const b = interruptActive(book([link("a", "b"), link("a", "c", "done")]));
    expect(b.links.map((l) => l.state)).toEqual(["interrupted", "done"]);
    const f = forget(book([link("a", "b"), link("c", "a")]), "a");
    expect(f.links.map((l) => [l.target, l.state])).toEqual([["a", "failed"]]);
  });
});

describe("v2 shapes", () => {
  test("branch suggestions and checks", () => {
    expect(suggestBranch("Fix the composer focus ring")).toBe("fix/composer-focus-ring");
    expect(suggestBranch("Add a login form")).toBe("feat/add-login-form");
    expect(suggestBranch("")).toBe("feat/thread");
    expect(validBranch("feat/login-form")).toBe(true);
    expect(validBranch("-x")).toBe(false);
    expect(validBranch("a..b")).toBe(false);
    expect(validBranch("a b")).toBe(false);
  });

  test("worktrees, refusals, turn diffs and checkpoint pairing", () => {
    expect(parseWorktree({ path: "/p", branch: "b", repo: "/r", dirty: true, ahead: 2 })?.ahead).toBe(2);
    expect(removeRefusal({ reason: "dirty" })).toBe("dirty");
    expect(removeRefusal({ reason: "x" })).toBeUndefined();
    expect(
      parseTurnDiff({ files: [{ path: "a", status: "added", insertions: 1, patch: "+x" }] })[0]?.status,
    ).toBe("added");
    const cps = [1, 2, 3].map((turn) => ({
      turn,
      ref: "",
      prompt: `prompt ${turn}`,
      filesChanged: 1,
      insertions: 1,
      deletions: 0,
    }));
    const paired = checkpointsByTurn(
      [
        { id: "t2", prompt: "prompt 2" },
        { id: "t3", prompt: "something else" },
      ],
      cps,
    );
    expect(paired.t2?.turn).toBe(2);
    expect(paired.t3).toBeUndefined();
  });
});

describe("projects", () => {
  test("a macOS /private path is the folder it links from", () => {
    expect(sameProject("/private/tmp/app", "/tmp/app")).toBe(true);
    expect(sameProject("/private/var/folders/x/", "/var/folders/x")).toBe(true);
    expect(sameProject("/private/tmpx/app", "/tmpx/app")).toBe(false);
    expect(sameProject("/Users/me/a", "/Users/me/b")).toBe(false);
  });
});
