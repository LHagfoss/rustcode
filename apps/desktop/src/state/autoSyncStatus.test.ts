import { beforeEach, describe, expect, test } from "bun:test";
import { autoSyncLine, errorDetail, recordAutoSync, useAutoSync } from "./autoSyncStatus";

beforeEach(() => useAutoSync.setState({ byMachine: {} }));

describe("automatic sync status", () => {
  test("keeps the last outcome of each kind per machine", () => {
    recordAutoSync("studio", "follow", "synced", "2 settings changed.", 1_000);
    recordAutoSync("studio", "backfill", "skipped", "Its server is too old to copy threads.", 2_000);
    recordAutoSync("studio", "follow", "failed", "timed out", 3_000);
    expect(useAutoSync.getState().byMachine).toEqual({
      studio: {
        follow: { outcome: "failed", at: 3_000, detail: "timed out" },
        backfill: { outcome: "skipped", at: 2_000, detail: "Its server is too old to copy threads." },
      },
    });
  });

  test("says what happened and when", () => {
    const now = 10 * 60_000;
    expect(autoSyncLine(undefined, now)).toBeNull();
    expect(autoSyncLine({ outcome: "synced", at: now - 5_000 }, now)).toBe("Synced just now");
    expect(autoSyncLine({ outcome: "synced", at: now - 3 * 60_000, detail: "1 setting changed." }, now)).toBe(
      "Synced 3 min ago. 1 setting changed.",
    );
    expect(autoSyncLine({ outcome: "skipped", at: now - 2 * 3_600_000, detail: "Away." }, now + 0)).toBe(
      "Skipped 2 h ago: Away.",
    );
    expect(autoSyncLine({ outcome: "failed", at: now, detail: "timed out" }, now)).toBe(
      "Failed just now: timed out",
    );
  });

  test("reads an error as its message", () => {
    expect(errorDetail(new Error("refused"))).toBe("refused");
    expect(errorDetail("gone")).toBe("gone");
  });
});
