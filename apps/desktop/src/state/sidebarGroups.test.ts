// The sidebar's project folders without a window: one flat list of every computer's threads, and
// with "Group by project" on, a folder per project and computer.

import { expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { hostOf, sidebarProjects } from "./sidebarGroups";
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

test("projects keep each computer's threads apart and group a worktree under its repository", () => {
  const worktree = { ...linuxOther, worktree: { repo: linux.cwd, path: "/tmp/tree", branch: "task" } };
  const groups = sidebarProjects([local, linux, worktree, studio]);
  expect(groups).toHaveLength(3);
  expect(groups[0]?.items).toEqual([local]);
  expect(groups[1]?.items).toEqual([linux, worktree]);
  expect(groups[2]?.items).toEqual([studio]);
  // Same folder name on two computers: two folders, with distinct keys.
  expect(groups[2]?.key).not.toBe(groups[0]?.key);
  expect(groups[0]?.key).toBe(local.cwd);
});

test("only a folder on this computer can start a thread", () => {
  const groups = sidebarProjects([local, linux]);
  expect(groups[0]?.project).toBe(local.cwd);
  expect(groups[1]?.project).toBeUndefined();
  expect(sidebarProjects([])).toEqual([]);
});
