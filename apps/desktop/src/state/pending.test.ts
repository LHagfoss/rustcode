import { describe, expect, test } from "bun:test";
import type { PermissionRequest } from "../acp/types";
import type { Link, LinkState } from "./orchestration";
import { type PendingRead, pendingRequestsOn } from "./pending";

const link = (id: string, orchestrator: string, target: string, state: LinkState): Link => ({
  id,
  orchestrator,
  target,
  orchestratorTitle: `Thread ${orchestrator}`,
  targetTitle: `Thread ${target}`,
  message: "do it",
  state,
  createdAt: 1,
});

const ask = (sessionId: string, rpcId: number, at: number): PermissionRequest => ({
  rpcId,
  sessionId,
  toolCallId: `call-${rpcId}`,
  title: "bash",
  options: [{ id: "allow", name: "Allow", kind: "allow_once" }],
  diffs: [],
  at,
});

const read = (
  id: string,
  links: Link[],
  requests: Record<string, PermissionRequest[]>,
  titles: Record<string, string> = {},
): PendingRead => ({
  id,
  links,
  requests: (thread) => requests[thread],
  title: (thread) => titles[thread],
});

describe("pending requests from threads below", () => {
  test("a child's request is shown, named as the child", () => {
    const request = ask("a", 1, 10);
    const shown = pendingRequestsOn(
      read("p", [link("p/1", "p", "a", "working")], { a: [request] }, { a: "Fix it" }),
    );
    expect(shown).toHaveLength(1);
    expect(shown[0]).toMatchObject({ thread: "a", title: "Fix it", request, pending: 1, number: 1 });
  });

  test("several children are shown oldest first, whatever the link order", () => {
    const links = [link("p/1", "p", "a", "working"), link("p/2", "p", "b", "working")];
    const shown = pendingRequestsOn(
      read("p", links, { a: [ask("a", 1, 30)], b: [ask("b", 2, 10)] }, { a: "A", b: "B" }),
    );
    expect(shown.map((entry) => entry.thread)).toEqual(["b", "a"]);
  });

  test("a grandchild reaches the top-level thread once, named as itself", () => {
    const links = [link("p/1", "p", "a", "done"), link("a/1", "a", "g", "working")];
    const shown = pendingRequestsOn(read("p", links, { g: [ask("g", 3, 20)] }, { g: "Grandchild" }));
    expect(shown).toHaveLength(1);
    expect(shown[0]).toMatchObject({ thread: "g", title: "Grandchild" });
    // Its own parent shows it too, named the same way.
    expect(pendingRequestsOn(read("a", links, { g: [ask("g", 3, 20)] }, { g: "Grandchild" }))).toHaveLength(
      1,
    );
  });

  test("a request answered in the child leaves its parent at once", () => {
    const links = [link("p/1", "p", "a", "working")];
    const before = pendingRequestsOn(read("p", links, { a: [ask("a", 1, 10)] }));
    expect(before).toHaveLength(1);
    expect(pendingRequestsOn(read("p", links, { a: [] }))).toEqual([]);
  });

  test("a child no longer linked is not shown", () => {
    const request = ask("a", 1, 10);
    expect(pendingRequestsOn(read("p", [], { a: [request] }))).toEqual([]);
    expect(pendingRequestsOn(read("p", [link("p/1", "p", "a", "cancelled")], { a: [request] }))).toEqual([]);
    // A stopped grandchild's link severs it from the top-level thread too.
    const links = [link("p/1", "p", "a", "working"), link("a/1", "a", "g", "interrupted")];
    expect(pendingRequestsOn(read("p", links, { g: [request] }))).toEqual([]);
  });

  test("a thread's own request is not one from below it", () => {
    expect(pendingRequestsOn(read("p", [], { p: [ask("p", 1, 10)] }))).toEqual([]);
  });

  test("one child's requests are numbered only among its approvals", () => {
    const links = [link("p/1", "p", "a", "working")];
    const question: PermissionRequest = {
      ...ask("a", 3, 30),
      question: { header: "Which?", text: "Which one?", descriptions: [], multiple: false },
    };
    const shown = pendingRequestsOn(
      read("p", links, { a: [ask("a", 1, 10), ask("a", 2, 20), question] }, { a: "A" }),
    );
    expect(shown.map((entry) => `${entry.request.rpcId}:${entry.number}/${entry.pending}`)).toEqual([
      "1:1/2",
      "2:2/2",
      "3:1/2",
    ]);
  });

  test("a thread the app holds no title for is named by its link", () => {
    const shown = pendingRequestsOn(read("p", [link("p/1", "p", "a", "working")], { a: [ask("a", 1, 1)] }));
    expect(shown[0]?.title).toBe("Thread a");
  });
});
