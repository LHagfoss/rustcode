import { describe, expect, test } from "bun:test";
import {
  type Checkpoint,
  checkpointsByTurn,
  handoffRefusal,
  parseCheckpoints,
  parseHandoff,
  parseRestoreAnswer,
  parseWorktreeStates,
} from "./v2";

const hour = 3_600_000;
const t0 = 1_700_000_000_000;

/** The prompt as the engine stores it in a checkpoint's message: whitespace collapsed. */
const key = (text: string) => text.split(/\s+/).join(" ").trim().slice(0, 80);

const checkpoint = (turn: number, prompt: string, createdAt: number, files = 0): Checkpoint => ({
  turn,
  ref: `refs/rustcode/checkpoints/t/${turn}`,
  createdAt,
  prompt: key(prompt),
  filesChanged: files,
  insertions: files ? 2 : 0,
  deletions: files ? 7 : 0,
});

describe("pairing a turn with its checkpoint", () => {
  const wake = (link: number) =>
    `1 child finished\n\nFinished:\n\n- link_id 01a1ac64985b-7001-93c4-0cc6-0cc6d0f47c49/${link} · done · “Review delegate wake changes”`;

  test("a turn lists only its own changes, not the next turn's", () => {
    // Four finished turns and a fifth checkpoint, taken as the next turn started: the extra one
    // used to shift every chip one turn down, so one diff showed on two turns in a row.
    const turns = [
      { id: "t1", prompt: wake(72), at: t0 + hour },
      { id: "t2", prompt: wake(73), at: t0 + 2 * hour },
      { id: "t3", prompt: "Review the sidebar spacing", at: t0 + 3 * hour },
      { id: "t4", prompt: "And the changes chip", at: t0 + 4 * hour },
    ];
    const checkpoints = [
      checkpoint(1, wake(72), t0, 0),
      checkpoint(2, wake(73), t0 + hour, 0),
      checkpoint(3, "Review the sidebar spacing", t0 + 2 * hour, 2),
      checkpoint(4, "And the changes chip", t0 + 3 * hour, 1),
      // A turn that has not ended yet: its changes belong to no finished turn.
      checkpoint(5, "A turn still running", t0 + 4 * hour + 60_000, 3),
    ];
    const paired = checkpointsByTurn(turns, checkpoints);
    expect(paired.t1!.turn).toBe(1);
    expect(paired.t2!.turn).toBe(2);
    expect(paired.t3!.turn).toBe(3);
    expect(paired.t4!.turn).toBe(4);
    // The running turn's checkpoint, with its own edits, is on no turn at all.
    expect(Object.values(paired).map((c) => c.turn)).not.toContain(5);
    expect([paired.t1, paired.t2, paired.t3, paired.t4].map((c) => c!.filesChanged)).toEqual([0, 0, 2, 1]);
  });

  test("a checkpoint made after the last finished turn ends is left out", () => {
    const turns = [{ id: "t1", prompt: "Fix the composer focus ring", at: t0 + hour }];
    const checkpoints = [
      checkpoint(1, "Fix the composer focus ring", t0, 1),
      checkpoint(2, "Fix the composer focus ring", t0 + hour + 30_000, 2),
    ];
    expect(checkpointsByTurn(turns, checkpoints).t1!.turn).toBe(1);
  });

  test("a prompt the transcript does not hold is not shown on the wrong turn", () => {
    // The transcript starts mid-thread: its oldest turn's checkpoint was pruned, so the list is
    // one longer than the turns the reader has.
    const turns = [
      { id: "t2", prompt: "Add a login form", at: t0 + 2 * hour },
      { id: "t3", prompt: "Tidy the pricing page copy", at: t0 + 3 * hour },
    ];
    const checkpoints = [
      checkpoint(1, "Fix the composer focus ring", t0, 4),
      checkpoint(2, "Add a login form", t0 + hour, 3),
      checkpoint(3, "Tidy the pricing page copy", t0 + 2 * hour, 2),
    ];
    expect(checkpointsByTurn(turns, checkpoints)).toMatchObject({
      t2: { turn: 2, filesChanged: 3 },
      t3: { turn: 3, filesChanged: 2 },
    });
  });

  test("a turn whose own checkpoint is missing gets none, rather than the one above it", () => {
    const turns = [
      { id: "t1", prompt: "Fix the composer focus ring", at: t0 + hour },
      { id: "t2", prompt: "Add a login form", at: t0 + 2 * hour },
    ];
    const checkpoints = [checkpoint(1, "Fix the composer focus ring", t0, 1)];
    expect(checkpointsByTurn(turns, checkpoints)).toMatchObject({ t1: { turn: 1 } });
    expect(checkpointsByTurn(turns, checkpoints).t2).toBeUndefined();
  });

  test("the same message twice takes its own checkpoint each time", () => {
    const turns = [
      { id: "t1", prompt: "continue", at: t0 + hour },
      { id: "t2", prompt: "continue", at: t0 + 2 * hour },
    ];
    const checkpoints = [checkpoint(1, "continue", t0, 1), checkpoint(2, "continue", t0 + hour, 2)];
    expect(checkpointsByTurn(turns, checkpoints)).toMatchObject({
      t1: { turn: 1 },
      t2: { turn: 2 },
    });
  });

  test("a thread with no timing to go on still pairs from the end", () => {
    const turns = [
      { id: "t1", prompt: "One" },
      { id: "t2", prompt: "Two" },
    ];
    const checkpoints = [checkpoint(1, "One", t0, 1), checkpoint(2, "Two", t0 + hour, 2)];
    expect(checkpointsByTurn(turns, checkpoints)).toMatchObject({
      t1: { turn: 1 },
      t2: { turn: 2 },
    });
  });
});

describe("a rewind as an attempt", () => {
  const listed = (turn: number, prompt: string, attempt = false) => ({
    turn,
    ref: `refs/rustcode/checkpoints/t/${turn}`,
    created_at: new Date(t0 + turn * hour).toISOString(),
    prompt,
    files_changed: 1,
    insertions: 2,
    deletions: 1,
    ...(attempt ? { attempt: true } : {}),
  });

  test("a checkpoint kept for the abandoned state pairs with no turn", () => {
    // turn 3 is the state the rewind left, kept after turn 2: an empty prompt used to match any
    // turn, so it must be told apart by what it is, not by its message.
    const checkpoints = parseCheckpoints({
      checkpoints: [listed(1, "one"), listed(2, "two"), listed(3, "", true)],
    });
    expect(checkpoints.map((c) => c.attempt)).toEqual([false, false, true]);
    const turns = [
      { id: "t1", prompt: "one", at: t0 + 2 * hour },
      { id: "t2", prompt: "two", at: t0 + 3 * hour },
    ];
    expect(checkpointsByTurn(turns, checkpoints)).toMatchObject({ t1: { turn: 1 }, t2: { turn: 2 } });
  });

  test("the answer says whether the conversation came back and what was kept", () => {
    expect(parseRestoreAnswer({ restored: true, conversation: "rewound", kept: { turn: 4 } })).toEqual({
      conversation: "rewound",
      kept: 4,
      rewindError: undefined,
    });
    expect(
      parseRestoreAnswer({ restored: true, conversation: "kept", rewind_error: "no such message" }),
    ).toEqual({
      conversation: "kept",
      kept: undefined,
      rewindError: "no such message",
    });
    expect(parseRestoreAnswer(undefined)).toEqual({
      conversation: "kept",
      kept: undefined,
      rewindError: undefined,
    });
  });
});

describe("a worktree's state and a handoff", () => {
  test("each row names the branch, the distance, the changed files and the last commit", () => {
    const rows = parseWorktreeStates({
      worktrees: [
        {
          path: "/wt/login",
          exists: true,
          branch: "feat/login",
          against: "origin/feat/login",
          ahead: 2,
          behind: 1,
          dirty: 3,
          last_commit: "2026-05-01T10:00:00Z",
          last_commit_subject: "feat(mcp): add the tool",
        },
        // A worktree removed since the app listed it: a row, not an error.
        { path: "/wt/gone", exists: false },
        // A git that could not answer says so, and gives no counts.
        { path: "/wt/broken", exists: true, error: "git status failed: boom" },
      ],
    });
    expect(rows[0]).toMatchObject({
      path: "/wt/login",
      branch: "feat/login",
      against: "origin/feat/login",
      ahead: 2,
      behind: 1,
      dirty: 3,
      lastCommit: "2026-05-01T10:00:00Z",
      lastCommitSubject: "feat(mcp): add the tool",
    });
    expect(rows[1]).toMatchObject({ path: "/wt/gone", exists: false });
    expect(rows[1]!.dirty).toBeUndefined();
    expect(rows[2]).toMatchObject({ path: "/wt/broken", error: "git status failed: boom" });
    expect(parseWorktreeStates(undefined)).toEqual([]);
    // A row without a path names nothing, so it is left out.
    expect(parseWorktreeStates({ worktrees: [{ exists: true }] })).toEqual([]);
  });

  test("a handoff's answer says which way it moved and why a switch did not happen", () => {
    expect(
      parseHandoff({
        session_id: "d1",
        to: "checkout",
        moved: true,
        worktree_removed: true,
        switched: false,
        branch: "feat/login",
        path: "/repo",
        detail: "the checkout could not switch to it: local changes",
      }),
    ).toEqual({
      sessionId: "d1",
      to: "checkout",
      moved: true,
      worktreeRemoved: true,
      switched: false,
      branch: "feat/login",
      path: "/repo",
      repo: undefined,
      detail: "the checkout could not switch to it: local changes",
    });
    expect(parseHandoff({ to: "worktree", moved: true, path: "/wt", branch: "feat/x" })).toMatchObject({
      to: "worktree",
      moved: true,
      path: "/wt",
    });
    expect(parseHandoff(undefined)).toBeUndefined();
  });

  test("a refusal keeps the reason the engine gave, and nothing else", () => {
    expect(handoffRefusal({ reason: "dirty_checkout" })).toBe("dirty_checkout");
    expect(handoffRefusal({ reason: "unpushed_checkout" })).toBe("unpushed_checkout");
    expect(handoffRefusal({ reason: "ignored_files" })).toBe("ignored_files");
    expect(handoffRefusal({ reason: "has_worktree" })).toBe("has_worktree");
    expect(handoffRefusal({ reason: "something else" })).toBeUndefined();
    expect(handoffRefusal(undefined)).toBeUndefined();
  });
});
