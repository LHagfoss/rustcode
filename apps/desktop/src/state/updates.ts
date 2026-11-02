// Updates, after the SwiftUI app's Updater: a check on launch and every 30
// minutes while the app runs, an "Update to X" row at the foot of the sidebar
// when a newer release is out, and What's New with the release's notes the
// first time a new version starts. The shell does the work
// (src-tauri/src/updater.rs); this keeps what the UI shows.

import { create } from "zustand";
import { openExternal } from "../lib/native";
import { shellCall } from "../lib/shell";
import { canUpdate, checkForUpdate, installUpdate, type UpdateInfo } from "../lib/updater";

export const UPDATE_CHECK_MS = 30 * 60_000;
const SEEN_KEY = "rustcode.seenVersion";

interface UpdatesState {
  info: UpdateInfo | null;
  installing: boolean;
  /** The release notes to show once, after the app was updated. */
  whatsNew: { version: string; notes: string | null } | null;
}

export const useUpdates = create<UpdatesState>(() => ({ info: null, installing: false, whatsNew: null }));

/** `0.116.0` is newer than `0.115.2`; prerelease suffixes compare as their base. */
export function newerVersion(a: string, b: string): boolean {
  const parts = (v: string) =>
    v
      .replace(/^v/, "")
      .split("-")[0]!
      .split(".")
      .map((n) => Number.parseInt(n, 10) || 0);
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0);
  }
  return false;
}

/**
 * Whether What's New shows for `current`, given the version last seen: only
 * after an update, never on a first install and never for a dev build.
 */
export function shouldShowWhatsNew(current: string, seen: string | null, development: boolean): boolean {
  return !development && !!seen && newerVersion(current, seen);
}

function readSeen(): string | null {
  try {
    return localStorage.getItem(SEEN_KEY);
  } catch {
    return null;
  }
}

function writeSeen(version: string) {
  try {
    localStorage.setItem(SEEN_KEY, version);
  } catch {
    // Not kept in a private window.
  }
}

async function check(first: boolean) {
  let info: UpdateInfo;
  try {
    info = await checkForUpdate();
  } catch {
    return; // Offline or GitHub busy: the next check tries again.
  }
  useUpdates.setState({ info });
  if (!first) return;
  const seen = readSeen();
  writeSeen(info.current);
  if (shouldShowWhatsNew(info.current, seen, info.development)) {
    const notes = await shellCall<string | null>("release_notes", { version: info.current }).catch(
      () => null,
    );
    useUpdates.setState({ whatsNew: { version: info.current, notes } });
  }
}

let started = false;

/** Starts the checks once per app run; the browser UI has nothing to update. */
export function startUpdateChecks() {
  // `?updatePreview` in a dev browser: the row and What's New with sample data, for screenshots.
  if (import.meta.env.DEV && !canUpdate() && new URLSearchParams(location.search).has("updatePreview")) {
    useUpdates.setState({
      info: {
        current: "0.115.0",
        latest: "0.116.0",
        available: true,
        canInstall: true,
        development: false,
        page: null,
      },
      whatsNew: {
        version: "0.116.0",
        notes:
          "## Desktop\n- The sidebar shows a live clock for running threads and Pin/Settle on hover.\n- ⌘O changes the folder of a thread; ⌘⇧O the default.\n- Reports like `/status` open in a panel above the composer.\n\n## Engine\n- The agent edits in place and no longer branches or commits unless asked.",
      },
    });
    return;
  }
  if (started || !canUpdate()) return;
  started = true;
  setTimeout(() => void check(true), 5_000);
  setInterval(() => void check(false), UPDATE_CHECK_MS);
}

/** The sidebar row's action: install in place where the app can, else the release page. */
export async function applyUpdate(): Promise<string | null> {
  const info = useUpdates.getState().info;
  if (!info?.available) return null;
  if (!info.canInstall) {
    if (info.page) openExternal(info.page);
    return null;
  }
  useUpdates.setState({ installing: true });
  try {
    await installUpdate();
    return null;
  } catch (error) {
    useUpdates.setState({ installing: false });
    return error instanceof Error ? error.message : String(error);
  }
}

export function closeWhatsNew() {
  useUpdates.setState({ whatsNew: null });
}

/** What's New for the running version on demand (Settings › Engine). */
export async function showWhatsNew(version: string) {
  const notes = await shellCall<string | null>("release_notes", { version }).catch(() => null);
  useUpdates.setState({ whatsNew: { version, notes } });
}
