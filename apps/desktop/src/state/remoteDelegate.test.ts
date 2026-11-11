// A delegate on a paired machine, from the message that starts it to the turn
// that ends it: the machine's own link is what the app reads, and what it
// settles the link with (issues 3523, 3524, 3525).
//
// The machine is a stand-in for the shell's supervisor and that machine's
// engine: the frames `state/store.ts` `handleInbound` reads, and the answers
// `state/machines.ts` `readThreads` reads.

import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { answerOrchestration } from "./host";
import { resetMachinesForTests, useMachines } from "./machines";
import { emptyBook, type Link } from "./orchestration";
import {
  createSession,
  resetForTests,
  runPrompt,
  startMachineRouting,
  useApp,
  watchMachineConnection,
} from "./store";
import { emptyThread } from "./thread";

const WINDOWS = "gw-3523-windows";
const PARENT = "parent";
const CWD = "C:\\Users\\LG\\code\\rustcode";
const LINK = `${PARENT}/1`;
const ANSWER = "| Component | Specs |\n|---|---|\n| OS | Windows 11 |";

/** What the machine's engine does with its next turn. */
interface MachineState {
  /** Its own thread list: a snapshot from before the thread the app just made is a stale one. */
  listed: { id: string; cwd: string; title: string }[];
  /** What the turn that runs answers, streamed as the harness streams it. */
  answer?: string;
  /** What the turn that runs fails with, as the engine reports it. */
  failure?: string;
  /** What the machine's data store says about a thread, or nothing for an engine without one. */
  activity?: JsonObject | null;
}

/** The machine's engine: a thread list, a turn, and the activity its store holds. */
function machineEngine(state: MachineState): AcpConnection {
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.method !== "string" || typeof frame.id !== "number") return;
    const id = frame.id;
    const params = (frame.params ?? {}) as JsonObject;
    const reply = (result: JsonObject) => conn.receive(JSON.stringify({ jsonrpc: "2.0", id, result }));
    const fail = (message: string) =>
      conn.receive(JSON.stringify({ jsonrpc: "2.0", id, error: { code: -32000, message } }));
    queueMicrotask(() => {
      if (conn.isClosed) return;
      if (frame.method === "session/list") {
        return reply({
          sessions: state.listed.map((thread) => ({
            sessionId: thread.id,
            cwd: thread.cwd,
            title: thread.title,
            updatedAt: new Date().toISOString(),
          })),
        });
      }
      if (frame.method === "session/new") {
        return reply({ sessionId: "win-1", configOptions: [] });
      }
      if (frame.method === Method.sessionPrompt) {
        if (state.answer) {
          conn.receive(
            JSON.stringify({
              jsonrpc: "2.0",
              method: "session/update",
              params: {
                sessionId: String(params.sessionId),
                update: {
                  sessionUpdate: "agent_message_chunk",
                  content: { type: "text", text: state.answer },
                },
              },
            }),
          );
        }
        if (state.failure) return fail(state.failure);
        return reply({ stopReason: "end_turn" });
      }
      if (frame.method === Method.dataActivity) {
        if (state.activity === null) return fail("unknown method");
        return reply(state.activity ?? { items: [], last_provider_error: null });
      }
      return reply({ configOptions: [], sessions: [] });
    });
  });
  return conn;
}

/** The machine's connection with this window, and its thread list as the app last read it. */
function attach(state: MachineState): void {
  const conn = machineEngine(state);
  useMachines.setState((s) => ({ sessions: { ...s.sessions, [WINDOWS]: [] } }));
  watchMachineConnection(WINDOWS, conn);
}

/** The machine's own list as it answers it now, which is what the app's mirror reads. */
function listed(state: MachineState): void {
  useMachines.setState((s) => ({
    sessions: {
      ...s.sessions,
      [WINDOWS]: state.listed.map((thread) => ({
        id: thread.id,
        cwd: thread.cwd,
        title: thread.title,
        updatedAt: Date.now(),
      })),
    },
  }));
}

/** The thread the app just made on the machine, and the parent's link that waits on it. */
async function started(): Promise<string> {
  const thread = await createSession({
    host: WINDOWS,
    cwd: CWD,
    harness: "codex",
    title: "Windows PC specs",
    parent: PARENT,
  });
  const link: Link = {
    id: LINK,
    orchestrator: PARENT,
    target: thread,
    orchestratorTitle: "Report the specs",
    targetTitle: "Windows PC specs",
    message: "Report this machine's specs",
    state: "working",
    started: true,
    machine: "Windows",
    createdAt: Date.now(),
    startedAt: Date.now(),
  };
  useApp.setState((s) => ({
    links: { ...emptyBook(), links: [link] },
    threads: {
      ...s.threads,
      [PARENT]: { ...emptyThread(PARENT, "/Users/lagos/code/rustcode", "Report the specs"), loaded: true },
    },
  }));
  return thread;
}

const link = () => useApp.getState().links.links.find((l) => l.id === LINK);

beforeEach(async () => {
  watchMachineConnection(WINDOWS, null);
  resetForTests();
  useMachines.setState({ sessions: {} });
  // The mirror that puts a machine's own list into the app's (state/store.ts `mergeMachineLists`).
  await startMachineRouting();
});

afterAll(() => {
  watchMachineConnection(WINDOWS, null);
  resetForTests();
  resetMachinesForTests();
});

describe("a delegate on a paired machine", () => {
  test("keeps the thread its machine has not listed yet, and settles done with its answer", async () => {
    const state: MachineState = { listed: [], answer: ANSWER };
    attach(state);
    const thread = await started();
    // The machine's list, read now, is from before the thread it just made: it does not have it.
    listed(state);
    expect(useApp.getState().sessions.some((x) => x.id === thread)).toBe(true);

    // The turn ends there, over the machine's own link.
    await runPrompt(thread, "Report this machine's specs", undefined, { link: LINK });
    expect(link()?.state).toBe("done");
    expect(link()?.answer).toBe(ANSWER);
    expect(link()?.error).toBeUndefined();
  });

  test("a turn that failed carries its reason: the chip, its events, and wait_session", async () => {
    const state: MachineState = { listed: [], answer: ANSWER, failure: "Codex: the context is full" };
    attach(state);
    const thread = await started();

    await runPrompt(thread, "Report this machine's specs", undefined, { link: LINK });
    const failed = link();
    expect(failed?.state).toBe("failed");
    expect(failed?.error).toContain("the context is full");
    // The hover card lists the reason beside the event, not only the word "Failed".
    expect(failed?.events?.at(-1)).toMatchObject({ kind: "failed" });
    expect(failed?.events?.at(-1)?.detail).toContain("the context is full");

    // The parent reads the same reason from the wait, not only the answer.
    const answered = await answerOrchestration({
      sessionId: PARENT,
      tool: "wait_session",
      args: { link_ids: [LINK], timeout_ms: 1 },
    });
    const [entry] = (answered.links ?? []) as JsonObject[];
    expect(entry?.status).toBe("failed");
    expect(String(entry?.error)).toContain("the context is full");
    expect(String(entry?.answer)).toContain("Component");
  });

  test("a link whose thread is only missing from the list is not failed as gone", async () => {
    const state: MachineState = { listed: [] };
    attach(state);
    const thread = await started();
    // The app holds the thread's own state but no list has it (a machine's list that
    // lags behind the session it just made). That is unlisted, not a deleted thread.
    useApp.setState((s) => ({
      sessions: s.sessions.filter((x) => x.id !== thread),
      threads: { ...s.threads, [PARENT]: { ...s.threads[PARENT]!, running: true, loaded: true } },
    }));

    await answerOrchestration({
      sessionId: PARENT,
      tool: "wait_session",
      args: { link_ids: [LINK], timeout_ms: 1 },
    });
    expect(link()?.state).toBe("working");
    expect(link()?.error).toBeUndefined();
  });

  test("a link whose thread no list names and nothing keeps is failed as gone", async () => {
    // A link this window did not just make (one from before a restart): the app has a copy of the
    // thread's own state, but no list names it and the keep-unlisted set does not hold it. The
    // copy was what the old check looked at, so a gone link could never fail.
    const at = Date.now();
    const target = `${WINDOWS}/gone`;
    useApp.setState((s) => ({
      links: {
        ...emptyBook(),
        links: [
          {
            id: LINK,
            orchestrator: PARENT,
            target,
            orchestratorTitle: "Report the specs",
            targetTitle: "Gone",
            message: "Report this machine's specs",
            state: "working",
            started: true,
            machine: "Windows",
            createdAt: at,
            startedAt: at,
          },
        ],
      },
      threads: {
        ...s.threads,
        [PARENT]: {
          ...emptyThread(PARENT, "/Users/lagos/code/rustcode", "Report the specs"),
          running: true,
          loaded: true,
        },
        [target]: emptyThread(target, CWD, "Gone"),
      },
    }));

    await answerOrchestration({
      sessionId: PARENT,
      tool: "wait_session",
      args: { link_ids: [LINK], timeout_ms: 1 },
    });
    expect(link()?.state).toBe("failed");
    expect(link()?.error).toBe("That thread is gone.");
  });

  test("read_activity is the machine's own, or says it has none to give", async () => {
    const activity: JsonObject = {
      items: [{ kind: "turn.summary", at: "2026-11-11T20:35:00Z" }],
      last_provider_error: { at: "2026-11-11T20:35:00Z", status_code: 429, error_kind: "rate_limit" },
    };
    const state: MachineState = { listed: [], activity };
    attach(state);
    const thread = await started();
    useApp.setState((s) => ({
      threads: {
        ...s.threads,
        [PARENT]: { ...s.threads[PARENT]!, running: true, loaded: true },
      },
    }));

    const read = await answerOrchestration({
      sessionId: PARENT,
      tool: "read_activity",
      args: { session_id: thread, limit: 5, order: "newest" },
    });
    expect(read.error).toBeUndefined();
    expect(read.last_provider_error).toEqual(activity.last_provider_error);
    expect((read.items as JsonObject[])[0]?.kind).toBe("turn.summary");

    // An older machine has no store to ask: the answer says so instead of an empty list.
    state.activity = null;
    const older = await answerOrchestration({
      sessionId: PARENT,
      tool: "read_activity",
      args: { session_id: thread, limit: 5 },
    });
    expect(older.items).toBeUndefined();
    expect(String((older.error as JsonObject)?.message)).toContain("not available from");
  });
});
