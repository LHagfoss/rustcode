// The inbox's answers per row: a destructive approval is answered once or
// declined, never granted, even when the harness offered a grant.

import { describe, expect, test } from "bun:test";
import type { PermissionRequest } from "../acp/types";
import type { InboxItem } from "../state/inbox";
import { inboxActions } from "./Inbox";

const project = "/Users/me/code/app";

function approval(title: string, input: PermissionRequest["input"]): InboxItem {
  const request: PermissionRequest = {
    rpcId: 1,
    sessionId: "t1",
    toolCallId: "c1",
    title,
    toolKind: "execute",
    input,
    diffs: [],
    options: [
      { id: "allow", name: "Allow once", kind: "allow_once" },
      { id: "allow_hour", name: "Allow for 1 hour", kind: "allow_always" },
      { id: "always", name: "Always allow", kind: "allow_always" },
      { id: "reject", name: "Reject", kind: "reject_once" },
    ],
  };
  return {
    key: "t1:request:1",
    kind: "approval",
    threadId: "t1",
    title: "A thread",
    project,
    detail: `Wants to ${title}`,
    at: 1,
    request,
    pending: 1,
    number: 1,
  };
}

describe("inbox answers", () => {
  test("a destructive command offers only Allow and Decline", () => {
    const actions = inboxActions(approval("bash", { command: "rm -rf node_modules && bun install" }));
    expect(actions.map((a) => a.label)).toEqual(["Allow", "Decline", "Open"]);
    expect(actions[0]?.primary).toBe(true);
  });

  test("a command that is not destructive keeps the grant", () => {
    const actions = inboxActions(approval("bash", { command: "cargo test --lib" }));
    expect(actions.map((a) => a.label)).toEqual(["Allow", "1 hour", "Always", "Decline", "Open"]);
  });

  test("a question keeps its own answers", () => {
    const item = approval("Which tone?", { header: "Tone", options: [] });
    item.kind = "question";
    item.request!.question = { header: "Tone", text: "Which tone?", descriptions: [], multiple: false };
    item.request!.options = [
      { id: "plain", name: "Plain", kind: "allow_once" },
      { id: "warm", name: "Warm", kind: "allow_once" },
    ];
    expect(inboxActions(item).map((a) => a.label)).toEqual(["Plain", "Warm", "Open"]);
  });
});
