// The sidebar's machine groups without a window: one heading per paired computer, the threads by
// project inside, and the rule that only the Threads list keeps a heading for a machine with no
// threads yet (Working and Pinned would otherwise draw a heading with nothing under it).

import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { type GroupedMachine, machineGroups } from "./sidebarGroups";
import { LOCAL_HOST } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const STUDIO = "gw-8e1f4b7a5c2d4e6f";

function mac(id: string, name: string, state: GroupedMachine["state"] = "connected"): GroupedMachine {
  return { id, name, state, retryInSecs: null };
}

function thread(id: string, cwd: string, host?: string): SessionSummary {
  return { id: host ? `${host}/${id}` : id, host, cwd, title: id };
}

const local = thread("local-1", "/Users/you/code/notes");
const linux = thread("mock-linux-1", "/home/lars/code/rustcode", LINUX);
const linuxOther = thread("mock-linux-2", "/home/lars/code/site", LINUX);
const studio = thread("mock-studio-1", "/Users/lars/code/notes", STUDIO);

describe("the sidebar's machine groups", () => {
  test("Working keeps no heading for a machine with no rows", () => {
    const groups = machineGroups("working", [local], [mac(LINUX, "Lars's Linux box"), mac(STUDIO, "Studio")]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.machine?.id).toBe(LOCAL_HOST);
    expect(groups[0]?.machine?.name).toBe("This computer");
    expect(groups[0]?.items).toEqual([local]);
    // No empty "Lars's Linux box" or "Studio" heading above nothing.
    expect(groups.some((group) => group.machine?.id === LINUX || group.machine?.id === STUDIO)).toBe(false);
  });

  test("Pinned keeps no heading for a machine with no rows either", () => {
    const groups = machineGroups(
      "pinned",
      [local, linux],
      [mac(LINUX, "Lars's Linux box"), mac(STUDIO, "Studio")],
    );
    expect(groups.map((group) => group.machine?.id)).toEqual([LOCAL_HOST, LINUX]);
  });

  test("Threads keeps a heading for a machine that has listed no thread yet", () => {
    const groups = machineGroups(
      "threads",
      [local],
      [mac(LINUX, "Lars's Linux box", "offline"), mac(STUDIO, "Studio")],
    );
    expect(groups.map((group) => group.machine?.id)).toEqual([LOCAL_HOST, LINUX, STUDIO]);
    const idle = groups[1]!;
    expect(idle.items).toEqual([]);
    expect(idle.only).toBe(true);
    expect(idle.first).toBe(true);
    expect(idle.machine?.state).toBe("offline");
  });

  test("this computer comes first, then the saved machines, each with its projects inside", () => {
    const groups = machineGroups(
      "threads",
      [linux, local, linuxOther, studio],
      [mac(LINUX, "Lars's Linux box"), mac(STUDIO, "Studio")],
    );
    expect(groups.map((group) => group.machine?.id)).toEqual([LOCAL_HOST, LINUX, LINUX, STUDIO]);
    expect(groups.map((group) => group.label)).toEqual(["notes", "rustcode", "site", "notes"]);
    // The first project of a machine draws its heading, and only it.
    expect(groups.map((group) => group.first)).toEqual([true, true, false, true]);
    expect(groups.map((group) => group.only)).toEqual([true, false, false, true]);
    expect(groups[1]?.items).toEqual([linux]);
    expect(groups[2]?.items).toEqual([linuxOther]);
  });

  test("each group's key names its machine and project, so two machines' folders never share one", () => {
    const groups = machineGroups(
      "threads",
      [linux, studio],
      [mac(LINUX, "Lars's Linux box"), mac(STUDIO, "Studio")],
    );
    expect(groups.map((group) => group.key)).toEqual([
      `${LINUX}\u0000/home/lars/code/rustcode`,
      `${STUDIO}\u0000/Users/lars/code/notes`,
    ]);
  });

  test("a machine's worktree thread belongs to its repository, not the worktree folder", () => {
    const inWorktree: SessionSummary = {
      ...linux,
      id: `${LINUX}/mock-linux-3`,
      cwd: "/home/lars/code/.worktrees/threads",
      worktree: {
        path: "/home/lars/code/.worktrees/threads",
        branch: "feat/x",
        repo: "/home/lars/code/rustcode",
      },
    };
    const groups = machineGroups("threads", [linux, inWorktree], [mac(LINUX, "Lars's Linux box")]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.items).toEqual([linux, inWorktree]);
  });

  test("without a paired machine there are no machine groups at all", () => {
    expect(machineGroups("threads", [local], [])).toEqual([
      {
        key: `${LOCAL_HOST}\u0000/Users/you/code/notes`,
        label: "notes",
        items: [local],
        machine: { id: LOCAL_HOST, name: "This computer", state: "connected", retryInSecs: null },
        first: true,
        only: true,
      },
    ]);
  });

  test("a thread whose machine was just removed keeps a group of its own", () => {
    const orphan = thread("mock-gone-1", "/tmp/gone", "gw-gone");
    const groups = machineGroups("threads", [local, orphan], []);
    expect(groups.map((group) => group.machine?.id)).toEqual([LOCAL_HOST, "gw-gone"]);
    // It is not drawn as this computer's thread, and it does not vanish from the list.
    expect(groups[1]?.machine?.name).toBe("Another computer");
    expect(groups[1]?.items).toEqual([orphan]);
  });
});
