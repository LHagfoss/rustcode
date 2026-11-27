import { expect, test } from "bun:test";
import type { Checkpoint } from "../acp/v2";
import { threadChanges } from "./ComposerStrip";

const checkpoint = (turn: number, insertions: number, deletions: number, attempt?: boolean): Checkpoint => ({
  turn,
  ref: `r${turn}`,
  prompt: "",
  filesChanged: insertions || deletions ? 1 : 0,
  insertions,
  deletions,
  attempt,
});

test("the strip adds up the turns that changed files and leaves rewound attempts out", () => {
  expect(
    threadChanges([
      checkpoint(1, 10, 2),
      checkpoint(2, 0, 0),
      checkpoint(3, 5, 1),
      checkpoint(4, 99, 99, true),
    ]),
  ).toEqual({
    turns: 2,
    added: 15,
    removed: 3,
  });
  expect(threadChanges(undefined)).toEqual({ turns: 0, added: 0, removed: 0 });
});
