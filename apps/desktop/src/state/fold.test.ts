import { describe, expect, test } from "bun:test";
import { foldLabel, foldTurns, groupTools, inferToolKind, type TurnFold, toolGroupLabel } from "./fold";
import type { TranscriptItem } from "./thread";

const user = (id: string): TranscriptItem => ({ kind: "user", id, text: id });
const reply = (id: string): TranscriptItem => ({ kind: "assistant", id, text: id });
const thought = (id: string): TranscriptItem => ({
  kind: "thought",
  id,
  text: id,
  summary: false,
  startedAt: 0,
  done: true,
});
const tool = (id: string, status: "completed" | "failed" = "completed", path?: string): TranscriptItem => ({
  kind: "tool",
  id,
  title: id,
  status,
  diffs: path ? [{ path, oldText: "a", newText: "b" } as never] : [],
});
/** A fan-out over `harnesses`, as the call's input holds it. */
const fanOut = (id: string, harnesses: string[]): TranscriptItem => ({
  kind: "tool",
  id,
  title: "mcp__rustcode__fan_out",
  status: "completed",
  input: { harnesses: harnesses.map((harness) => ({ harness })) },
  diffs: [],
});
const end = (id: string, elapsedMs = 80_000): TranscriptItem => ({
  kind: "turnEnd",
  id,
  elapsedMs,
  outcome: "completed",
});
const error = (id: string): TranscriptItem => ({ kind: "notice", id, text: id, tone: "error" });

const keys = (rows: ReturnType<typeof foldTurns>) =>
  rows.map((r) => ("fold" in r ? `[${r.fold.key}]` : r.key));

describe("turn folding", () => {
  const items = [
    user("u1"),
    thought("t1"),
    reply("r1"), // between steps: folds
    tool("c1", "completed", "src/a.ts"),
    tool("c2", "failed"),
    tool("c3", "completed", "src/a.ts"),
    reply("r2"), // the final answer
    end("e1"),
    user("u2"),
    thought("t2"),
    tool("c4"),
    reply("r3"),
  ];

  test("a finished turn folds its work above the final answer", () => {
    const rows = foldTurns(items, true, new Set());
    expect(keys(rows).slice(0, 4)).toEqual(["u1", "[fold-e1]", "r2", "e1"]);
    const fold = rows.find((r) => "fold" in r)!;
    expect("fold" in fold && fold.fold).toMatchObject({
      steps: 5,
      tools: 3,
      failed: 1,
      files: 1,
      open: false,
    });
  });

  test("a short running turn shows every step", () => {
    const rows = foldTurns(items, true, new Set());
    expect(keys(rows).slice(4)).toEqual(["u2", "t2", "c4", "r3"]);
  });

  test("once it is not running, the last turn folds too (a replayed thread)", () => {
    const rows = foldTurns(items, false, new Set());
    expect(keys(rows).slice(4)).toEqual(["u2", "[fold-t2]", "r3"]);
  });

  test("an opened turn shows its steps in order after the summary", () => {
    const rows = foldTurns(items, true, new Set(["fold-e1"]));
    expect(keys(rows).slice(0, 9)).toEqual(["u1", "[fold-e1]", "t1", "r1", "c1", "c2", "c3", "r2", "e1"]);
    expect(rows.filter((r) => "inFold" in r && r.inFold === "fold-e1").length).toBe(5);
  });

  test("errors and warnings stay visible; a turn with no work does not fold", () => {
    const rows = foldTurns(
      [user("u1"), tool("c1"), error("x1"), reply("r1"), end("e1"), user("u2"), reply("r2"), end("e2")],
      false,
      new Set(),
    );
    expect(keys(rows)).toEqual(["u1", "[fold-e1]", "x1", "r1", "e1", "u2", "r2", "e2"]);
  });

  test("the divider reads how long it worked, or that you stopped it", () => {
    const seconds = (ms: number) => `${Math.round(ms / 1000)}s`;
    const rows = foldTurns(items, true, new Set());
    const fold = rows.find((r) => "fold" in r);
    expect(fold && "fold" in fold ? foldLabel(fold.fold, seconds) : "").toBe("Worked for 80s");
    const stopped = foldTurns(
      [
        user("u"),
        tool("c"),
        reply("r"),
        { kind: "turnEnd", id: "e", elapsedMs: 12_000, outcome: "cancelled" },
      ],
      false,
      new Set(),
    ).find((r) => "fold" in r);
    expect(stopped && "fold" in stopped ? foldLabel(stopped.fold, seconds) : "").toBe(
      "You stopped after 12s",
    );
  });

  test("a failed step marks itself, never the header of a turn that finished", () => {
    const fold = (rows: ReturnType<typeof foldTurns>) => rows.find((r) => "fold" in r) as { fold: TurnFold };
    // Two calls failed inside a turn that then answered: the header stays clean, so a reader
    // cannot take the turn for a failure.
    expect(fold(foldTurns(items, false, new Set())).fold).toMatchObject({
      failed: 1,
      failedTurn: false,
      stopped: false,
    });
    const failedTurn = [
      user("u"),
      tool("c"),
      { kind: "turnEnd", id: "e", elapsedMs: 3_000, at: 9, outcome: "failed" },
    ] as TranscriptItem[];
    expect(fold(foldTurns(failedTurn, false, new Set())).fold).toMatchObject({ failedTurn: true });
  });

  test("a turn that wrote no answer keeps the time in its header", () => {
    const answered = foldTurns(items, false, new Set()).find((r) => "fold" in r) as { fold: TurnFold };
    expect(answered.fold.answered).toBe(true);
    const empty = [
      user("u"),
      tool("c"),
      { kind: "turnEnd", id: "e", elapsedMs: 4_000, at: 1_700_000_000_000, outcome: "completed" },
    ] as TranscriptItem[];
    const fold = foldTurns(empty, false, new Set()).find((r) => "fold" in r) as { fold: TurnFold };
    expect(fold.fold).toMatchObject({ answered: false, time: 1_700_000_000_000, elapsedMs: 4_000 });
    // A reply of spaces is no answer either.
    const blank = foldTurns(
      [user("u"), tool("c"), reply(" "), { kind: "turnEnd", id: "e", elapsedMs: 4_000 }] as TranscriptItem[],
      false,
      new Set(),
    ).find((r) => "fold" in r) as { fold: TurnFold };
    expect(blank.fold.answered).toBe(false);
  });
});

describe("tool groups", () => {
  const kinded = (
    id: string,
    toolKind: string,
    status: "completed" | "failed" | "in_progress" = "completed",
  ): TranscriptItem => ({
    kind: "tool",
    id,
    title: id,
    toolKind,
    status,
    diffs: [],
  });
  const live = (items: TranscriptItem[], open: ReadonlySet<string> = new Set()) =>
    groupTools(foldTurns(items, true, new Set()), open);

  test("two or more finished calls in a row fold into one row", () => {
    const rows = live([
      user("u"),
      kinded("a", "read"),
      kinded("b", "read"),
      kinded("c", "execute"),
      kinded("d", "edit", "in_progress"),
    ]);
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "d"]);
    const group = rows[1]!;
    expect("group" in group && group.group.label).toBe("Read 2 files and ran 1 command");
    expect("group" in group && group.group.kind).toBe("read");
  });

  test("one call stays as it is, and an open group lists its calls", () => {
    expect(
      live([user("u"), kinded("a", "read"), kinded("b", "read", "in_progress")]).map((r) => r.key),
    ).toEqual(["u", "a", "b"]);
    const rows = live(
      [user("u"), kinded("a", "read"), kinded("b", "read"), kinded("c", "read")],
      new Set(["tools-a"]),
    );
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "a", "b", "c"]);
    expect(rows[2] && "inFold" in rows[2] ? rows[2].inFold : undefined).toBe("tools-a");
  });

  test("the label counts what the calls did, most first", () => {
    expect(toolGroupLabel(["execute", "read", "execute", "search", undefined])).toBe(
      "Ran 2 commands, read 1 file and performed 2 other actions",
    );
    expect(toolGroupLabel(["read", "read"])).toBe("Read 2 files");
    expect(toolGroupLabel(["read", "edit", "execute"])).toBe(
      "Read 1 file, edited 1 file and performed 1 other action",
    );
  });

  test("orchestration calls read as what each did to threads", () => {
    expect(toolGroupLabel(["start", "start", "start", "execute"])).toBe(
      "Started 3 threads and ran 1 command",
    );
    expect(toolGroupLabel(["message", "message"])).toBe("Messaged 2 threads");
    expect(toolGroupLabel(["wait", "wait"])).toBe("Waited for threads");
    expect(toolGroupLabel(["read_thread", "list_thread"])).toBe("Read 1 thread and listed threads");
  });

  test("a fan-out counts the threads it sent work to", () => {
    const rows = live([user("u"), fanOut("f", ["a", "b", "c"]), kinded("a", "read"), kinded("b", "read")]);
    const group = rows[1]!;
    expect("group" in group && group.group.label).toBe("Started 3 threads and read 2 files");
    const one = live([user("u"), fanOut("f", []), kinded("a", "read"), kinded("b", "read")]);
    const oneGroup = one[1]!;
    expect("group" in oneGroup && oneGroup.group.label).toBe("Read 2 files and started 1 thread");
  });

  test("an orchestration call's kind comes from its name, on RustCode's server or bare", () => {
    const kindOf = (title: string) => inferToolKind({ title });
    expect(kindOf("mcp__rustcode__start_session")).toBe("start");
    expect(kindOf("start_session")).toBe("start");
    expect(kindOf("message_session")).toBe("message");
    expect(kindOf("mcp__rustcode__fan_out")).toBe("start");
    expect(kindOf("mcp__rustcode__wait_session")).toBe("wait");
    expect(kindOf("read_session")).toBe("read_thread");
    expect(kindOf("mcp__rustcode__list_sessions")).toBe("list_thread");
    // Another server's tool of the same name is not RustCode's: it keeps its own reading.
    expect(kindOf("mcp__github__list_sessions")).toBe("other");
    expect(kindOf("mcp__github__start_session")).toBe("other");
    // Everything else reads as before.
    expect(kindOf("Read")).toBe("read");
    expect(kindOf("bash")).toBe("execute");
    expect(kindOf("bin/run_tests")).toBe("other");
  });
});

describe("the running turn", () => {
  const long = [
    user("u"),
    thought("t"),
    tool("a1", "completed", "src/x.ts"),
    tool("a2", "failed"),
    tool("a3"),
    reply("r"),
    tool("b1"),
    tool("b2"),
  ];

  test("shows as it streams, nothing folded", () => {
    expect(keys(foldTurns(long, true, new Set()))).toEqual(["u", "t", "a1", "a2", "a3", "r", "b1", "b2"]);
  });

  test("its finished calls (and the thoughts among them) read as one line per run", () => {
    const rows = groupTools(foldTurns(long, true, new Set()), new Set());
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-t", "r", "tools-b1"]);
    const first = rows[1]!;
    expect("group" in first && first.group).toMatchObject({ count: 4, failed: 1 });
  });
});

describe("what a finished turn keeps in view", () => {
  test("subagents and started threads stay out of the fold", () => {
    const tool = (id: string, title: string, toolName?: string): TranscriptItem => ({
      kind: "tool",
      id,
      title,
      toolName,
      status: "completed",
      diffs: [],
    });
    const rows = foldTurns(
      [
        { kind: "user", id: "u", text: "go" },
        tool("r", "Read a.ts"),
        tool("a", "Agent", "Agent"),
        tool("s", "start_session"),
        { kind: "turnEnd", id: "e", outcome: "completed" },
      ],
      false,
      new Set(),
    );
    const keys = rows.flatMap((r) => ("item" in r ? [r.item.id] : []));
    expect(keys).toEqual(["u", "a", "s", "e"]);
  });
});
