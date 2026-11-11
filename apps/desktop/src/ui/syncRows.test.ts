import { describe, expect, test } from "bun:test";
import { SETTINGS_INDEX } from "./settingsIndex";
import { SYNC_COLUMNS, syncColumn, syncRows } from "./syncRows";

describe("what a machine's settings page says syncs", () => {
  test("maps the three registry kinds to the three columns", () => {
    expect(syncColumn("server")).toBe("shared");
    expect(syncColumn("machine")).toBe("machine");
    expect(syncColumn("app")).toBe("app");
    expect(SYNC_COLUMNS.map((column) => column.column)).toEqual(["shared", "machine", "app"]);
  });

  test("every Settings row appears once, in the page order the index has", () => {
    const rows = syncRows();
    expect(rows).toHaveLength(SETTINGS_INDEX.length);
    expect(rows.map((row) => row.title)).toEqual(SETTINGS_INDEX.map((entry) => entry.title));
    // Every column has rows, so the table is never one empty third.
    for (const { column } of SYNC_COLUMNS) {
      expect(rows.some((row) => row.column === column)).toBe(true);
    }
  });

  test("a row's own kind wins over its page's, e.g. the default folder is machine-only", () => {
    const rows = syncRows();
    expect(rows.find((row) => row.title === "Default project folder")?.column).toBe("machine");
    expect(rows.find((row) => row.title === "Default access")?.column).toBe("shared");
    expect(rows.find((row) => row.title === "System notifications")?.column).toBe("app");
  });
});
