// The v10 tools: answering what a thread waits on (`answer_request`) and
// organizing several at once (`organize_threads`), plus the files
// `message_session` carries. The rules they share with `update_session` —
// the caller's own delegation tree, the project, the user's approval — are
// checked through the same `_rustcode/orchestration` entry the app answers,
// and so is what leaves the app: every answer here is read off the transport,
// because a card that leaves the queue without a reply reaching the engine is
// exactly the failure these tools must not have.

import { beforeEach, describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import { permissionSelected, questionTextAnswer } from "../acp/types";
import { answerOrchestration, resolveGrant } from "./host";
import { emptyBook, type Link } from "./orchestration";
import {
  connect,
  markFinished,
  markUnread,
  openThread,
  renameThread,
  resetForTests,
  selectThread,
  setOrchestration,
  statusOf,
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
  const transport = new MockTransport({ delayMs: 0 });
  // Every line the app sends, so an answer can be read where the engine reads it.
  const sent: JsonObject[] = [];
  const send = transport.send.bind(transport);
  transport.send = (line: string) => {
    try {
      const parsed = JSON.parse(line) as JsonObject;
      if (parsed && typeof parsed === "object") sent.push(parsed);
    } catch {
      // A line the engine would refuse is no answer to anything.
    }
    send(line);
  };
  await connect(transport);
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
  return { parent, child, link, sent };
}

/** The thread answers for itself: no card, as a person's own approval would. */
const fullAccess = (id: string) =>
  useApp.setState({ permissionModes: { ...useApp.getState().permissionModes, [id]: "full" } });

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

const question = (thread: string, rpcId: number) =>
  ask(thread, rpcId, {
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
      { id: "reject", name: "Decline", kind: "reject_once" as const },
    ],
  });

const errorOf = (answer: JsonObject) =>
  (answer.error as { code?: string; message?: string } | undefined) ?? {};

/** The results the app sent for one waiting request, as the engine's `respond` writes them. */
const responseTo = (sent: JsonObject[], rpcId: number) =>
  sent.filter((line) => line.id === rpcId && line.result !== undefined).map((line) => line.result);

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
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 6)] }));
    const approval = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "6", answer: "yes" },
    });
    expect(errorOf(approval).code).toBe("invalid");
    expect(errorOf(approval).message).toContain("approval");
    // A question takes words or one of its options, not an approval's choice.
    updateThread(child, (t) => ({ ...t, permissions: [question(child, 7)] }));
    const withChoice = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "7", choice: "allow_once" },
    });
    expect(errorOf(withChoice).code).toBe("invalid");
    expect(errorOf(withChoice).message).toContain("question");
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(1);
    expect(responseTo(sent, 7)).toEqual([]);
  });

  test("an approval's answer reaches the engine as the option its card would send", async () => {
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 101)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "101", choice: "allow_once" },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer).toMatchObject({
      session_id: child,
      request_id: "101",
      kind: "approval",
      choice: "allow_once",
      option: "Allow once",
    });
    // The child's own request, answered with the option the user's click sends.
    expect(responseTo(sent, 101)).toEqual([permissionSelected("allow_once")]);
    // And the card left the child's queue, as if the user had clicked it.
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(0);
  });

  test("a scope the request does not offer is refused, never swapped for another", async () => {
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    // A destructive command ships "Allow once" and "Reject" and nothing that stands.
    const once = ask(child, 102, {
      title: "Run: rm -rf build",
      options: [
        { id: "allow_once", name: "Allow once", kind: "allow_once" as const },
        { id: "reject", name: "Decline", kind: "reject_once" as const },
      ],
    });
    updateThread(child, (t) => ({ ...t, permissions: [once] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "102", choice: "allow_always" },
    });
    expect(errorOf(answer).code).toBe("invalid");
    expect(errorOf(answer).message).toContain("allow once, decline");
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(1);
    expect(responseTo(sent, 102)).toEqual([]);
  });

  test("a question answered with an option sends that option", async () => {
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    updateThread(child, (t) => ({ ...t, permissions: [question(child, 103)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "103", answer: "sqlite" },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer).toMatchObject({ kind: "question", answer: "sqlite", option: "SQLite" });
    expect(responseTo(sent, 103)).toEqual([permissionSelected("sqlite")]);
  });

  test("a question answered in words sends them on the card's carrier option", async () => {
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    updateThread(child, (t) => ({ ...t, permissions: [question(child, 104)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "104", answer: "  either, really " },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer).toMatchObject({ kind: "question", answer: "either, really" });
    expect(responseTo(sent, 104)).toEqual([
      questionTextAnswer("either, really", { id: "pg", name: "Postgres", kind: "allow_once" }),
    ]);
  });

  test("a question's own Decline answers it, as the card's button does", async () => {
    const { parent, child, sent } = await staged();
    fullAccess(parent);
    updateThread(child, (t) => ({ ...t, permissions: [question(child, 105)] }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "105", choice: "deny" },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer).toMatchObject({ kind: "question", choice: "deny", option: "Decline" });
    expect(responseTo(sent, 105)).toEqual([permissionSelected("reject")]);
  });

  test("the user is asked first: a deny leaves the card and sends nothing", async () => {
    const { parent, child, sent } = await staged();
    // The caller is on the default access, so answering is the user's to allow.
    expect(useApp.getState().permissionModes[parent]).toBeUndefined();
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 106)] }));
    const pending = answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "106", choice: "allow_once" },
    });
    await until(() => !!useApp.getState().grant);
    const card = useApp.getState().grant!;
    expect(card.kind).toBe("answer");
    expect(card.target).toBe(useApp.getState().threads[child]?.title ?? "");
    expect(card.detail).toContain("allow it once");
    expect(card.detail).toContain("Run: cargo test");
    resolveGrant("deny");
    const answer = await pending;
    expect(errorOf(answer).code).toBe("denied");
    expect(useApp.getState().threads[child]?.permissions).toHaveLength(1);
    expect(responseTo(sent, 106)).toEqual([]);
  });

  test("a grant covers that request, not the next one of the thread", async () => {
    const { parent, child, sent } = await staged();
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 107)] }));
    const first = answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "107", choice: "allow_once" },
    });
    await until(() => !!useApp.getState().grant);
    resolveGrant("always");
    expect(errorOf(await first)).toEqual({});
    expect(responseTo(sent, 107)).toHaveLength(1);
    // Another command of the same thread is another request: it asks again.
    updateThread(child, (t) => ({ ...t, permissions: [ask(child, 108, { title: "Run: rm -rf build" })] }));
    const second = answerOrchestration({
      sessionId: parent,
      tool: "answer_request",
      args: { session_id: child, request_id: "108", choice: "allow_once" },
    });
    await until(() => !!useApp.getState().grant);
    expect(useApp.getState().grant?.detail).toContain("rm -rf build");
    resolveGrant("deny");
    expect(errorOf(await second).code).toBe("denied");
    expect(responseTo(sent, 108)).toEqual([]);
  });

  test("list_pending_requests names the id an answer takes, oldest first", async () => {
    const { parent, child } = await staged();
    updateThread(child, (t) => ({
      ...t,
      permissions: [ask(child, 111, { at: 5_000 }), ask(child, 112, { at: 1_000 })],
    }));
    const listed = await answerOrchestration({
      sessionId: parent,
      tool: "list_pending_requests",
      args: {},
    });
    const requests = listed.requests as JsonObject[];
    expect(requests).toHaveLength(2);
    // The card that has waited longest comes first, as the doc says.
    expect(requests.map((r) => r.request_id)).toEqual(["112", "111"]);
    expect(requests[1]).toMatchObject({
      session_id: child,
      request_id: "111",
      kind: "approval",
      asks: "Run: cargo test",
    });
    expect(requests[1]?.options).toEqual([
      { id: "allow_once", name: "Allow once" },
      { id: "allow_always", name: "Always allow" },
      { id: "reject", name: "Decline" },
    ]);
  });
});

describe("update_session settles a thread (issue 3529)", () => {
  test("a thread settled in the middle of its turn reports settled, and is settled once the turn ends", async () => {
    const { parent } = await staged();
    updateThread(parent, (t) => ({ ...t, running: true }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "update_session",
      args: { status: "settled" },
    });
    expect(errorOf(answer)).toEqual({});
    // It says what it set, not the "running" the turn still reads; `running` says the turn goes on.
    expect(answer.status).toBe("done");
    expect(answer.running).toBe(true);
    expect(useApp.getState().sessions.find((x) => x.id === parent)?.status).toBe("done");
    // The turn ends unseen (the user is elsewhere): the thread is settled, not "just finished".
    updateThread(parent, (t) => ({ ...t, running: false }));
    markFinished(parent);
    expect(statusOf(useApp.getState(), parent)).toBe("done");
    // Settling an unread thread reads it: no unseen finish keeps it in the active list.
    const again = await answerOrchestration({
      sessionId: parent,
      tool: "update_session",
      args: { status: "active" },
    });
    expect(again.status).toBe("idle");
    markFinished(parent);
    expect(parent in useApp.getState().finished).toBe(true);
    const settled = await answerOrchestration({
      sessionId: parent,
      tool: "update_session",
      args: { status: "settled" },
    });
    expect(settled.status).toBe("done");
    expect(parent in useApp.getState().finished).toBe(false);
    expect(statusOf(useApp.getState(), parent)).toBe("done");
  });
});

describe("an agent names its own thread (issue 3599)", () => {
  test("an automatic title takes the agent's name; one the user typed stays", async () => {
    const { parent } = await staged();
    const named = await answerOrchestration({
      sessionId: parent,
      tool: "update_session",
      args: { title: "Fix the login redirect" },
    });
    expect(errorOf(named)).toEqual({});
    expect(named.title).toBe("Fix the login redirect");
    expect(named.title_kept).toBeUndefined();
    expect(useApp.getState().sessions.find((x) => x.id === parent)?.title).toBe("Fix the login redirect");

    // The user renames it from the sidebar: the agent's next name leaves it as it is, and says so.
    expect(await renameThread(parent, "My own name", { user: true })).toBe(true);
    const kept = await answerOrchestration({
      sessionId: parent,
      tool: "update_session",
      args: { title: "Something else", pinned: true },
    });
    expect(errorOf(kept)).toEqual({});
    expect(kept.title).toBe("My own name");
    expect(String(kept.title_kept)).toContain("user named this thread");
    // The rest of the call still applies.
    expect(kept.pinned).toBe(true);
    expect(useApp.getState().sessions.find((x) => x.id === parent)?.title).toBe("My own name");
  });
});

describe("organize_threads", () => {
  test("changes several threads once, and says what it skipped", async () => {
    const { parent, child } = await staged();
    // Full access answers the management card itself, so the batch reaches the store.
    fullAccess(parent);
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

  test("unread false marks a thread read, and only real changes are counted", async () => {
    const { parent, child } = await staged();
    fullAccess(parent);
    markUnread(child);
    expect(useApp.getState().finished[child]).toBeDefined();
    const read = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [child], unread: false },
    });
    expect(errorOf(read)).toEqual({});
    expect(read.changed).toBe(1);
    expect(useApp.getState().finished[child]).toBeUndefined();
    // Already read: nothing to change, so nothing is claimed...
    const again = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [child], unread: false },
    });
    expect(again.changed).toBe(0);
    expect(again.threads).toEqual([]);
    // ...and no card is raised for it.
    expect(useApp.getState().grant).toBeNull();
    // The same for a pin the thread already has.
    const pin = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [child], pinned: true },
    });
    expect(pin.changed).toBe(1);
    const pinned = await answerOrchestration({
      sessionId: parent,
      tool: "organize_threads",
      args: { session_ids: [child], pinned: true },
    });
    expect(pinned.changed).toBe(0);
    expect(useApp.getState().pinned).toContain(child);
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

  test("the files a message carries reach the prompt it queues", async () => {
    const { parent, child } = await staged();
    fullAccess(parent);
    // A busy thread takes the message into its queue: the files have to go with it.
    updateThread(child, (t) => ({ ...t, running: true }));
    const file = "/Users/you/code/rustcode/docs/orchestrator.md";
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "message_session",
      args: { session_id: child, message: "read it", attachments: [file] },
    });
    expect(errorOf(answer)).toEqual({});
    expect(answer.receipt).toBe("queued");
    expect(answer.attachments).toEqual([file]);
    const queued = useApp.getState().threads[child]?.queue ?? [];
    expect(queued).toHaveLength(1);
    expect(queued[0]?.extras?.attachments).toMatchObject([
      { kind: "file", name: "orchestrator.md", path: file },
    ]);
  });
});
