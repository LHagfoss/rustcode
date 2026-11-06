import { describe, expect, test } from "bun:test";
import {
  beginSetup,
  dismissSetup,
  type NewSetup,
  type PendingSetup,
  pendingStatus,
  pendingSummary,
  setupFailed,
  setupInProgress,
  setupSucceeded,
} from "./setup";

const NEXT: NewSetup = {
  id: "pending-1",
  text: "Add the login form",
  title: "Add the login form",
  cwd: "/Users/you/code/rustcode",
  worktree: { repo: "/Users/you/code/rustcode", title: "Add the login form" },
};

const started = (over: Partial<PendingSetup> = {}): PendingSetup => ({
  ...NEXT,
  startedAt: 1_000,
  ...over,
});

describe("a thread setting up", () => {
  test("appears at once with its title and its own clock", () => {
    const pending = beginSetup(null, NEXT, 1_000);
    expect(pending).toMatchObject({ id: "pending-1", title: "Add the login form", startedAt: 1_000 });
    expect(setupInProgress(pending)).toBe(true);
  });

  test("becomes the real thread in one update, clearing only itself", () => {
    const pending = started();
    expect(setupSucceeded(pending, pending.id)).toBeNull();
    expect(setupSucceeded(pending, "pending-2")).toBe(pending);
    expect(setupSucceeded(null, pending.id)).toBeNull();
  });

  test("a failure keeps the message and the worktree, and says why", () => {
    const failed = setupFailed(started(), "the branch is already checked out", 2_000);
    expect(failed.failure).toEqual({ text: "the branch is already checked out", at: 2_000 });
    expect(failed.text).toBe("Add the login form");
    expect(failed.worktree?.branch).toBeUndefined();
    expect(setupInProgress(failed)).toBe(false);
  });

  test("a second send while it is setting up starts no second worktree", () => {
    const pending = started();
    expect(beginSetup(pending, { ...NEXT, id: "pending-2" }, 3_000)).toBeNull();
    expect(setupInProgress(pending)).toBe(true);
  });

  test("a failed setup can be retried, and dismissal leaves nothing", () => {
    const failed = setupFailed(started(), "no git here");
    const retry = beginSetup(failed, { ...NEXT, id: "pending-2" }, 4_000);
    expect(retry).toMatchObject({ id: "pending-2", startedAt: 4_000 });
    expect(retry?.failure).toBeUndefined();
    expect(setupSucceeded(failed, null)).toBe(failed);
    expect(dismissSetup()).toBeNull();
  });

  test("the row carries the project and the branch it will use", () => {
    const row = pendingSummary(
      started({ worktree: { repo: "/Users/you/code/rustcode", branch: "feat/login" } }),
    );
    expect(row).toMatchObject({
      id: "pending-1",
      cwd: "/Users/you/code/rustcode",
      title: "Add the login form",
      updatedAt: 1_000,
    });
    expect(row.worktree).toMatchObject({ branch: "feat/login", repo: "/Users/you/code/rustcode" });
  });

  test("a thread with no worktree is placed by its folder, as its own row will be", () => {
    const row = pendingSummary(started({ worktree: undefined }));
    expect(row.worktree).toBeUndefined();
    expect(row.cwd).toBe("/Users/you/code/rustcode");
    expect(pendingStatus(started({ worktree: undefined }))).toBe("Starting…");
    expect(pendingStatus(started())).toBe("Setting up worktree…");
  });

  test("it holds the message's own attachments and links until the session exists", () => {
    const extras = {
      links: ["https://example.com"],
      attachments: [
        { id: "img1", kind: "image" as const, name: "shot.png", mimeType: "image/png", data: "AAAA" },
      ],
    };
    const pending = beginSetup(null, { ...NEXT, extras }, 1_000)!;
    expect(pending.extras).toBe(extras);
    // A failure keeps them with the message: a retry sends exactly what was written.
    expect(setupFailed(pending, "no git here").extras).toBe(extras);
  });
});
