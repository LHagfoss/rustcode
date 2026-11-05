// The Changes panel draws only the read made for the folder it is showing
// (ui/ChangesPanel.tsx): a slow answer for the thread before it must not
// appear under the new thread's name.

import { describe, expect, test } from "bun:test";
import { bodyWithoutChanges, changesForFolder } from "./ChangesPanel";

const read = (folder: string) => ({ folder, changes: { files: [], diff: "" }, at: 1 });

describe("changes belong to their folder", () => {
  test("the read for the folder on screen is drawn", () => {
    const mine = read("/tmp/one");
    expect(changesForFolder(mine, "/tmp/one")).toBe(mine);
  });

  test("a read for the previous thread's folder is not", () => {
    expect(changesForFolder(read("/tmp/one"), "/tmp/two")).toBeNull();
    expect(changesForFolder(read("/tmp/one"), undefined)).toBeNull();
    expect(changesForFolder(null, "/tmp/one")).toBeNull();
  });
});

describe("what the body says while it has no changes", () => {
  test("a read in flight is reading, even while the turn runs", () => {
    expect(bodyWithoutChanges({ loading: true, running: true })).toBe("loading");
    expect(bodyWithoutChanges({ loading: true, running: false })).toBe("loading");
  });

  test("a running turn waits for it to end before reading", () => {
    expect(bodyWithoutChanges({ loading: false, running: true })).toBe("waiting");
  });

  test("idle with nothing in flight has nothing to say", () => {
    expect(bodyWithoutChanges({ loading: false, running: false })).toBeNull();
  });
});
