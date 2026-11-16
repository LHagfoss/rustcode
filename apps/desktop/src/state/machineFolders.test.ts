import { describe, expect, test } from "bun:test";
import { childPath, folderNameProblem, parseListing } from "./machineFolders";

describe("a paired machine's folders", () => {
  test("a listing reads the engine's answer and drops malformed rows", () => {
    const listing = parseListing({
      path: "/home/me",
      parent: "/home",
      home: "/home/me",
      is_git: false,
      folders: [
        { name: "code", path: "/home/me/code", is_git: false },
        { name: "api", path: "/home/me/api", is_git: true },
        { name: "broken" },
      ],
      truncated: true,
    });
    expect(listing.path).toBe("/home/me");
    expect(listing.parent).toBe("/home");
    expect(listing.folders).toEqual([
      { name: "code", path: "/home/me/code", isGit: false },
      { name: "api", path: "/home/me/api", isGit: true },
    ]);
    expect(listing.truncated).toBe(true);
  });

  test("the root has no parent", () => {
    expect(parseListing({ path: "/", parent: null, folders: [] }).parent).toBeNull();
  });

  test("a new folder's name is one folder name", () => {
    expect(folderNameProblem("api")).toBeNull();
    expect(folderNameProblem("  ")).toBe("Type a name");
    expect(folderNameProblem("..")).not.toBeNull();
    expect(folderNameProblem("a/b")).not.toBeNull();
    expect(folderNameProblem("a\\b")).not.toBeNull();
    expect(folderNameProblem("x".repeat(121))).not.toBeNull();
  });

  test("a child path keeps the parent's separator", () => {
    expect(childPath("/home/me/", " api ")).toBe("/home/me/api");
    expect(childPath("/", "srv")).toBe("/srv");
    expect(childPath("C:\\Users\\me", "api")).toBe("C:\\Users\\me\\api");
  });
});
