// The files a thread attaches to a message it sends another one: the paths it
// may name, and the composer extras the turn carries. The shell measures a
// file (`src-tauri` `attached_files`); these rules decide which paths reach it.

import { describe, expect, test } from "bun:test";
import {
  attachedExtras,
  attachmentRefusal,
  attachmentRoots,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENT_FILES,
} from "./attachments";
import type { RuleThread } from "./orchestration";

const thread = (extra: Partial<RuleThread> = {}): RuleThread => ({
  id: "caller",
  title: "Caller",
  folder: "/Users/you/code/rustcode",
  running: false,
  ...extra,
});

describe("attachmentRefusal", () => {
  test("a file under the project is fine, at any depth", () => {
    const roots = attachmentRoots(thread());
    expect(roots).toContain("/Users/you/code/rustcode");
    expect(attachmentRefusal("/Users/you/code/rustcode/a.ts", roots)).toBeNull();
    expect(attachmentRefusal("/Users/you/code/rustcode/docs/deep/b.md", roots)).toBeNull();
    // macOS links /tmp into /private/tmp; the roots are compared the same way.
    expect(attachmentRefusal("/Users/you/code/rustcode-other/a.ts", roots)).not.toBeNull();
  });

  test("a file outside the project is refused", () => {
    const roots = attachmentRoots(thread());
    for (const path of ["/etc/passwd", "/Users/you/.ssh/id_rsa", "/Users/you/code/rustcode/../secret"]) {
      const refused = attachmentRefusal(path, roots);
      expect(refused).not.toBeNull();
      expect(refused?.code).toBe("not_allowed");
    }
  });

  test("a relative path is a mistake, not a guess", () => {
    const roots = attachmentRoots(thread());
    const refused = attachmentRefusal("docs/notes.md", roots);
    expect(refused?.code).toBe("invalid");
    expect(refused?.message).toContain("not an absolute path");
  });

  test("a worktree thread reaches its worktree and its project, and nothing else", () => {
    const worktree = thread({
      folder: "/Users/you/.config/rustcode/worktrees/rustcode/feat-x",
      worktree: {
        path: "/Users/you/.config/rustcode/worktrees/rustcode/feat-x",
        branch: "feat/x",
        repo: "/Users/you/code/rustcode",
      } as RuleThread["worktree"],
    });
    const roots = attachmentRoots(worktree);
    expect(roots).toContain("/Users/you/code/rustcode");
    expect(attachmentRefusal("/Users/you/code/rustcode/a.ts", roots)).toBeNull();
    expect(attachmentRefusal("/Users/you/.config/rustcode/worktrees/rustcode/feat-x/b.ts", roots)).toBeNull();
    expect(attachmentRefusal("/Users/you/code/rustcode-mobile/a.ts", roots)).not.toBeNull();
  });

  test("a thread with no folder can attach nothing", () => {
    expect(attachmentRefusal("/tmp/a.ts", attachmentRoots(thread({ folder: "" })))).not.toBeNull();
  });
});

describe("attachedExtras", () => {
  test("each file becomes the composer's path attachment, so a turn carries the same chips", () => {
    const extras = attachedExtras([
      { path: "/repo/a.png", name: "a.png", bytes: 10 },
      { path: "/repo/b.log", name: "b.log", bytes: 20 },
    ]);
    expect(extras.attachments).toHaveLength(2);
    expect(extras.attachments[0]).toMatchObject({ kind: "file", name: "a.png", path: "/repo/a.png" });
    expect(extras.links).toEqual([]);
  });

  test("ten files of 5 MB are the most one message carries", () => {
    expect(MAX_ATTACHMENT_FILES).toBe(10);
    expect(MAX_ATTACHMENT_BYTES).toBe(5 * 1024 * 1024);
  });
});
