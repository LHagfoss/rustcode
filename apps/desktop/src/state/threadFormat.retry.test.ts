import { expect, test } from "bun:test";
import { retriedStarts, type ToolItem } from "./threadFormat";

const start = (id: string, status: ToolItem["status"], prompt: string): ToolItem => ({
  kind: "tool",
  id,
  title: "start_thread",
  status,
  input: { prompt, title: "Review MR 71" },
  diffs: [],
});

test("a failed start that a later call retried and started is left out", () => {
  const items = [start("a", "failed", "Review it"), start("b", "completed", "Review it")];
  expect([...retriedStarts(items)]).toEqual(["a"]);
});

test("a failed start that nothing retried keeps its row", () => {
  const items = [start("a", "completed", "Other"), start("b", "failed", "Review it")];
  expect(retriedStarts(items).size).toBe(0);
});
