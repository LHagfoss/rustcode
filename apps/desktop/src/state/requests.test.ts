// The v10 tools: answering what a thread waits on (`answer_request`) and
// organizing several at once (`organize_threads`), plus the files
// `message_session` carries. The rules they share with `update_session` —
// the caller's own delegation tree, the project, the user's approval — are
// checked through the same `_rustcode/orchestration` entry the app answers.

import { beforeEach, describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import { answerOrchestration } from "./host";
import { emptyBook, type Link } from "./orchestration";
import {
  connect,
  openThread,
  resetForTests,
  selectThread,
  setOrchestration,
  updateThread,
  useApp,
} from "./store";

const until = async (check: () => boolean, ms = 3000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

/** Two threads of one project in the mock engine, the second handed a task by the first. */
async function staged() {
  setOrchestration(true);
  await connect(new MockTransport({ delayMs: 0 }));
  await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
  // The mock lists a thread of another project too; a link needs two of the same one.
  const same = useApp.getState().sessions.filter((x) => x.cwd === "/Users/you/code/rustcode");
  const [parent, child] = same.map((x) => x.id) as [string, string];
  selectThread(parent);
  await until(() => useApp.getState().threads[parent]?.loaded === true);
  // A thread that asks has to be open for its request to be the app's to see.
  await openThread(child);
  await until(() => !!useApp.getState().threads[child]);
  const link: Link = {
    id: `${parent}/1`,
    orchestrator: parent,
    target: child,
    orchestratorTitle: "Parent",
    targetTitle: "Child",
    message: "do it",
    state: "needs_user",
    createdAt: Date.now(),
  };
  useApp.setState({ links: { ...emptyBook(), links: [link], counters: { [parent]: 1 } } });
  return { parent, child, link };
}

const ask = (thread: string, rpcId: number, extra: JsonObject = {}) => ({
  rpcId,
  sessionId: thread,
  toolCallId: `call-${rpcId}`,
  title: "Run: cargo test",
  options: [
    { id: "allow_once", name: "Allow once", kind: "allow_once" as const },
    { id: "allow_always", name: "Always allow", kind: "allow_always" as const },
    { id: "reject", name: "Decline", kind: "reject_once" as const },
  ],
  diffs: [],
  at: Date.now(),
  ...extra,
});

const errorOf = (answer: JsonObject) =>
  (answer.error as { code?: string; message?: string } | undefined) ?? {};

beforeEach(() => resetForTests());

describe("answer_request", () => {
  test("cannot answer its own request", async () => {
    const { parent } = await staged();
    await until(() => !!useApp.getState().threads[parent]);
    updateThread(parent, (t) => ({ ...t, permissions: [ask(parent, 3)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: parent, request_id: "3", choice: "allow_once" },
    });
    expect(errorOf(answer).code).toBe("not_allowed");
    expect(errorOf(answer).message).toContain("cannot answer its own request");
    // Still waiting: a refusal never answers the card.
    expect(useApp.getState().threads[parent]?.permissions).toHaveLength(1);
  });

  test("cannot answer a thread outside its delegation tree", async () => {
    const { parent, child } = await staged();
    // A third thread, in the same project but under nobody: not this thread's to answer.
    const other = useApp
      .getState()
      .sessions.find((x) => x.cwd === "/Users/you/code/rustcode" && x.id !== parent && x.id !== child)?.id;
    expect(other).toBeDefined();
    await openThread(other!);
    await until(() => !!useApp.getState().threads[other!]);
    updateThread(other!, (t) => ({ ...t, permissions: [ask(other!, 9)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: other!, request_id: "9", choice: "allow_once" },
    });
    expect(errorOf(answer).code).toBe("not_allowed");
    expect(errorOf(answer).message).toContain("delegation tree");
    expect(useApp.getState().threads[other!]?.permissions).toHaveLength(1);
    // The child below it is another matter: that one is the caller's own.
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 4)] }));
  });

  test("a request that is no longer waiting is not answered", async () => {
    const { parent, child } = await staged();
    updateThread(child, (t) => ({ ...t, permissions: [] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "5", choice: "deny" },
    });
    expect(errorOf(answer).code).toBe("not_found");
    expect(errorOf(answer).message).toContain("list_pending_requests");
  });

  test("an answer of the wrong shape for the request is refused", async () => {
    const { parent, child } = await staged();
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 6)] }));
    const approval = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "6", answer: "yes" },
    });
    expect(errorOf(approval).code).toBe("invalid");
    expect(errorOf(approval).message).toContain("approval");
    // A question is answered by words or one of its options, never an approval's choice.
    const question = ask(child, 7, {
      title: "Which database?",
      question: {
        header: "Database",
        text: "Which one should the migration use?",
        descriptions: ["Durable", "Fast"],
        multiple: false,
      },
      options: [
        { id: "pg", name: "Postgres", kind: "allow_once" as const },
        { id: "sqlite", name: "SQLite", kind: "allow_once" as const },
      ],
    });
    updateThread(child, (t) => ({ ...t, permissions: [question] }));
    const withChoice = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "7", choice: "deny" },
    });
    expect(errorOf(withChoice).code).toBe("invalid");
    expect(errorOf(withChoice).message).toContain("question");
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(1);
  });

  test("answers the child's card, through the same store call the card uses", async () => {
    const { parent, child } = await staged();
    useApp.setState({ permissionModes: { ...useApp.getState().permissionModes, [parent]: "full" } });
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 12)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "12", choice: "allow_once" },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer).toMatchObject({
      session_id: child,
      request_id: "12",
      kind: "approval",
      choice: "allow_once",
      option: "Allow once",
    });
    // The card left the child's queue, as if the user had clicked it.
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(0);
  });

  test("list_pending_requests names the id an answer takes", async () => {
    const { parent, child } = await staged();
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 11)] }));
    const listed = await answerOrchestration({
      sessionId: parent,
      tool: "list_pending_requests",
      args: {},
    });
    const requests = listed.requests as JsonObject[];
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({
      session_id: child,
      request_id: "11",
      kind: "approval",
      asks: "Run: cargo test",
    });
  });
});

describe("organize_threads", () => {
  test("changes several threads once, and says what it skipped", async () => {
    const { parent, child } = await staged();
    // Full access answers the management card itself, so the batch reaches the store.
    useApp.setState({ permissionModes: { ...useApp.getState().permissionModes, [parent]: "full" } });
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [parent, child, "no-such-thread"], title: "Same work", unread: true },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer.changed).toBe(2);
    const threads = answer.threads as JsonObject[];
    expect(threads.map((t) => t.session_id).sort()).toEqual([parent, child].sort());
    for (const entry of threads) expect(entry.title).toBe("Same work");
    const skipped = answer.skipped as JsonObject[];
    expect(skipped).toHaveLength(1);
    expect(skipped[0]?.session_id).toBe("no-such-thread");
    expect(useApp.getState().threads[child]?.title).toBe("Same work");
    // `archived` is the settled status, the one update_session's `settled` sets.
    const settled = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [child], archived: true },
    });
    expect(errorOf(settled)).toEqual({});
    expect((settled.threads as JsonObject[])[0]).toMatchObject({ session_id: child, archived: true });
  });

  test("refuses a call that changes nothing, and one with no manageable thread", async () => {
    const { parent } = await staged();
    const nothing = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [parent] },
    });
    expect(errorOf(nothing).code).toBe("invalid");
    const unknown = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: ["no-such-thread"], pinned: true },
    });
    expect(errorOf(unknown).code).toBe("not_allowed");
  });
});

describe("message_session attachments", () => {
  test("a file outside the caller's project is refused before anything is sent", async () => {
    const { parent, child } = await staged();
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "message_session",
      args: { session_id: child, message: "read it", attachments: ["/etc/passwd"] },
    });
    expect(errorOf(answer).code).toBe("not_allowed");
    expect(errorOf(answer).message).toContain("outside this thread's project");
    expect(useApp.getState().links.links).toHaveLength(1);
  });

  test("a relative path is refused too", async () => {
    const { parent, child } = await staged();
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "message_session",
      args: { session_id: child, message: "read it", attachments: ["docs/notes.md"] },
    });
    expect(errorOf(answer).code).toBe("invalid");
    expect(errorOf(answer).message).toContain("not an absolute path");
  });
});
