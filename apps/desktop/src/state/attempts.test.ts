import { describe, expect, test } from "bun:test";
import type { Checkpoint } from "../acp/v2";
import { attemptTurns, checkpointsByMessage, splitAtTurn, transcriptTurns } from "./attempts";
import type { TranscriptItem } from "./thread";

const user = (id: string): TranscriptItem => ({ kind: "user", id, text: id });
const reply = (id: string): TranscriptItem => ({ kind: "assistant", id, text: id });
const tool = (id: string): TranscriptItem => ({
  kind: "tool",
  id,
  title: id,
  status: "completed",
  diffs: [],
});
const end = (id: string, at?: number): TranscriptItem => ({ kind: "turnEnd", id, outcome: "completed", at });
const steer = (id: string): TranscriptItem => ({ kind: "user", id, text: id, steered: true });

const checkpoint = (turn: number, prompt: string, files = 1): Checkpoint => ({
  turn,
  ref: `refs/rustcode/checkpoints/t/${turn}`,
  prompt,
  filesChanged: files,
  insertions: 2,
  deletions: 1,
});

/** Three turns, as a thread that ran all of them reads. */
const thread = (): TranscriptItem[] => [
  user("u1"),
  tool("c1"),
  reply("a1"),
  end("e1"),
  user("u2"),
  tool("c2"),
  reply("a2"),
  end("e2"),
  user("u3"),
  reply("a3"),
  end("e3"),
];

describe("splitting a transcript where a rewind cuts it", () => {
  test("the turn and every turn after it go, from the message that started it", () => {
    const items = thread();
    const split = splitAtTurn(items, "e2")!;
    expect(split.keep.map((item) => item.id)).toEqual(["u1", "c1", "a1", "e1"]);
    expect(split.tail.map((item) => item.id)).toEqual(["u2", "c2", "a2", "e2", "u3", "a3", "e3"]);
  });

  test("the first turn takes its own message, and nothing above it", () => {
    const split = splitAtTurn(thread(), "e1")!;
    expect(split.keep).toEqual([]);
    expect(split.tail).toHaveLength(11);
  });

  test("the last turn leaves everything before it live", () => {
    const split = splitAtTurn(thread(), "e3")!;
    expect(split.keep.map((item) => item.id)).toEqual(["u1", "c1", "a1", "e1", "u2", "c2", "a2", "e2"]);
    expect(split.tail.map((item) => item.id)).toEqual(["u3", "a3", "e3"]);
  });

  test("a message steered into a turn does not become the turn's start", () => {
    const items = [user("u1"), end("e1"), user("u2"), steer("s2"), reply("a2"), end("e2")];
    const split = splitAtTurn(items, "e2")!;
    expect(split.tail.map((item) => item.id)).toEqual(["u2", "s2", "a2", "e2"]);
  });

  test("a turn with no message of its own cannot be cut, so nothing is claimed", () => {
    // A resumed turn starts the transcript as this reader knows it: taking the tail would take
    // turns that are not this one.
    expect(splitAtTurn([tool("c1"), reply("a1"), end("e1"), user("u2"), end("e2")], "e1")).toBeUndefined();
    expect(splitAtTurn(thread(), "missing")).toBeUndefined();
  });
});

describe("an attempt as the transcript reads it", () => {
  test("counts its turns, and says one for a cut with no finished turn", () => {
    expect(attemptTurns(thread())).toBe(3);
    expect(attemptTurns([user("u1"), reply("a1")])).toBe(1);
  });

  test("pairs each finished turn with the message that started it", () => {
    expect(transcriptTurns(thread())).toEqual([
      { id: "e1", prompt: "u1", at: undefined },
      { id: "e2", prompt: "u2", at: undefined },
      { id: "e3", prompt: "u3", at: undefined },
    ]);
  });
});

describe("the minimap's checkpoint lane", () => {
  const byEnd = { e1: checkpoint(1, "u1"), e2: checkpoint(2, "u2") };

  test("one node per turn a rewind can go back to, hung on the message that started it", () => {
    const nodes = checkpointsByMessage(thread(), byEnd);
    expect(Object.keys(nodes)).toEqual(["u1", "u2"]);
    expect(nodes.u1!.turn).toBe(1);
    expect(nodes.u2!.turn).toBe(2);
  });

  test("a message that started no paired turn gets no node", () => {
    const nodes = checkpointsByMessage(thread(), { e3: checkpoint(3, "u3") });
    expect(Object.keys(nodes)).toEqual(["u3"]);
  });

  test("a resumed turn, which has no message, hangs nowhere", () => {
    const items = [...thread(), end("e4")];
    expect(checkpointsByMessage(items, { e4: checkpoint(4, "") })).toEqual({});
  });
});
