import { describe, expect, test } from "bun:test";
import { folderRows, folderScore, matchSpans, middleEllipsis } from "./folderList";

const HOME = "/Users/ada";
const CODE = "/Users/ada/code";
const REPO = "/Users/ada/code/rustcode";
const OTHER = "/Users/ada/code/notes";
const WORKTREES = `${HOME}/.config/rustcode/worktrees`;
const CUT = `${WORKTREES}/rustcode/feat-composer-footer`;
const DAY = 86_400_000;
const NOW = Date.UTC(2026, 0, 30);

describe("the folder picker's rows", () => {
  test("a folder that only holds other projects is not offered", () => {
    const rows = folderRows([{ cwd: "/" }, { cwd: HOME }, { cwd: CODE }, { cwd: REPO }, { cwd: OTHER }]);
    expect(rows.map((row) => row.path)).toEqual([OTHER, REPO]);
  });

  test("the home folder is a place projects live, so it goes too", () => {
    expect(folderRows([{ cwd: HOME }, { cwd: REPO }]).map((row) => row.path)).toEqual([REPO]);
    expect(folderRows([{ cwd: HOME }])).toEqual([]);
    expect(folderRows([{ cwd: "/home/ada" }, { cwd: "/home/ada/notes" }]).map((row) => row.path)).toEqual([
      "/home/ada/notes",
    ]);
  });

  test("a folder that is not a container and holds nothing stays", () => {
    const rows = folderRows([{ cwd: "/Volumes/work" }, { cwd: "/Volumes/archive" }]);
    expect(rows.map((row) => row.path)).toEqual(["/Volumes/archive", "/Volumes/work"]);
  });

  test("a real project with a project's threads nested inside it keeps its row", () => {
    // The bug: the app's own threads ran in `apps/desktop/src-tauri`, so `rustcode` looked like a
    // folder that only holds a project and vanished from the picker.
    const TAURI = `${REPO}/apps/desktop/src-tauri`;
    const APPS = `${REPO}/apps/desktop`;
    const rows = folderRows([{ cwd: REPO }, { cwd: TAURI }, { cwd: APPS }, { cwd: OTHER }]);
    const paths = rows.map((row) => row.path);
    expect(paths).toContain(REPO);
    expect(paths).toContain(TAURI);
    // `apps/desktop` sits inside a project, not in a place: a folder called `desktop` there is a
    // folder of the project's own, so it keeps its row.
    expect(paths).toContain(APPS);
    expect(
      folderRows([{ cwd: REPO }, { cwd: TAURI }], { filter: "rustcode" }).map((row) => row.path),
    ).toEqual([REPO, TAURI]);
  });

  test("a container is a place only when it sits in one", () => {
    const API = "/Users/ada/work/api";
    const rows = folderRows([{ cwd: "/Users/ada/work" }, { cwd: API }, { cwd: "/Users/ada/code/work" }]);
    // `~/work` holds a project and sits in the home folder: a place, not a row.
    expect(rows.map((row) => row.path)).toEqual([API, "/Users/ada/code/work"]);
    // A container name deep inside a project is that project's own folder.
    const deep = folderRows([{ cwd: `${REPO}/src` }, { cwd: `${REPO}/src/app` }]);
    expect(deep.map((row) => row.path)).toEqual([`${REPO}/src/app`, `${REPO}/src`]);
  });

  test("a repo's worktrees never crowd out the other projects", () => {
    const branches = Array.from({ length: 40 }, (_, i) => {
      const path = `${WORKTREES}/rustcode/branch-${i}`;
      return { cwd: path, worktree: { path, repo: REPO, branch: `branch-${i}` }, updatedAt: NOW - i * DAY };
    });
    const others = Array.from({ length: 5 }, (_, i) => ({ cwd: `/tmp/project-${i}` }));
    const rows = folderRows([{ cwd: REPO }, ...branches, ...others], { now: NOW });
    const projects = rows.filter((row) => !row.repo);
    expect(projects).toHaveLength(6);
    expect(projects.map((row) => row.path)).toContain(REPO);
    for (let i = 0; i < 5; i++) expect(projects.map((row) => row.path)).toContain(`/tmp/project-${i}`);
    // Three branches stand in for the forty, the most recently used ones.
    const shown = rows.filter((row) => row.repo === REPO).map((row) => row.name);
    expect(shown).toEqual(["branch-0", "branch-1", "branch-2"]);
    // A search asks for everything: all forty are there.
    expect(
      folderRows([{ cwd: REPO }, ...branches], { filter: "rustcode", now: NOW, limit: 100 }),
    ).toHaveLength(41);
  });

  test("a worktree that names itself as its repo is one row, not two", () => {
    const rows = folderRows([{ cwd: CUT, worktree: { path: CUT, repo: CUT, branch: "feat/composer" } }]);
    expect(rows).toEqual([{ path: CUT, name: "feat/composer", repo: undefined }]);
  });

  test("the newer folder first, once each", () => {
    const sources = [
      { cwd: OTHER, updatedAt: NOW - DAY },
      { cwd: REPO, updatedAt: NOW },
      { cwd: OTHER, updatedAt: NOW - DAY },
    ];
    expect(folderRows(sources, { now: NOW }).map((row) => row.name)).toEqual(["rustcode", "notes"]);
  });

  test("use ranks the projects: recent beats yesterday and a lot long ago", () => {
    const many = (cwd: string, count: number, ago: number) =>
      Array.from({ length: count }, () => ({ cwd, updatedAt: NOW - ago }));
    const rows = folderRows(
      [
        ...many(REPO, 40, 0),
        { cwd: OTHER, updatedAt: NOW - DAY },
        ...many("/Users/ada/code/old", 40, 400 * DAY),
      ],
      { now: NOW },
    );
    expect(rows.map((row) => row.name)).toEqual(["rustcode", "notes", "old"]);
  });

  test("projects of the same use are ordered by name", () => {
    const rows = folderRows([{ cwd: "/tmp/beta" }, { cwd: "/tmp/alpha" }], { now: NOW });
    expect(rows.map((row) => row.name)).toEqual(["alpha", "beta"]);
  });

  test("a worktree nests under its repo, right after it, named by its branch", () => {
    const rows = folderRows([
      { cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } },
      { cwd: OTHER },
      { cwd: REPO },
    ]);
    // The worktree's thread counts toward the repo, so the repo outranks the other project.
    expect(rows.map((row) => row.path)).toEqual([REPO, CUT, OTHER]);
    expect(rows[1]).toEqual({ path: CUT, name: "feat/composer", repo: REPO });
  });

  test("a repo only used through its worktrees is a project, with them under it", () => {
    const rows = folderRows([{ cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } }]);
    expect(rows).toEqual([
      { path: REPO, name: "rustcode" },
      { path: CUT, name: "feat/composer", repo: REPO },
    ]);
  });

  test("a worktree the engine did not record still finds its repo by name", () => {
    const rows = folderRows([{ cwd: REPO }, { cwd: `${WORKTREES}/rustcode/fix-tray` }]);
    expect(rows.map((row) => row.path)).toEqual([REPO, `${WORKTREES}/rustcode/fix-tray`]);
    expect(rows[1]!.repo).toBe(REPO);
    expect(rows[1]!.name).toBe("fix-tray");
  });

  test("a worktree with no repo at all is a row of its own, after the projects", () => {
    const rows = folderRows([
      { cwd: OTHER },
      { cwd: "/tmp/wt", worktree: { path: "/tmp/wt", branch: "feat/composer" } },
    ]);
    expect(rows.map((row) => row.path)).toEqual([OTHER, "/tmp/wt"]);
    expect(rows[1]!.repo).toBeUndefined();
  });

  test("a filtering search keeps a worktree under the repo it names", () => {
    const sources = [
      { cwd: REPO },
      { cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } },
      { cwd: OTHER },
    ];
    expect(folderRows(sources, { filter: "rust" }).map((row) => row.path)).toEqual([REPO, CUT]);
    expect(folderRows(sources, { filter: "composer" }).map((row) => row.path)).toEqual([CUT]);
    expect(folderRows(sources, { filter: "notes" }).map((row) => row.path)).toEqual([OTHER]);
  });

  test("a path matches whatever the folder is called", () => {
    const rows = folderRows([{ cwd: REPO }], { filter: "/code/" });
    expect(rows.map((row) => row.path)).toEqual([REPO]);
  });

  test("a filtering search keeps the ranking", () => {
    const many = (cwd: string, count: number) =>
      Array.from({ length: count }, () => ({ cwd, updatedAt: NOW }));
    const rows = folderRows([...many("/tmp/rust-a", 5), ...many("/tmp/rust-b", 30)], {
      filter: "rust",
      now: NOW,
    });
    expect(rows.map((row) => row.name)).toEqual(["rust-b", "rust-a"]);
  });

  test("the list stops at the cap", () => {
    const sources = Array.from({ length: 8 }, (_, i) => ({ cwd: `/tmp/p${i}` }));
    expect(folderRows(sources, { limit: 3 })).toHaveLength(3);
  });
});

describe("how much a project is used", () => {
  test("a project used now is 0.7 of the score, and volume is the other 0.3", () => {
    expect(folderScore({ updatedAt: NOW, threads: 1 }, NOW)).toBeCloseTo(
      0.7 + 0.3 * (Math.log(2) / Math.log(51)),
    );
    expect(folderScore({ updatedAt: NOW, threads: 1 }, NOW)).toBeLessThan(1);
  });

  test("recency halves every two weeks", () => {
    expect(folderScore({ updatedAt: NOW, threads: 0 }, NOW)).toBeCloseTo(0.7);
    expect(folderScore({ updatedAt: NOW - 14 * DAY, threads: 0 }, NOW)).toBeCloseTo(0.35);
    expect(folderScore({ updatedAt: NOW - 28 * DAY, threads: 0 }, NOW)).toBeCloseTo(0.175);
  });

  test("volume fills up at 51 threads", () => {
    expect(folderScore({ updatedAt: NOW, threads: 50 }, NOW)).toBeCloseTo(1);
    expect(folderScore({ updatedAt: NOW, threads: 500 }, NOW)).toBeCloseTo(1);
  });

  test("a project with no date is volume only", () => {
    expect(folderScore({ threads: 0 }, NOW)).toBe(0);
    expect(folderScore({ threads: 50 }, NOW)).toBeCloseTo(0.3);
  });
});

describe("the matched characters", () => {
  test("a row with no search is one plain run", () => {
    expect(matchSpans("rustcode", "")).toEqual([{ text: "rustcode", hit: false }]);
  });

  test("the matched run is marked, either side of it plain", () => {
    expect(matchSpans("rustcode", "ust")).toEqual([
      { text: "r", hit: false },
      { text: "ust", hit: true },
      { text: "code", hit: false },
    ]);
  });

  test("every occurrence is marked", () => {
    expect(matchSpans("banana", "an").filter((span) => span.hit)).toHaveLength(2);
  });

  test("a match at either end needs no empty run", () => {
    expect(matchSpans("rust", "ru")).toEqual([
      { text: "ru", hit: true },
      { text: "st", hit: false },
    ]);
    expect(matchSpans("rust", "st")[0]).toEqual({ text: "ru", hit: false });
  });
});

describe("a path with its middle given up", () => {
  test("both ends stay, the middle goes", () => {
    expect(middleEllipsis("/Users/ada/code/rustcode", 20)).toContain("…");
    expect(middleEllipsis("/Users/ada/code/rustcode", 20).startsWith("/Users")).toBe(true);
    expect(middleEllipsis("/Users/ada/code/rustcode", 20).endsWith("rustcode")).toBe(true);
  });

  test("never longer than the budget", () => {
    for (let max = 6; max < 40; max++) {
      expect(middleEllipsis("/Users/ada/code/rustcode", max).length).toBeLessThanOrEqual(max);
    }
  });

  test("a path that fits comes back whole, and a sliver is still cut", () => {
    expect(middleEllipsis("/tmp/app", 20)).toBe("/tmp/app");
    // Too little room for an ellipsis between two ends: a hard cut, never over the budget.
    expect(middleEllipsis("/tmp/app", 4)).toBe("/tmp");
  });
});

describe("Windows folders", () => {
  test("names projects and removes drive, home and nested containers", () => {
    const repo = "C:\\Users\\ada\\code\\work\\app";
    expect(
      folderRows(
        ["C:\\", "C:\\Users\\ada", "C:\\Users\\ada\\code", "C:\\Users\\ada\\code\\work", repo].map((cwd) => ({
          cwd,
        })),
      ),
    ).toEqual([{ path: repo, name: "app" }]);
  });

  test("one folder is one row with or without the extended-length prefix", () => {
    const repo = "C:\\Users\\ada\\code\\app";
    expect(folderRows([{ cwd: `\\\\?\\${repo}` }, { cwd: repo }])).toEqual([{ path: repo, name: "app" }]);
  });

  test("RustCode's own install folder is not a project", () => {
    const repo = "C:\\Users\\ada\\code\\app";
    expect(
      folderRows([
        { cwd: "\\\\?\\C:\\Users\\ada\\AppData\\Local\\RustCode" },
        { cwd: "C:\\Users\\ada\\AppData\\Local\\RustCode" },
        { cwd: repo },
      ]),
    ).toEqual([{ path: repo, name: "app" }]);
  });

  test("nests AppData worktrees beneath their repo without worktree metadata", () => {
    const repo = "C:\\Users\\ada\\code\\app";
    const cut = "C:\\Users\\ada\\AppData\\Roaming\\rustcode\\worktrees\\app\\fix-folder";
    expect(folderRows([{ cwd: repo }, { cwd: cut }])).toEqual([
      { path: repo, name: "app" },
      { path: cut, name: "fix-folder", repo },
    ]);
  });

  test("names UNC projects and nests worktrees with explicit repo metadata", () => {
    const repo = "\\\\server\\share\\app";
    const cut = "\\\\server\\share\\worktrees\\fix-folder";
    expect(
      folderRows([
        { cwd: "\\\\server\\share" },
        { cwd: repo },
        { cwd: cut, worktree: { path: cut, repo, branch: "fix-folder" } },
      ]),
    ).toEqual([
      { path: repo, name: "app" },
      { path: cut, name: "fix-folder", repo },
    ]);
  });

  test("middle ellipsis retains a whole Windows tail folder", () => {
    expect(middleEllipsis("C:\\Users\\ada\\code\\rustcode", 20)).toBe("C:\\Users…\\rustcode");
    expect(middleEllipsis("\\\\server\\share\\code\\rustcode", 20)).toBe("\\\\server…\\rustcode");
  });
});

test("includes explicitly registered projects with no threads, including places", () => {
  const rows = folderRows([
    { cwd: "/", registered: true },
    { cwd: "/code/new", registered: true },
  ]);
  expect(rows.map((row) => row.path)).toContain("/");
  expect(rows.map((row) => row.path)).toContain("/code/new");
});
