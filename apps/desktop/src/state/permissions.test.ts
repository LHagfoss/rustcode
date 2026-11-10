import { describe, expect, test } from "bun:test";
import type { PermissionRequest } from "../acp/types";
import { autoAnswer, delegationAllowed, effectiveMode, parsePermissionMode } from "./permissions";

const request = (toolKind: string, extra: Partial<PermissionRequest> = {}): PermissionRequest => ({
  rpcId: 1,
  sessionId: "s",
  toolCallId: "t",
  title: "x",
  toolKind,
  options: [
    { id: "allow", name: "Allow", kind: "allow_once" },
    { id: "always", name: "Always", kind: "allow_always" },
    { id: "deny", name: "Deny", kind: "reject_once" },
  ],
  diffs: [],
  ...extra,
});

describe("autoAnswer", () => {
  test("Ask leaves every request to the user", () => {
    expect(autoAnswer(request("execute"), "ask")).toBeNull();
    expect(autoAnswer(request("edit"), "ask")).toBeNull();
  });

  test("Auto-edit allows edits and reads, not commands", () => {
    expect(autoAnswer(request("edit"), "edits")?.id).toBe("allow");
    expect(autoAnswer(request("read"), "edits")?.id).toBe("allow");
    expect(autoAnswer(request("execute"), "edits")).toBeNull();
    expect(autoAnswer(request("delete"), "edits")).toBeNull();
    expect(autoAnswer(request("other"), "edits")).toBeNull();
  });

  test("Full access allows everything but questions and plan reviews", () => {
    expect(autoAnswer(request("execute"), "full")?.id).toBe("allow");
    expect(autoAnswer(request("other"), "full")?.id).toBe("allow");
    expect(autoAnswer(request("switch_mode"), "full")).toBeNull();
    expect(
      autoAnswer(request("other", { question: { header: "", question: "?", options: [] } as never }), "full"),
    ).toBeNull();
  });

  test("Auto-edit reads RustCode's built-in tool names when no kind is sent", () => {
    expect(autoAnswer(request("", { toolKind: undefined, title: "replace_file_content" }), "edits")?.id).toBe(
      "allow",
    );
    expect(autoAnswer(request("", { toolKind: undefined, title: "run_command" }), "edits")).toBeNull();
    expect(autoAnswer(request("", { toolKind: undefined, title: "delete_file" }), "edits")).toBeNull();
  });

  test("a request without an allow option still asks", () => {
    expect(
      autoAnswer(
        request("execute", { options: [{ id: "deny", name: "Deny", kind: "reject_once" }] }),
        "full",
      ),
    ).toBeNull();
  });
});

test("unknown stored modes fall back to Ask", () => {
  expect(parsePermissionMode("full")).toBe("full");
  expect(parsePermissionMode("bogus")).toBe("ask");
  expect(parsePermissionMode(null)).toBe("ask");
});

describe("delegation policy", () => {
  test("Ask still prompts for every delegation", () => {
    for (const kind of ["read", "start", "message", "fan_out"] as const) {
      expect(delegationAllowed("ask", kind)).toBe(false);
    }
  });

  test("Auto-edit may read another thread, not start or message one", () => {
    expect(delegationAllowed("edits", "read")).toBe(true);
    expect(delegationAllowed("edits", "start")).toBe(false);
    expect(delegationAllowed("edits", "message")).toBe(false);
    expect(delegationAllowed("edits", "fan_out")).toBe(false);
  });

  test("Full access delegates without a prompt", () => {
    for (const kind of ["read", "start", "message", "fan_out"] as const) {
      expect(delegationAllowed("full", kind)).toBe(true);
    }
  });
});

describe("delegationAllowed for managing threads", () => {
  test("Auto-edit may organize, only Full access may stop or reconfigure", () => {
    expect(delegationAllowed("ask", "organize")).toBe(false);
    expect(delegationAllowed("edits", "organize")).toBe(true);
    expect(delegationAllowed("edits", "interrupt")).toBe(false);
    expect(delegationAllowed("edits", "configure")).toBe(false);
    expect(delegationAllowed("edits", "transfer")).toBe(false);
    expect(delegationAllowed("full", "interrupt")).toBe(true);
    expect(delegationAllowed("full", "configure")).toBe(true);
    expect(delegationAllowed("full", "transfer")).toBe(true);
    // Answering a request hands a delegate what it asked for, so Auto-edit does not either.
    expect(delegationAllowed("edits", "answer")).toBe(false);
    expect(delegationAllowed("full", "answer")).toBe(true);
  });
});

describe("effectiveMode", () => {
  const modes = ["ask", "edits", "full"] as const;

  test("a delegated thread never runs broader than the thread that started it", () => {
    const rank = { ask: 0, edits: 1, full: 2 };
    for (const parent of modes) {
      for (const requested of modes) {
        const mode = effectiveMode(parent, requested);
        // The lower of the two, whichever side it comes from.
        expect(rank[mode]).toBe(Math.min(rank[parent], rank[requested]));
        // And never above the parent, which is the rule that matters.
        expect(rank[mode]).toBeLessThanOrEqual(rank[parent]);
      }
    }
  });

  test("the parent's limit wins when Default access is broader", () => {
    expect(effectiveMode("ask", "full")).toBe("ask");
    expect(effectiveMode("ask", "edits")).toBe("ask");
    expect(effectiveMode("edits", "full")).toBe("edits");
  });

  test("a broader parent keeps a narrower default, and its own mode at most", () => {
    expect(effectiveMode("full", "ask")).toBe("ask");
    expect(effectiveMode("full", "edits")).toBe("edits");
    expect(effectiveMode("full", "full")).toBe("full");
    expect(effectiveMode("edits", "edits")).toBe("edits");
  });
});
