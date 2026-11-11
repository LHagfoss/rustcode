// Finding the same project on another computer (docs/desktop.md › Several machines): a project is
// a folder, its name, and the git remote it came from, and **Run on** uses those to carry a
// conversation's folder from one machine to the same project on another.

import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { isRootFolder, matchProject, projectAt, projectsOn, remoteRepository } from "./projects";
import { LOCAL_HOST } from "./threadKeys";

const STUDIO = "gw-8e1f4b7a5c2d4e6f";

function session(over: Partial<SessionSummary> & { cwd: string }): SessionSummary {
  return { id: over.id ?? over.cwd, ...over };
}

describe("a git remote as one repository", () => {
  test("the same repository spells the same, however git prints it", () => {
    for (const remote of [
      "git@github.com:LHagfoss/rustcode.git",
      "git@github.com:LHagfoss/rustcode",
      "https://github.com/lhagfoss/rustcode.git",
      "https://github.com/LHagfoss/rustcode",
      "ssh://git@github.com/LHagfoss/rustcode.git",
      "https://user:token@github.com/LHagfoss/rustcode",
    ]) {
      expect(remoteRepository(remote)).toBe("github.com/lhagfoss/rustcode");
    }
  });

  test("another host, another repository, or nothing", () => {
    expect(remoteRepository("git@gitlab.com:LHagfoss/rustcode.git")).not.toBe(
      remoteRepository("git@github.com:LHagfoss/rustcode.git"),
    );
    expect(remoteRepository("git@github.com:LHagfoss/other.git")).not.toBe(
      remoteRepository("git@github.com:LHagfoss/rustcode.git"),
    );
    expect(remoteRepository(null)).toBeNull();
    expect(remoteRepository("  ")).toBeNull();
  });

  test("a remote that is a path keeps its path, without `.git`", () => {
    expect(remoteRepository("/srv/git/rustcode.git")).toBe("srv/git/rustcode");
    expect(remoteRepository("/srv/git/rustcode.git/")).toBe("srv/git/rustcode");
  });
});

describe("a machine's projects", () => {
  const sessions: SessionSummary[] = [
    session({
      id: "a",
      cwd: "/Users/you/code/rustcode",
      updatedAt: 1000,
      remote: "git@github.com:LHagfoss/rustcode.git",
    }),
    session({ id: "b", cwd: "/Users/you/code/website", updatedAt: 3000 }),
    // A worktree thread belongs to the repository it was cut from, as the sidebar groups it.
    session({
      id: "c",
      cwd: "/Users/you/.config/rustcode/worktrees/rustcode/feat-x",
      updatedAt: 2000,
      worktree: {
        path: "/Users/you/.config/rustcode/worktrees/rustcode/feat-x",
        repo: "/Users/you/code/rustcode",
        branch: "feat-x",
      },
    }),
    session({ id: "d", host: STUDIO, cwd: "/Users/lars/code/rustcode", updatedAt: 4000 }),
  ];

  test("one row per folder, most recently used first, this computer's only", () => {
    const mine = projectsOn(sessions, LOCAL_HOST);
    expect(mine.map((project) => project.path)).toEqual([
      "/Users/you/code/website",
      "/Users/you/code/rustcode",
    ]);
    expect(mine.find((project) => project.name === "rustcode")?.remote).toBe(
      "git@github.com:LHagfoss/rustcode.git",
    );
    // The machine's own list holds its own projects, never this computer's.
    expect(projectsOn(sessions, STUDIO).map((project) => project.path)).toEqual([
      "/Users/lars/code/rustcode",
    ]);
  });

  test("a thread with no folder of its own is not a project", () => {
    // Its machine lists a stand-in for it (its home folder, `/` from a service manager), which made
    // "no project" a place a thread could be started in (issue 3425).
    const standIns: SessionSummary[] = [
      session({ id: "x", cwd: "/Users/you", updatedAt: 5000, workspaceUnknown: true }),
      session({ id: "y", cwd: "/", updatedAt: 4000 }),
      session({ id: "z", cwd: "C:\\", updatedAt: 3000, host: STUDIO }),
      session({ id: "w", cwd: "/Users/you/code/website", updatedAt: 1000 }),
    ];
    expect(projectsOn(standIns).map((project) => project.path)).toEqual(["/Users/you/code/website"]);
    expect(projectsOn(standIns, STUDIO)).toEqual([]);
  });

  test("one folder, however Windows spells it", () => {
    const spellings: SessionSummary[] = [
      session({
        id: "long",
        cwd: "\\\\?\\C:\\Users\\me\\code\\app",
        updatedAt: 2000,
        repo: "\\\\?\\C:\\Users\\me\\code\\app",
      }),
      session({ id: "plain", cwd: "C:\\Users\\me\\code\\app", updatedAt: 1000 }),
    ];
    // The engine strips the prefix; the app does too, for what an older build stored.
    expect(projectsOn(spellings)).toEqual([
      { path: "C:\\Users\\me\\code\\app", name: "app", remote: undefined },
    ]);
  });

  test("a folder is a root when nothing but a drive and separators is left", () => {
    for (const root of ["/", "//", "C:", "C:\\", "\\\\?\\C:\\"]) {
      expect(isRootFolder(root)).toBe(true);
    }
    for (const folder of ["", "  ", "/Users/me", "C:\\Users", "\\\\server\\share"]) {
      expect(isRootFolder(folder)).toBe(false);
    }
  });

  test("the project a folder is in, exactly", () => {
    expect(projectAt(sessions, "/Users/you/code/website")?.name).toBe("website");
    expect(projectAt(sessions, "/Users/you/code")).toBeUndefined();
  });
});

describe("the same project on another machine", () => {
  const local = {
    path: "/Users/you/code/rustcode",
    name: "rustcode",
    remote: "git@github.com:LHagfoss/rustcode.git",
  };

  test("the same path first", () => {
    const match = matchProject(local, [
      { path: "/Users/lars/code/rustcode", name: "rustcode", remote: "https://github.com/LHagfoss/rustcode" },
      { path: local.path, name: "rustcode" },
    ]);
    expect(match?.path).toBe(local.path);
  });

  test("then the same git remote, whatever the folder is called", () => {
    const match = matchProject(local, [
      { path: "/Users/lars/code/notes", name: "notes" },
      {
        path: "/Users/lars/src/rc-main",
        name: "rc-main",
        remote: "https://github.com/lhagfoss/rustcode",
      },
    ]);
    expect(match?.path).toBe("/Users/lars/src/rc-main");
  });

  test("then a folder with the same name, when no remote is known", () => {
    const match = matchProject({ path: "/home/lars/work/website", name: "website" }, [
      { path: "/Users/lars/code/notes", name: "notes" },
      { path: "/Users/lars/code/website", name: "website" },
    ]);
    expect(match?.path).toBe("/Users/lars/code/website");
  });

  test("nothing when the machine has no such project", () => {
    expect(matchProject(local, [{ path: "/Users/lars/code/notes", name: "notes" }])).toBeUndefined();
    expect(matchProject(null, [{ path: "/Users/lars/code/notes", name: "notes" }])).toBeUndefined();
  });
});
