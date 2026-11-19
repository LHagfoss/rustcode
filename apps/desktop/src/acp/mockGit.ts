import type { Json, JsonObject } from "./jsonrpc";
import { APP_PATCH, COMPOSER_PATCH, LOGIN_PATCH, type MockWorktree } from "./mockSeeds";

// The mock's git: canned changes, per-worktree status, and the QR rows of a pairing code.

/** Canned `git_changes`, as the shell answers it: the branch since its base adds the login form. */
export function mockGitChanges(cwd: string, base?: string): JsonObject {
  const files: Json[] = [
    { path: "src/app.ts", status: "M", added: 1, removed: 1, binary: false },
    { path: "src/ui/Composer.tsx", status: "M", added: 1, removed: 0, binary: false },
  ];
  let diff = `diff --git a/src/app.ts b/src/app.ts\nindex 1..2 100644\n--- a/src/app.ts\n+++ b/src/app.ts\n${APP_PATCH}\ndiff --git a/src/ui/Composer.tsx b/src/ui/Composer.tsx\nindex 3..4 100644\n--- a/src/ui/Composer.tsx\n+++ b/src/ui/Composer.tsx\n${COMPOSER_PATCH}\n`;
  if (base || cwd.includes("worktrees")) {
    files.push({ path: "src/ui/LoginForm.tsx", status: "A", added: 14, removed: 0, binary: false });
    diff += `diff --git a/src/ui/LoginForm.tsx b/src/ui/LoginForm.tsx\nnew file mode 100644\n--- /dev/null\n+++ b/src/ui/LoginForm.tsx\n${LOGIN_PATCH}\n`;
  }
  return { branch: cwd.includes("worktrees") ? "feat/login-form" : "main", files, diff };
}

/**
 * A QR-shaped module matrix for the mock's pairing dialog (one string of `0`
 * and `1` per row, quiet zone included). It is drawn, not encoded: the mock
 * has no pairing payload worth scanning, and the dialog only needs the shape a
 * real code has. The engine sends the real one (`remote_gateway/qr.rs`).
 */
export function mockQrRows(size = 37): string[] {
  let seed = 0x31_85_79;
  const next = () => {
    seed = (seed * 1_103_515_245 + 12_345) & 0x7fff_ffff;
    return seed / 0x7fff_ffff;
  };
  const rows: string[] = [];
  for (let y = 0; y < size; y += 1) {
    let row = "";
    for (let x = 0; x < size; x += 1) {
      const inFinder = (ox: number, oy: number) =>
        x >= ox && x < ox + 7 && y >= oy && y < oy + 7
          ? // A finder pattern: a dark ring with a dark 3x3 centre.
            Math.max(Math.abs(x - ox - 3), Math.abs(y - oy - 3)) !== 2
          : null;
      // A four-module quiet zone, as a scanner expects.
      const left = inFinder(4, 4);
      const right = inFinder(size - 11, 4);
      const bottom = inFinder(4, size - 11);
      const finder = left ?? right ?? bottom;
      row += finder === null ? (next() > 0.5 ? "1" : "0") : finder ? "1" : "0";
    }
    rows.push(row);
  }
  return rows;
}

// ---------------------------------------------------------------- git (issue 3724)

export interface MockGit {
  branch: string;
  defaultBranch: string;
  onDefault: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  hasRemote: boolean;
  files: { path: string; status: string }[];
}

const mockGitStates = new Map<string, MockGit>();

/**
 * Each folder's git state, kept across calls so a commit, push and pull request move it on. A
 * worktree thread starts from its worktree's numbers; the main checkout has uncommitted changes.
 */
export function mockGit(threads: readonly { worktree?: MockWorktree }[], path: string): MockGit {
  const known = mockGitStates.get(path);
  if (known) return known;
  const worktree = threads.find((t) => t.worktree?.path === path)?.worktree;
  const state: MockGit = worktree
    ? {
        branch: worktree.branch,
        defaultBranch: "main",
        onDefault: false,
        upstream: `origin/${worktree.branch}`,
        ahead: worktree.ahead,
        behind: worktree.behind,
        hasRemote: true,
        files: worktree.dirty ? [{ path: "src/app.ts", status: "M" }] : [],
      }
    : {
        branch: "feat/desktop-git-actions",
        defaultBranch: "main",
        onDefault: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        hasRemote: true,
        files: [
          { path: "apps/desktop/src/ui/GitActions.tsx", status: "A" },
          { path: "apps/desktop/src/ui/ChatView.tsx", status: "M" },
          { path: "docs/desktop.md", status: "M" },
        ],
      };
  mockGitStates.set(path, state);
  return state;
}
