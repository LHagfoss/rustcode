// What a machine's settings page says about every Settings row: shared with
// the other machines, kept on that machine only, or this app's own. Built from
// the one registry and the one index, never hand-written, so a new row cannot
// be missing from the table (docs/desktop.md › Settings on one machine or all).

import { rowKind, type SettingKind } from "../state/settingsRegistry";
import { SETTINGS_INDEX, type SettingEntry } from "./settingsIndex";

/** The three columns the table shows, in the order it shows them. */
export type SyncColumn = "shared" | "machine" | "app";

export const SYNC_COLUMNS: { column: SyncColumn; label: string }[] = [
  { column: "shared", label: "Shared" },
  { column: "machine", label: "This machine only" },
  { column: "app", label: "This app only" },
];

/** The column a registry kind belongs in. */
export function syncColumn(kind: SettingKind): SyncColumn {
  return kind === "server" ? "shared" : kind === "machine" ? "machine" : "app";
}

export interface SyncRow {
  section: SettingEntry["section"];
  title: string;
  column: SyncColumn;
}

/** Every Settings row with the column it belongs in, in page order. */
export function syncRows(entries: readonly SettingEntry[] = SETTINGS_INDEX): SyncRow[] {
  return entries.map((entry) => ({
    section: entry.section,
    title: entry.title,
    column: syncColumn(rowKind(entry.section, entry.title)),
  }));
}
