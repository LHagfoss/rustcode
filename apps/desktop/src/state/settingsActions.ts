// Settings actions on the app store; read it only when called to keep module loading safe.

import type { ChatWidth } from "../lib/chatWidth";
import {
  APP_COMMANDS,
  type AppCommand,
  assignBinding,
  bindingOf,
  commandFor,
  commandHint,
  type KeyEventLike,
} from "../lib/keys";
import { clampNotifySoundVolume, type NotifySoundTone, parseNotifySoundTone } from "../lib/notifySound";
import type { AppState } from "./appState";
import { SETTINGS_DEFAULTS, STORAGE, store, stored } from "./appState";
import { HEADER_BUTTON_IDS, type HeaderButtonId } from "./headerButtons";
import { wakeTick } from "./host";
import { parseRules } from "./limitFallback";
import {
  MAX_DELEGATES_PER_THREAD,
  MAX_DELEGATES_TOTAL,
  MIN_DELEGATES_PER_THREAD,
  MIN_DELEGATES_TOTAL,
  WAKE_BATCH_MAX_S,
} from "./orchestration";
import { type OpenRecap, parseRecapMinutes, pruneSeen, recapAfterMessages } from "./recap";
import { useApp } from "./store";

const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((state: AppState) => Partial<AppState>)) => useApp.setState(patch);

// ---------------------------------------------------------------- subagents and usage

export function setResumeAfterRestart(on: boolean) {
  set({ resumeAfterRestart: on });
  store(STORAGE.resumeAfterRestart, on ? "1" : "0");
}

export function setSystemNotifications(on: boolean) {
  set({ systemNotifications: on });
  store(STORAGE.systemNotifications, on ? "1" : "0");
}

export function setNotificationsBackgroundOnly(on: boolean) {
  set({ notificationsBackgroundOnly: on });
  store(STORAGE.notificationsBackgroundOnly, on ? "1" : "0");
}

export function setNotificationSound(on: boolean) {
  set({ notificationSound: on });
  store(STORAGE.notificationSound, on ? "1" : "0");
}

export function setNotificationSoundTone(tone: NotifySoundTone) {
  const kept = parseNotifySoundTone(tone);
  set({ notificationSoundTone: kept });
  store(STORAGE.notificationSoundTone, kept);
}

export function setNotificationSoundVolume(volume: number) {
  const kept = clampNotifySoundVolume(volume);
  set({ notificationSoundVolume: kept });
  store(STORAGE.notificationSoundVolume, String(kept));
}

export function setChatRecaps(on: boolean) {
  set({ chatRecaps: on, ...(on ? {} : { recaps: {} }) });
  store(STORAGE.chatRecaps, on ? "1" : "0");
}

export function setRecapMinutes(minutes: number) {
  const kept = parseRecapMinutes(String(minutes));
  set({ recapMinutes: kept });
  store(STORAGE.recapMinutes, String(kept));
}

/** A return to a chat after a long enough quiet: its recap, from when it went quiet. */
export function openChatRecap(threadId: string, since: number, messages?: number) {
  set((s) => ({ recaps: { ...s.recaps, [threadId]: { since, messages } } }));
}

/** Puts a chat's recap away until the next return after a long enough quiet. */
export function dismissChatRecap(threadId: string) {
  set((s) => {
    if (!s.recaps[threadId]) return s;
    const { [threadId]: _gone, ...rest } = s.recaps;
    return { recaps: rest };
  });
}

/** Counts each open recap's messages once its chat has loaded, and closes one when more arrive. */
export function settleRecaps(counts: (threadId: string) => number | undefined) {
  set((s) => {
    let next: Record<string, OpenRecap> | undefined;
    for (const [id, recap] of Object.entries(s.recaps)) {
      const count = counts(id);
      const kept = recapAfterMessages(recap, count);
      if (kept === recap) continue;
      next ??= { ...s.recaps };
      if (kept) next[id] = kept;
      else delete next[id];
    }
    return next ? { recaps: next } : s;
  });
}

/** When each chat was last on screen, from app storage. */
export function threadSeenAt(): Record<string, number> {
  const raw = stored(STORAGE.threadSeen);
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    return pruneSeen(parsed as Record<string, number>);
  } catch {
    return {};
  }
}

/** Records when each chat was last on screen. */
export function storeThreadSeen(seen: Readonly<Record<string, number>>) {
  store(STORAGE.threadSeen, JSON.stringify(pruneSeen(seen)));
}

export type AppFont = "inter" | "system" | "rounded";
export type TextSize = "small" | "medium" | "large";

/** The saved font; before the Font setting, the Rounded font switch was the only choice. */
export function parseAppFont(saved: string | null, legacyRounded: string | null): AppFont {
  if (saved === "inter" || saved === "system" || saved === "rounded") return saved;
  return legacyRounded === "1" ? "rounded" : SETTINGS_DEFAULTS.font;
}

export function parseTextSize(saved: string | null): TextSize {
  return saved === "small" || saved === "medium" || saved === "large" ? saved : SETTINGS_DEFAULTS.textSize;
}

export function setFont(font: AppFont) {
  set({ font });
  store(STORAGE.font, font);
}

export function setTextSize(textSize: TextSize) {
  set({ textSize });
  store(STORAGE.textSize, textSize);
}

/** A command's shortcut as the app shows it ("⌘B"), or "" when it has none. */
export function shortcutHint(id: string): string {
  const s = get();
  return commandHint(
    bindingOf(s.shortcuts, id),
    APP_COMMANDS.find((c) => c.id === id),
  );
}

/** The command a keydown runs, if any (state/shortcuts.ts runs it). */
export function shortcutFor(event: KeyEventLike): string | null {
  return commandFor(get().shortcuts, event);
}

/** What a command is bound to now, for a component that must follow a remap. */
export function useShortcutHint(id: string): string {
  return useApp((s) =>
    commandHint(
      bindingOf(s.shortcuts, id),
      APP_COMMANDS.find((c) => c.id === id),
    ),
  );
}

/**
 * Rebinds a command; the caller has already checked the binding (lib/keys.ts). A command that
 * answered the key loses it, and is returned so the page can say where the key came from.
 */
export function setShortcut(id: string, binding: string): AppCommand[] {
  const { overrides, taken } = assignBinding(get().shortcuts, id, binding);
  set({ shortcuts: overrides });
  store(STORAGE.shortcuts, JSON.stringify(overrides));
  return taken;
}

/** Drops a command's override: its default binding is back, taken from whoever had it since. */
export function resetShortcut(id: string): AppCommand[] {
  const own = APP_COMMANDS.find((command) => command.id === id);
  if (own?.binding) return setShortcut(id, own.binding);
  const next = { ...get().shortcuts };
  delete next[id];
  set({ shortcuts: next });
  store(STORAGE.shortcuts, JSON.stringify(next));
  return [];
}

export function setAnimateStreaming(on: boolean) {
  set({ animateStreaming: on });
  store(STORAGE.animateStreaming, on ? "1" : "0");
}

export function setReduceTransparency(on: boolean) {
  set({ reduceTransparency: on });
  store(STORAGE.reduceTransparency, on ? "1" : "0");
}

export function setTransparency(percent: number) {
  set({ transparency: percent });
  store(STORAGE.transparency, String(percent));
}

export function setChatWidth(width: ChatWidth) {
  set({ chatWidth: width });
  store(STORAGE.chatWidth, width);
}

export function setShowToolCalls(on: boolean) {
  set({ showToolCalls: on });
  store(STORAGE.showToolCalls, on ? "1" : "0");
}

/**
 * The header's buttons, in the order they show. The ids this build knows are
 * saved beside them, so a button added later shows by default (headerButtons.ts).
 */
export function setHeaderButtons(shown: HeaderButtonId[]) {
  set({ headerButtons: shown });
  store(STORAGE.headerButtons, JSON.stringify({ shown, known: HEADER_BUTTON_IDS }));
}

export function setTrustDelegateAccounts(on: boolean) {
  set({ trustDelegateAccounts: on });
  store(STORAGE.trustDelegateAccounts, on ? "1" : "0");
}

export function setReadAcrossProjects(on: boolean) {
  set({ readAcrossProjects: on });
  store(STORAGE.readAcrossProjects, on ? "1" : "0");
}

/** The wake batch window in seconds, kept to 0..600 (0 turns batching off). */
export function setWakeBatchSeconds(seconds: number) {
  const n = Math.min(WAKE_BATCH_MAX_S, Math.max(0, Math.round(seconds)));
  if (n === get().wakeBatchSeconds) return;
  set({ wakeBatchSeconds: n });
  // A shorter window may have made a wake due already.
  wakeTick();
}

export function setLimitFallback(on: boolean) {
  set({ limitFallback: on });
}

export function setLimitFallbackMap(value: Parameters<typeof parseRules>[0]) {
  set({ limitFallbackMap: parseRules(value) });
}

export function setOrchestration(on: boolean) {
  set((s) => ({
    orchestration: on,
    orchestrationPending: s.engine.phase === "ready" ? !s.orchestrationPending : s.orchestrationPending,
  }));
  store(STORAGE.orchestration, on ? "1" : "0");
}

/** A thread's own delegate limit, kept to 2..32. */
export function setThreadLimit(limit: number) {
  const n = Math.min(MAX_DELEGATES_PER_THREAD, Math.max(MIN_DELEGATES_PER_THREAD, Math.round(limit)));
  set({ threadLimit: n });
  store(STORAGE.threadLimit, String(n));
}

/** The machine-wide delegate total, kept to 2..64. */
export function setTotalLimit(limit: number) {
  const n = Math.min(MAX_DELEGATES_TOTAL, Math.max(MIN_DELEGATES_TOTAL, Math.round(limit)));
  set({ totalLimit: n });
  store(STORAGE.totalLimit, String(n));
}
