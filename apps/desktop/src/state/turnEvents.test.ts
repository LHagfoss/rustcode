import { expect, test } from "bun:test";
import type { TranscriptItem } from "./thread";
import { eventsText, keepEvent, turnEvents } from "./turnEvents";

const items = [
  { kind: "user", id: "u", text: "Fix it\nKeep the sidebar visible.", at: 1 },
  { kind: "thought", id: "t", text: "Reading the request\nmore", summary: false, startedAt: 2, done: true },
  {
    kind: "tool",
    id: "c",
    title: "bash",
    toolName: "Bash",
    toolKind: "execute",
    status: "completed",
    input: { command: "bun test" },
    output: "ok",
    diffs: [],
    startedAt: 3,
  },
  { kind: "tool", id: "f", title: "edit", toolKind: "edit", status: "failed", diffs: [], startedAt: 4 },
  { kind: "assistant", id: "a", text: "Done.\nAll green.", at: 5 },
] as unknown as TranscriptItem[];

test("a turn's rows become its events in order, with badges and kinds", () => {
  const events = turnEvents(items);
  expect(events.map((e) => [e.badge, e.kind, e.at])).toEqual([
    ["You", "user", 1],
    ["Thinking", "thinking", 2],
    ["Bash", "run", 3],
    ["Edit", "failed", 4],
    ["Agent", "agent", 5],
  ]);
  expect(events[0]!.line).toBe("Fix it");
  expect(events[0]!.body).toBe("Fix it\nKeep the sidebar visible.");
  expect(events[1]!.line).toBe("Reading the request");
  expect(events[2]!.body).toContain("bun test");
  expect(events[2]!.body).toContain("ok");
});

test("filters keep what they name", () => {
  const events = turnEvents(items);
  expect(events.filter((e) => keepEvent(e, "tools")).map((e) => e.id)).toEqual(["c", "f"]);
  expect(events.filter((e) => keepEvent(e, "agent")).map((e) => e.id)).toEqual(["u", "t", "a"]);
  expect(events.filter((e) => keepEvent(e, "failed")).map((e) => e.id)).toEqual(["f"]);
  expect(eventsText(events)).toStartWith("#1 You: Fix it");
});
