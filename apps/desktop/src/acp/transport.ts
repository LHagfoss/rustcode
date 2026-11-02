// How the UI reaches the engine. Three implementations share one shape:
// - Tauri: the Rust shell runs `rustcode --acp` as a sidecar and relays lines.
// - WebSocket: `scripts/engine-bridge.ts` relays lines for `bun run dev:web`.
// - Mock: an in-process fake engine (`mock.ts`) for tests and screenshots.

export type TransportEvent =
  | { type: "line"; line: string }
  | { type: "open"; engine?: string }
  | { type: "closed"; reason: string }
  | { type: "log"; line: string };

export interface Transport {
  readonly name: "tauri" | "websocket" | "mock";
  /** Start (or restart) the engine and deliver its events. */
  start(onEvent: (event: TransportEvent) => void): Promise<void>;
  send(line: string): void;
  /** Stop the engine and start a fresh one; an `open` event follows. */
  restart(): Promise<void>;
  stop(): Promise<void>;
  /**
   * A native helper outside Tauri: the dev bridge runs the same commands as
   * the shell (`src-tauri/src/*.rs`) so `dev:web` can show them, and the mock
   * answers a few with canned data. Absent where nothing can answer.
   */
  call?(command: string, args: Record<string, unknown>): Promise<unknown>;
}

/** Tauri sidecar transport: lines and lifecycle events arrive on a Channel. */
export class TauriTransport implements Transport {
  readonly name = "tauri";

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const channel = new Channel<TransportEvent>();
    channel.onmessage = onEvent;
    await invoke("engine_start", { onEvent: channel });
  }

  send(line: string): void {
    void import("@tauri-apps/api/core").then(({ invoke }) => invoke("engine_send", { line }));
  }

  async restart(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("engine_restart");
  }

  async stop(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("engine_stop");
  }
}

/** WebSocket transport for the browser dev mode, talking to `engine-bridge.ts`. */
export class WebSocketTransport implements Transport {
  readonly name = "websocket";
  private socket: WebSocket | null = null;
  private queue: string[] = [];
  private calls = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private nextCall = 1;

  constructor(private readonly url: string) {}

  start(onEvent: (event: TransportEvent) => void): Promise<void> {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(this.url);
      this.socket = socket;
      let opened = false;
      socket.onopen = () => {
        opened = true;
        for (const line of this.queue.splice(0)) socket.send(line);
        resolve();
      };
      socket.onmessage = (message) => {
        let event:
          | TransportEvent
          | PtyEvent
          | { type: "reply"; id: number; ok: boolean; value?: unknown; error?: string };
        try {
          event = JSON.parse(String(message.data));
        } catch {
          return; // The bridge only sends JSON events.
        }
        if (event.type === "reply") {
          const call = this.calls.get(event.id);
          this.calls.delete(event.id);
          if (event.ok) call?.resolve(event.value);
          else call?.reject(new Error(event.error ?? "The bridge could not run it."));
          return;
        }
        if (event.type === "pty") {
          for (const listener of ptyListeners) listener(event);
          return;
        }
        onEvent(event);
      };
      socket.onclose = () => {
        onEvent({ type: "closed", reason: "The engine bridge disconnected." });
        if (!opened) reject(new Error(`No engine bridge at ${this.url}. Run \`bun run dev:web\`.`));
      };
    });
  }

  send(line: string): void {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(line);
    else this.queue.push(line);
  }

  async restart(): Promise<void> {
    this.send(JSON.stringify({ bridge: "restart" }));
  }

  call(command: string, args: Record<string, unknown>): Promise<unknown> {
    const id = this.nextCall++;
    return new Promise((resolve, reject) => {
      this.calls.set(id, { resolve, reject });
      this.send(JSON.stringify({ bridge: "call", id, command, args }));
      setTimeout(() => {
        if (this.calls.delete(id)) reject(new Error(`${command} timed out`));
      }, 15_000);
    });
  }

  async stop(): Promise<void> {
    this.socket?.close();
    this.socket = null;
  }
}

/** Terminal output and exits the dev bridge streams for `pty_*` sessions. */
export type PtyEvent = { type: "pty"; id: number; data?: string; exit?: number | null };
const ptyListeners = new Set<(event: PtyEvent) => void>();
export function onBridgePty(listener: (event: PtyEvent) => void): () => void {
  ptyListeners.add(listener);
  return () => ptyListeners.delete(listener);
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Picks the transport: Tauri inside the app, else `?transport=` / `VITE_TRANSPORT`, else the bridge. */
export async function defaultTransport(): Promise<Transport> {
  if (isTauri()) return new TauriTransport();
  const query = new URLSearchParams(window.location.search).get("transport");
  const choice = query ?? import.meta.env.VITE_TRANSPORT;
  if (choice === "mock") {
    const { MockTransport } = await import("./mock");
    const scenario = new URLSearchParams(window.location.search).get("scenario") ?? undefined;
    return new MockTransport({ scenario });
  }
  const port = import.meta.env.VITE_BRIDGE_PORT ?? "1421";
  return new WebSocketTransport(`ws://127.0.0.1:${port}`);
}
