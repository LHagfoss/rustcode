// Store-level tests for auto-compaction's scheduling side effects. The rules
// themselves are pure (`autoCompact.test.ts`); these drive the store and the
// mock engine, so a guard or a quiet turn is tested where it runs.

import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import type { SessionSummary, SessionUsage } from "../acp/types";
import { emptyExtras } from "./compose";
import { foldTurns } from "./fold";
import {
  cancel,
  clearFinished,
  connect,
  markUnread,
  resetForTests,
  runPrompt,
  scheduleAutoCompact,
  selectThread,
  setAutoCompactForThread,
  setAutoCompactMode,
  setAutoCompactThreshold,
  snoozeThread,
  trashThread,
  updateThread,
  useApp,
} from "./store";
import type { TranscriptItem } from "./thread";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

const settle = (ms = 60) => new Promise((r) => setTimeout(r, ms));

const usage = (used: number): SessionUsage => ({ used, size: 272_000, estimated: false });

let turn = 0;

/** A fact for the thread: its last turn ended at `at`, with `used` in context. */
function endedTurn(id: string, at: number, used: number) {
  updateThread(id, (t) => {
    const end: TranscriptItem = {
      kind: "turnEnd",
      id: `test-turn-${++turn}`,
      at,
      outcome: "completed",
    };
    return { ...t, usage: usage(used), waiting: undefined, items: [...t.items, end] };
  });
}

const compacted = (id: string) =>
  useApp
    .getState()
    .threads[id]?.items.some(
      (item) => item.kind === "notice" && item.text.startsWith("Compacted automatically"),
    ) ?? false;

const compactPrompt = (id: string) =>
  useApp
    .getState()
    .threads[id]?.items.some((item) => item.kind === "user" && item.text.trim() === "/compact") ?? false;

/** An open thread on Claude Code's two-rule policy, its size threshold at 20k. */
async function openClaudeThread() {
  await connect(new MockTransport({ delayMs: 0 }));
  await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
  const id = useApp.getState().sessions[0]!.id;
  selectThread(id);
  await until(() => useApp.getState().threads[id]?.loaded === true);
  const capabilities = useApp
    .getState()
    .harnesses!.harnesses.find((harness) => harness.id === "claude-code")!.capabilities;
  updateThread(id, (t) => ({ ...t, harness: { id: "claude-code", capabilities } }));
  setAutoCompactMode("size");
  setAutoCompactThreshold(20_000);
  return id;
}

beforeEach(() => resetForTests());

describe("auto-compaction in the store", () => {
  test("a stopped or failed turn does not start one", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);

    await runPrompt(id, "fail this turn");
    await settle(80);
    expect(compacted(id)).toBe(false);
    expect(compactPrompt(id)).toBe(false);

    const running = runPrompt(id, "keep working");
    await until(() => useApp.getState().threads[id]?.running === true);
    await cancel(id);
    await running;
    await settle(80);
    expect(compacted(id)).toBe(false);
    expect(compactPrompt(id)).toBe(false);
  });

  test("a pending limit resume is a guard and survives an automatic compaction", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    const wait = { until: Date.now() + 60 * 60_000, harness: "claude-code", account: null, attempts: 1 };
    useApp.setState({ limitWaits: { [id]: wait } });

    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);

    await runPrompt(id, "/compact", emptyExtras, { automatic: true, autoCompact: true });
    expect(useApp.getState().limitWaits[id]).toEqual(wait);
  });

  test("a snoozed thread is a guard and stays snoozed through an automatic compaction", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    snoozeThread(id, Date.now() + 60 * 60_000);

    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);
    expect(id in useApp.getState().snoozed).toBe(true);

    await runPrompt(id, "/compact", emptyExtras, { automatic: true, autoCompact: true });
    expect(id in useApp.getState().snoozed).toBe(true);
  });

  test("rule 2 fires only inside its window after the idle deadline", async () => {
    const id = await openClaudeThread();
    setAutoCompactMode("cache");
    setAutoCompactThreshold(150_000);
    const now = Date.now();

    // The deadline passed more than a minute ago: a restart or a wake from
    // sleep must not compact with a cold cache.
    endedTurn(id, now - 4.5 * 60_000 - 2 * 60_000, 80_000);
    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);

    endedTurn(id, Date.now() - 4.5 * 60_000 - 10_000, 80_000);
    scheduleAutoCompact(id);
    await until(() => compacted(id));
    expect(compactPrompt(id)).toBe(false);
  });

  test("a background task is a guard", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    updateThread(id, (t) => ({ ...t, waiting: 1 }));

    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);

    updateThread(id, (t) => ({ ...t, waiting: undefined }));
    scheduleAutoCompact(id);
    await until(() => compacted(id));
    expect(compactPrompt(id)).toBe(false);
  });

  test("a delegate is compacted only while its parent is still talking to it", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    const parent = (updatedAt: number): SessionSummary => ({
      id: "parent",
      cwd: "/tmp/project",
      title: "Parent",
      updatedAt,
      harness: "claude-code",
    });
    useApp.setState((s) => ({
      sessions: [
        ...s.sessions.map((x) => (x.id === id ? { ...x, parent: { id: "parent" } } : x)),
        parent(Date.now() - 20 * 60_000),
      ],
    }));

    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);

    useApp.setState((s) => ({
      sessions: s.sessions.map((x) => (x.id === "parent" ? { ...x, updatedAt: Date.now() } : x)),
    }));
    scheduleAutoCompact(id);
    await until(() => compacted(id));
  });

  test("a thread whose last answer is unread is left alone", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    // Done until the user opens it: the answer is not read yet.
    markUnread(id);

    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);

    // Read: the size rule may run.
    clearFinished(id);
    scheduleAutoCompact(id);
    await until(() => compacted(id));
  });

  test("the per-thread off switch and a trashed thread leave no scheduler state", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    setAutoCompactForThread(id, false);
    scheduleAutoCompact(id);
    await settle(80);
    expect(compacted(id)).toBe(false);
    expect(useApp.getState().autoCompactOff[id]).toBe(true);

    // Trash through the store: its `thread_trash` call is the mock's own.
    expect(await trashThread(id)).toBe(true);
    expect(useApp.getState().autoCompactOff[id]).toBeUndefined();
    // A second schedule finds no thread and starts nothing.
    scheduleAutoCompact(id);
    await settle(30);
  });
});

describe("quiet automatic compaction", () => {
  test("it leaves no user bubble and no turn footer", async () => {
    const id = await openClaudeThread();
    endedTurn(id, Date.now(), 30_000);
    const before = useApp.getState().threads[id]!.items.length;
    scheduleAutoCompact(id);
    await until(() => compacted(id));
    const after = useApp.getState().threads[id]!;
    expect(compactPrompt(id)).toBe(false);
    expect(after.items.slice(before).some((item) => item.kind === "user")).toBe(false);
    expect(after.items.slice(before).some((item) => item.kind === "turnEnd")).toBe(false);
    expect(after.running).toBe(false);
  });

  test("the answer above it stays the last answer, not folded away", async () => {
    const id = await openClaudeThread();
    await runPrompt(id, "yolo, what shipped?");
    const answer = useApp
      .getState()
      .threads[id]!.items.findLast((item) => item.kind === "assistant" && !!item.text.trim());
    expect(answer).toBeDefined();
    // The user has read it: Done is cleared, and the size rule may run.
    clearFinished(id);
    updateThread(id, (t) => ({ ...t, usage: usage(30_000) }));
    scheduleAutoCompact(id);
    await until(() => compacted(id));

    const thread = useApp.getState().threads[id]!;
    const index = thread.items.findIndex((item) => item.id === answer!.id);
    expect(index).toBeGreaterThanOrEqual(0);
    // Nothing the compaction did became a row: the answer, its footer, then the notice.
    const after = thread.items.slice(index + 1).map((item) => item.kind);
    expect(after.filter((kind) => kind === "user" || kind === "tool")).toEqual([]);
    expect(after.at(-1)).toBe("notice");
    // And the fold does not hide it under the turn's `Worked for …`.
    const rows = foldTurns(thread.items, false, new Set());
    expect(rows.some((row) => "item" in row && row.item.id === answer!.id)).toBe(true);
    // The read answer was not flagged again by the compaction.
    expect(id in useApp.getState().finished).toBe(false);
  });
});
