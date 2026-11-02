// Updates, after the SwiftUI app's Updater: a check on launch and every 30
// minutes while the app runs, an "Update to X" row at the foot of the sidebar
// when a newer release is out, and What's New with the release's notes the
// first time a new version starts. The shell does the work
// (src-tauri/src/updater.rs); this keeps what the UI shows.

import { create } from "zustand";
import { openExternal } from "../lib/native";
import { shellCall } from "../lib/shell";
import {
  appVersion,
  canUpdate,
  checkForUpdate,
  type InstallProgress,
  installUpdate,
  isDevBuild,
  type UpdateInfo,
} from "../lib/updater";

export const UPDATE_CHECK_MS = 30 * 60_000;
const SEEN_KEY = "rustcode.seenVersion";
/** Written just before an in-app install: the next launch shows What's New whatever else it knows. */
const UPDATED_FROM_KEY = "rustcode.updatedFrom";

interface UpdatesState {
  info: UpdateInfo | null;
  /** Why the last check failed, for Settings › Engine; null after a good check. */
  error: string | null;
  checkedAt: number | null;
  installing: boolean;
  /** The install's progress, for the sidebar row's bar. */
  progress: InstallProgress | null;
  /** The release notes to show once, after the app was updated. */
  whatsNew: { version: string; notes: string | null } | null;
}

export const useUpdates = create<UpdatesState>(() => ({
  info: null,
  error: null,
  checkedAt: null,
  installing: false,
  progress: null,
  whatsNew: null,
}));

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
 * Whether What's New shows for `current`, given the version last seen and the
 * version an in-app install started from: only after an update, never on a
 * first install and never for a dev build.
 */
export function shouldShowWhatsNew(
  current: string,
  seen: string | null,
  development: boolean,
  updatedFrom: string | null = null,
): boolean {
  if (development) return false;
  if (updatedFrom && newerVersion(current, updatedFrom)) return true;
  return !!seen && newerVersion(current, seen);
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not kept in a private window.
  }
}

/** On launch, from the app's own version (no network): What's New after an update. */
async function whatsNewOnLaunch() {
  const current = await appVersion();
  if (!current) return;
  const seen = read(SEEN_KEY);
  const updatedFrom = read(UPDATED_FROM_KEY);
  write(SEEN_KEY, current);
  write(UPDATED_FROM_KEY, null);
  if (!shouldShowWhatsNew(current, seen, isDevBuild(), updatedFrom)) return;
  useUpdates.setState({ whatsNew: { version: current, notes: null } });
  // The notes follow when GitHub answers; offline the dialog links the release page.
  const notes = await shellCall<string | null>("release_notes", { version: current }).catch(() => null);
  if (useUpdates.getState().whatsNew?.version === current)
    useUpdates.setState({ whatsNew: { version: current, notes } });
}

/** A failed check tries again after this, not after the full interval. */
const RETRY_MS = 60_000;
let retry: ReturnType<typeof setTimeout> | undefined;

async function check() {
  clearTimeout(retry);
  let info: UpdateInfo;
  try {
    info = await checkForUpdate();
  } catch (error) {
    // Offline, GitHub busy, or the shell refused: say so in Settings and try again soon.
    useUpdates.setState({ error: error instanceof Error ? error.message : String(error) });
    retry = setTimeout(() => void check(), RETRY_MS);
    return;
  }
  useUpdates.setState({ info, error: null, checkedAt: Date.now() });
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
  void whatsNewOnLaunch();
  setTimeout(() => void check(), 5_000);
  setInterval(() => void check(), UPDATE_CHECK_MS);
  // Back in front after a while away (a laptop that slept): check again.
  window.addEventListener("focus", () => {
    const last = useUpdates.getState().checkedAt;
    if (!last || Date.now() - last > 10 * 60_000) void check();
  });
}

/** The sidebar row's action: install in place where the app can, else the release page. */
export async function applyUpdate(): Promise<string | null> {
  const info = useUpdates.getState().info;
  if (!info?.available) return null;
  if (!info.canInstall) {
    if (info.page) openExternal(info.page);
    return null;
  }
  useUpdates.setState({ installing: true, progress: null });
  write(UPDATED_FROM_KEY, info.current);
  try {
    await installUpdate((progress) => useUpdates.setState({ progress }));
    return null;
  } catch (error) {
    write(UPDATED_FROM_KEY, null);
    useUpdates.setState({ installing: false, progress: null });
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
