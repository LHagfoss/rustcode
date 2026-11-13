// A terminal session: the shell's PTY in Tauri (`src-tauri/src/pty.rs`), or
// the dev bridge's mirror in `dev:web`.

import { isTauri, onBridgePty } from "../acp/transport";
import { hasShell, shellCall } from "./shell";

export interface PtyHandle {
  write(data: string): void;
  resize(cols: number, rows: number): void;
  close(): void;
}

export function canOpenPty(): boolean {
  return hasShell();
}

export async function openPty(
  cwd: string,
  cols: number,
  rows: number,
  onData: (data: string) => void,
  onExit: () => void,
): Promise<PtyHandle> {
  let id: number;
  let stop: () => void = () => {};
  if (isTauri()) {
    const { Channel } = await import("@tauri-apps/api/core");
    const channel = new Channel<{ type: "data"; data: string } | { type: "exit" }>();
    channel.onmessage = (event) => (event.type === "data" ? onData(event.data) : onExit());
    id = await shellCall<number>("pty_open", { cwd, cols, rows, onEvent: channel });
  } else {
    const early: { data?: string; exit?: number | null }[] = [];
    let known: number | undefined;
    const unlisten = onBridgePty((event) => {
      if (known === undefined) early.push(event);
      else if (event.id === known) {
        if (event.data !== undefined) onData(event.data);
        if (event.exit !== undefined) onExit();
      }
    });
    id = await shellCall<number>("pty_open", { cwd, cols, rows });
    known = id;
    stop = () => {
      unlisten();
    };
    for (const e of early) {
      if ((e as { id?: number }).id !== id) continue;
      if (e.data !== undefined) onData(e.data);
      if (e.exit !== undefined) onExit();
    }
  }
  return {
    write: (data) =>
      void shellCall("pty_write", { id, data }).catch((error) => onData(`\r\n[${String(error)}]\r\n`)),
    resize: (c, r) => void shellCall("pty_resize", { id, cols: c, rows: r }).catch(() => undefined),
    close: () => {
      stop();
      void shellCall("pty_close", { id }).catch(() => undefined);
    },
  };
}
