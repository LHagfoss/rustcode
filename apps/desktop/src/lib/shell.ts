// The way to the native helpers: Tauri commands in the app, the dev bridge's
// mirror of them in `dev:web`, the mock's canned answers in tests.

import { isTauri, type Transport } from "../acp/transport";
import { withTimeout } from "./withTimeout";

let current: Transport | null = null;

export function setShellTransport(transport: Transport | null) {
  current = transport;
}

/** The transport the app is using, for code that needs to know how it is hosted. */
export function activeTransport(): Transport | null {
  return current;
}

/** Whether a native helper can answer here at all. */
export function hasShell(): boolean {
  return isTauri() || !!current?.call;
}

export async function shellCall<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(command, args);
  }
  if (current?.call) return (await current.call(command, args)) as T;
  throw new Error("Only the app can do this.");
}

/**
 * `shellCall` that gives up after `timeoutMs`, for a read or a button whose
 * native side may wait on a machine that is away. The native work is not
 * cancelled; the page just stops waiting and can say so.
 */
export function shellCallWithin<T>(
  timeoutMs: number,
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return withTimeout(shellCall<T>(command, args), timeoutMs, "The app");
}
