/**
 * Cross-window synchronization for multi-window desktop sessions.
 *
 * Shared settings, pins, snoozes, status marks, permission modes, shortcuts,
 * drafts, and sidebar preferences propagate through `localStorage` storage
 * events. Live thread turn starts/finishes and permission resolutions also
 * synchronize via BroadcastChannel and Tauri events while each window keeps
 * its own active thread selection, view, panel, and Settings state.
 */

import { isTauri } from "../acp/transport";
import type { ThreadStatus } from "../acp/v2";
import { parseChatWidth } from "../lib/chatWidth";
import { parseOverrides } from "../lib/keys";
import { parseNotifySoundTone, parseNotifySoundVolume } from "../lib/notifySound";
import { type AppState, SETTINGS_DEFAULTS, STORAGE } from "./appState";
import { parseAutoCompactSettings } from "./autoCompact";
import { DRAFTS_KEY, parseDrafts } from "./drafts";
import { sanitizeHeaderButtons } from "./headerButtons";
import { linkWaitEnded } from "./host";
import { type PermissionMode, parsePermissionMode } from "./permissions";
import { parseRecapMinutes } from "./recap";
import { parseAppFont, parseTextSize } from "./settingsActions";
import type { SidebarOrder } from "./sidebarOrder";
import { notifySidebarPrefs } from "./sidebarPrefs";
import { applySyncedStorage } from "./storageSync";
import { refreshSessions, updateThread, useApp } from "./store";
import { beginTurn, finishTurn, isRunning } from "./thread";

/** Tauri event emitted when another window answers an in-flight permission prompt. */
export const PERMISSION_RESOLVED_EVENT = "rustcode:permission-resolved";

/** Tauri event emitted when the primary window label changes. */
export const PRIMARY_WINDOW_EVENT = "window:primary";

const CHANNEL_NAME = "rustcode.windows";

/** Messages broadcast across open RustCode windows on the same origin. */
export type WindowSyncMessage =
  | { type: "thread:turn-started"; threadId: string; text: string; at: number }
  | {
      type: "thread:turn-finished";
      threadId: string;
      outcome: "completed" | "cancelled" | "failed";
      at: number;
      failure?: string;
    }
  | { type: "permission:resolved"; rpcId: number | string }
  | { type: "sessions:refresh" };

let currentWindowLabel: string | null = null;
let primaryWindowLabel: string | null = "main";
let channel: BroadcastChannel | null = null;

let roleKnown = !isTauri();
let waitingForPrimary: (() => void)[] = [];

/** Overrides the window role state for unit tests. */
export function setWindowRoleForTests(current: string | null, primary: string | null): void {
  currentWindowLabel = current;
  primaryWindowLabel = primary;
  roleKnown = true;
  waitingForPrimary = [];
}

/**
 * Run `start` in the primary window only: now when this window is primary, else once it becomes
 * primary (the one before it closed). Background loops that talk to every machine start this way,
 * so a second window does not run them twice.
 */
export function whenPrimaryWindow(start: () => void): void {
  if (roleKnown && isPrimaryWindow()) start();
  else waitingForPrimary.push(start);
}

/** The window's role is known or changed: start what waits for the primary window. */
export function primaryWindowChanged(primary: string | null): void {
  if (primary) primaryWindowLabel = primary;
  roleKnown = true;
  if (!isPrimaryWindow()) return;
  const waiting = waitingForPrimary;
  waitingForPrimary = [];
  for (const start of waiting) start();
}

/** Whether this window is currently the primary app window for background side effects. */
export function isPrimaryWindow(): boolean {
  if (!isTauri()) {
    if (currentWindowLabel !== null && primaryWindowLabel !== null) {
      return currentWindowLabel === primaryWindowLabel;
    }
    return true;
  }
  if (!currentWindowLabel || !primaryWindowLabel) return true;
  return currentWindowLabel === primaryWindowLabel;
}

function parseJson<T>(raw: string | null, fallback: T): T {
  if (!raw) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

function parseFlag(raw: string | null, fallback: boolean): boolean {
  return raw === null ? fallback : raw === "1";
}

function parseIntClamped(raw: string | null, fallback: number, min: number, max: number): number {
  const n = raw === null ? Number.NaN : Number.parseInt(raw, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

/**
 * Applies one `localStorage` change made in another window to shared app state.
 * Window-local state (`selectedId`, `view`, `panel`, `settings`, `sidebarHidden`)
 * is intentionally ignored. Returns true when a shared field was updated.
 */
export function applyStorageChange(key: string | null, newValue: string | null): boolean {
  if (!key) return false;
  if (key.startsWith("rustcode.sidebar.")) {
    notifySidebarPrefs();
    return true;
  }

  let patch: Partial<AppState> | null = null;
  switch (key) {
    case STORAGE.theme:
      patch = {
        theme:
          newValue === "light" || newValue === "dark" || newValue === "system"
            ? newValue
            : SETTINGS_DEFAULTS.theme,
      };
      break;
    case STORAGE.themeActive:
      patch = { themeActive: newValue ?? "rustcode" };
      break;
    case STORAGE.font:
      patch = { font: parseAppFont(newValue, null) };
      break;
    case STORAGE.textSize:
      patch = { textSize: parseTextSize(newValue) };
      break;
    case STORAGE.animateStreaming:
      patch = { animateStreaming: parseFlag(newValue, SETTINGS_DEFAULTS.animateStreaming) };
      break;
    case STORAGE.reduceTransparency:
      patch = { reduceTransparency: parseFlag(newValue, SETTINGS_DEFAULTS.reduceTransparency) };
      break;
    case STORAGE.transparency:
      patch = { transparency: parseIntClamped(newValue, SETTINGS_DEFAULTS.transparency, 0, 100) };
      break;
    case STORAGE.chatWidth:
      patch = { chatWidth: parseChatWidth(newValue) };
      break;
    case STORAGE.showToolCalls:
      patch = { showToolCalls: parseFlag(newValue, SETTINGS_DEFAULTS.showToolCalls) };
      break;
    case STORAGE.headerButtons:
      patch = { headerButtons: sanitizeHeaderButtons(parseJson<unknown>(newValue, null)) };
      break;
    case STORAGE.shortcuts:
      patch = { shortcuts: parseOverrides(newValue) };
      break;
    case STORAGE.pinned:
      patch = { pinned: parseJson<string[]>(newValue, []) };
      break;
    case STORAGE.sidebarOrder:
      patch = { sidebarOrder: parseJson<SidebarOrder>(newValue, {}) };
      break;
    case STORAGE.snoozed:
      patch = { snoozed: parseJson<Record<string, number>>(newValue, {}) };
      break;
    case STORAGE.statusMarks:
      patch = { statusMarks: parseJson<Record<string, ThreadStatus>>(newValue, {}) };
      break;
    case STORAGE.permissionMode:
      patch = { defaultPermissionMode: parsePermissionMode(newValue) };
      break;
    case STORAGE.projectPermissionModes:
      patch = { projectPermissionModes: parseJson<Record<string, PermissionMode>>(newValue, {}) };
      break;
    case STORAGE.permissionModes:
      patch = { permissionModes: parseJson<Record<string, PermissionMode>>(newValue, {}) };
      break;
    case STORAGE.systemNotifications:
      patch = { systemNotifications: parseFlag(newValue, SETTINGS_DEFAULTS.systemNotifications) };
      break;
    case STORAGE.notificationsBackgroundOnly:
      patch = {
        notificationsBackgroundOnly: parseFlag(newValue, SETTINGS_DEFAULTS.notificationsBackgroundOnly),
      };
      break;
    case STORAGE.notificationSound:
      patch = { notificationSound: parseFlag(newValue, SETTINGS_DEFAULTS.notificationSound) };
      break;
    case STORAGE.notificationSoundTone:
      patch = { notificationSoundTone: parseNotifySoundTone(newValue) };
      break;
    case STORAGE.notificationSoundVolume:
      patch = { notificationSoundVolume: parseNotifySoundVolume(newValue) };
      break;
    case STORAGE.chatRecaps:
      patch = { chatRecaps: parseFlag(newValue, SETTINGS_DEFAULTS.chatRecaps) };
      break;
    case STORAGE.recapMinutes:
      patch = { recapMinutes: parseRecapMinutes(newValue) };
      break;
    case STORAGE.continueAfterLimit:
      patch = { continueAfterLimit: parseFlag(newValue, SETTINGS_DEFAULTS.continueAfterLimit) };
      break;
    case STORAGE.resumeAfterRestart:
      patch = { resumeAfterRestart: parseFlag(newValue, SETTINGS_DEFAULTS.resumeAfterRestart) };
      break;
    case STORAGE.autoCompact:
      patch = { autoCompact: parseAutoCompactSettings(newValue) };
      break;
    case STORAGE.autoCompactOff:
      patch = { autoCompactOff: parseJson<Record<string, boolean>>(newValue, {}) };
      break;
    case STORAGE.project:
      patch = { defaultCwd: newValue ?? "" };
      break;
    case DRAFTS_KEY:
      patch = { drafts: parseDrafts(newValue) };
      break;
    default:
      return applySyncedStorage(key, newValue);
  }

  useApp.setState(patch);
  return true;
}

/** Removes a permission request that another window already answered. */
export function dismissResolvedPermission(rpcId: number | string): boolean {
  const threads = useApp.getState().threads;
  let removed = false;
  for (const [id, thread] of Object.entries(threads)) {
    if (!thread.permissions.some((p) => p.rpcId === rpcId)) continue;
    removed = true;
    updateThread(id, (t) => ({
      ...t,
      permissions: t.permissions.filter((p) => p.rpcId !== rpcId),
    }));
    if (!useApp.getState().threads[id]?.permissions.length) linkWaitEnded(id);
  }
  return removed;
}

/** Applies a cross-window message received from another open RustCode window. */
export function applyWindowMessage(message: WindowSyncMessage): void {
  switch (message.type) {
    case "thread:turn-started": {
      const thread = useApp.getState().threads[message.threadId];
      if (!thread || isRunning(thread)) return;
      updateThread(message.threadId, (t) => beginTurn(t, message.text, message.at));
      return;
    }
    case "thread:turn-finished": {
      const thread = useApp.getState().threads[message.threadId];
      if (!thread || !isRunning(thread)) return;
      updateThread(message.threadId, (t) => finishTurn(t, message.outcome, message.at, message.failure));
      void refreshSessions({ quiet: true });
      return;
    }
    case "permission:resolved":
      dismissResolvedPermission(message.rpcId);
      return;
    case "sessions:refresh":
      void refreshSessions({ quiet: true });
      return;
  }
}

function post(message: WindowSyncMessage): void {
  try {
    channel?.postMessage(message);
  } catch {
    return;
  }
}

/** Notifies sibling windows that a user turn started on `threadId`. */
export function broadcastTurnStarted(threadId: string, text: string, at: number): void {
  post({ type: "thread:turn-started", threadId, text, at });
}

/** Notifies sibling windows that a turn finished on `threadId`. */
export function broadcastTurnFinished(
  threadId: string,
  outcome: "completed" | "cancelled" | "failed",
  at: number,
  failure?: string,
): void {
  post({ type: "thread:turn-finished", threadId, outcome, at, failure });
}

/** Notifies sibling windows to refresh the session list quietly. */
export function broadcastSessionsRefresh(): void {
  post({ type: "sessions:refresh" });
}

/** Starts cross-window storage, BroadcastChannel, and Tauri event listeners. */
export function startWindowSync(): () => void {
  const cleanups: (() => void)[] = [];

  if (typeof window !== "undefined") {
    const onStorage = (event: StorageEvent) => {
      applyStorageChange(event.key, event.newValue);
    };
    window.addEventListener("storage", onStorage);
    cleanups.push(() => window.removeEventListener("storage", onStorage));
  }

  if (typeof BroadcastChannel !== "undefined") {
    try {
      const bc = new BroadcastChannel(CHANNEL_NAME);
      channel = bc;
      bc.onmessage = (event: MessageEvent<WindowSyncMessage>) => {
        if (event.data && typeof event.data === "object" && "type" in event.data) {
          applyWindowMessage(event.data);
        }
      };
      cleanups.push(() => {
        bc.close();
        if (channel === bc) channel = null;
      });
    } catch {
      channel = null;
    }
  }

  if (isTauri()) {
    let live = true;
    const tauriCleanups: (() => void)[] = [];
    void (async () => {
      try {
        const [{ getCurrentWindow }, { listen }, { invoke }] = await Promise.all([
          import("@tauri-apps/api/window"),
          import("@tauri-apps/api/event"),
          import("@tauri-apps/api/core"),
        ]);
        if (!live) return;
        currentWindowLabel = getCurrentWindow().label;
        const primary = await invoke<string | null>("primary_window_label").catch(() => null);
        primaryWindowChanged(primary);

        const stopPrimary = await listen<string>(PRIMARY_WINDOW_EVENT, (event) => {
          if (typeof event.payload === "string" && event.payload) primaryWindowChanged(event.payload);
        });
        const stopPerm = await listen<number | string>(PERMISSION_RESOLVED_EVENT, (event) => {
          if (typeof event.payload === "number" || typeof event.payload === "string") {
            dismissResolvedPermission(event.payload);
          }
        });
        if (!live) {
          stopPrimary();
          stopPerm();
          return;
        }
        tauriCleanups.push(stopPrimary, stopPerm);
      } catch {
        primaryWindowChanged(null);
      }
    })();
    cleanups.push(() => {
      live = false;
      for (const stop of tauriCleanups) stop();
    });
  }

  return () => {
    for (const stop of cleanups) stop();
  };
}
