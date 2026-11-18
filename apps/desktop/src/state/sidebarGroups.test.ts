// The sidebar's project folders without a window: one flat list of every computer's threads, and
// with "Group by project" on, matching folder names share a group.

import { expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import {
  byProjectName,
  filteredNeedsCount,
  hostOf,
  rowWhere,
  sidebarProjects,
  spansMachines,
  threadFolder,
} from "./sidebarGroups";
import { LOCAL_HOST } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const STUDIO = "gw-8e1f4b7a5c2d4e6f";

function thread(id: string, cwd: string, host?: string): SessionSummary {
  return { id: host ? `${host}/${id}` : id, host, cwd, title: id };
}

const local = thread("local-1", "/Users/you/code/notes");
const linux = thread("mock-linux-1", "/home/lars/code/rustcode", LINUX);
const linuxOther = thread("mock-linux-2", "/home/lars/code/site", LINUX);
const studio = thread("mock-studio-1", "/Users/lars/code/notes", STUDIO);

test("a thread without a host runs on this computer", () => {
  expect(hostOf(local)).toBe(LOCAL_HOST);
  expect(hostOf(linux)).toBe(LINUX);
});

test("projects group matching names across computers and worktrees under their repository", () => {
  const worktree = { ...linuxOther, worktree: { repo: linux.cwd, path: "/tmp/tree", branch: "task" } };
  const groups = sidebarProjects([local, linux, worktree, studio]);
  expect(groups).toHaveLength(2);
  expect(groups[0]?.items).toEqual([local, studio]);
  expect(groups[1]?.items).toEqual([linux, worktree]);
  expect(groups[0]?.project).toBe(local.cwd);
  expect(sidebarProjects([studio, local])[0]?.project).toBe(local.cwd);
});

test("only a folder on this computer can start a thread", () => {
  const groups = sidebarProjects([local, linux]);
  expect(groups[0]?.project).toBe(local.cwd);
  expect(groups[1]?.project).toBeUndefined();
  expect(sidebarProjects([])).toEqual([]);
});

test("folders are A to Z by name, not by which thread changed last", () => {
  const site = thread("s", "/Users/you/code/Site");
  const app = thread("a", "/Users/you/code/app");
  const names = byProjectName(sidebarProjects([site, local, app])).map((g) => g.label);
  expect(names).toEqual(["app", "notes", "Site"]);
});

test("machine tags follow the filtered rows, not the paired machine inventory", () => {
  expect(spansMachines([])).toBe(false);
  expect(spansMachines([local])).toBe(false);
  expect(spansMachines([linux, linuxOther])).toBe(false);
  expect(spansMachines([local, linux])).toBe(true);
  expect(spansMachines([linux, studio])).toBe(true);
});

test("working folder uses the worktree, never its repository", () => {
  expect(threadFolder(local)).toBe(local.cwd!);
  expect(threadFolder({ ...local, worktree: { path: "/tmp/task", repo: local.cwd!, branch: "task" } })).toBe(
    "/tmp/task",
  );
  expect(threadFolder({ cwd: "" })).toBe("");
});

test("needs-you count includes only filtered threads and counts each once", () => {
  const items = [{ threadId: local.id }, { threadId: local.id }, { threadId: linux.id }];
  expect(filteredNeedsCount(items, new Set([local.id]))).toBe(1);
  expect(filteredNeedsCount(items, new Set([local.id, linux.id]))).toBe(2);
  expect(filteredNeedsCount(items, new Set())).toBe(0);
});

test("a row says where it runs by branch, then subfolder, and nothing its heading says", () => {
  const repo = "/Users/you/code/app";
  const at = (cwd: string, extra: Partial<SessionSummary> = {}) => ({ cwd, repo, ...extra });
  const wt = { path: "/Users/you/.rustcode/worktrees/app-login", branch: "fix/login", repo };
  // A worktree names its branch whatever the checkout is on.
  expect(rowWhere({ session: at(wt.path, { worktree: wt }), grouped: true })).toEqual({
    kind: "worktree",
    text: "fix/login",
  });
  // A detached worktree names its folder.
  expect(rowWhere({ session: at(wt.path, { worktree: { ...wt, branch: "" } }), grouped: true }).kind).toBe(
    "worktree",
  );
  // The checkout off its default branch names the branch; on it, it says nothing.
  const off = { session: at(repo), branch: "feat/x", defaultBranch: "main", grouped: true };
  expect(rowWhere(off)).toEqual({ kind: "branch", text: "feat/x" });
  expect(rowWhere({ ...off, branch: "main" })).toEqual({ kind: "none" });
  // An unread branch is not guessed at.
  expect(rowWhere({ session: at(repo), grouped: true })).toEqual({ kind: "none" });
  // A subfolder is named from the project's root, and a sibling folder is not taken for one.
  expect(rowWhere({ session: at(`${repo}/apps/web`), grouped: true })).toEqual({
    kind: "folder",
    text: "apps/web",
  });
  expect(rowWhere({ session: at(`${repo}-old`), grouped: true })).toEqual({ kind: "none" });
  // The flat list has no heading, so the project leads.
  expect(rowWhere({ ...off, grouped: false })).toEqual({ kind: "branch", text: "app · feat/x" });
  expect(rowWhere({ session: at(repo), grouped: false })).toEqual({ kind: "folder", text: "app" });
  expect(rowWhere({ session: { cwd: "" }, grouped: true })).toEqual({ kind: "folder", text: "No folder" });
});
