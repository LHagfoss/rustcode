// How the UI reaches the engine. Three implementations share one shape:
// - Tauri: the Rust shell runs `rustcode --acp` as a sidecar and relays lines.
// - WebSocket: `scripts/engine-bridge.ts` relays lines for `bun run dev:web`.
// - Hub: the web build a hub serves (`bun run build:hub`), signed in with the
//   owner's passkey; ACP over the hub's `/app/acp` socket.
// - Mock: an in-process fake engine (`mock.ts`) for tests and screenshots.

export type TransportEvent =
  | { type: "line"; line: string }
  /** Several stdout lines read together (the shell batches what is already buffered). */
  | { type: "lines"; lines: string[] }
  | { type: "open"; engine?: string }
  | { type: "closed"; reason: string }
  | { type: "log"; line: string };

export interface Transport {
  readonly name: "tauri" | "websocket" | "mock" | "machine" | "hub";
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
    this.invoke = invoke;
    const channel = new Channel<TransportEvent>();
    channel.onmessage = onEvent;
    await invoke("engine_start", { onEvent: channel });
  }

  private invoke: typeof import("@tauri-apps/api/core").invoke | null = null;

  send(line: string): void {
    // In order: once the module is loaded, every send goes straight to invoke.
    if (this.invoke) void this.invoke("engine_send", { line });
    else void import("@tauri-apps/api/core").then(({ invoke }) => invoke("engine_send", { line }));
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

/**
 * The answer to a request the shell would not send, so whoever waits for it
 * hears now instead of at its timeout. Null for a line nobody waits on: a
 * notification, or an answer to the machine.
 */
export function droppedLineAnswer(line: string, reason: string): string | null {
  let message: unknown;
  try {
    message = JSON.parse(line);
  } catch {
    return null;
  }
  if (!message || typeof message !== "object" || Array.isArray(message)) return null;
  const { id, method } = message as { id?: unknown; method?: unknown };
  if (typeof method !== "string" || (typeof id !== "number" && typeof id !== "string")) return null;
  return JSON.stringify({ jsonrpc: "2.0", id, error: { code: -32000, message: reason } });
}

/**
 * Another computer this app is paired with (Settings › Machines). The shell
 * holds the supervisor, the connection and the pairing token; this relays the
 * machine's ACP frames to the page the way [`TauriTransport`] relays the local
 * engine's, so one client class serves both.
 *
 * Without the app's shell (`dev:web`, or `?transport=mock`) the answer comes
 * from whichever transport is running: the mock speaks for the machine in
 * process, and the dev bridge has no machines at all.
 */
export class MachineTransport implements Transport {
  readonly name = "machine";
  private onEvent: ((event: TransportEvent) => void) | null = null;
  /** `?transport=mock`: the canned machine answers its own lines. */
  private mock: { receive(line: string): string[] } | null = null;

  constructor(
    /** The machine's `host_id`, which every shell command takes. */
    readonly machineId: string,
    /** What the machine is called, for a log line and the page. */
    readonly label: string,
  ) {}

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    if (isTauri()) {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<TransportEvent>();
      channel.onmessage = onEvent;
      await invoke("machine_connect", { id: this.machineId, onEvent: channel });
      return;
    }
    // Loaded here, not at the top: lib/shell.ts reads isTauri from this module.
    const { activeTransport, shellCall } = await import("../lib/shell");
    const answer = await shellCall<{ connected?: boolean } | null>("machine_connect", {
      id: this.machineId,
    });
    if (answer?.connected === false) return;
    if (activeTransport()?.name === "mock") {
      const { MockMachine } = await import("./mock");
      this.mock = new MockMachine(this.label);
    }
    onEvent({ type: "open", engine: this.label });
  }

  send(line: string): void {
    if (isTauri()) {
      void import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke("machine_send", { id: this.machineId, line }))
        .catch((error: unknown) => this.dropped(line, error));
      return;
    }
    // Deferred: an answer must arrive after the client has registered the request it answers.
    for (const reply of this.mock?.receive(line) ?? []) {
      queueMicrotask(() => this.onEvent?.({ type: "line", line: reply }));
    }
  }

  /** The shell did not take a line (its queue is full, or the machine is not connected). */
  private dropped(line: string, error: unknown): void {
    const reason = typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
    this.onEvent?.({ type: "log", line: `desktop: a line for ${this.label} was not sent: ${reason}` });
    const refusal = droppedLineAnswer(line, reason);
    if (refusal) this.onEvent?.({ type: "line", line: refusal });
  }

  async restart(): Promise<void> {
    const onEvent = this.onEvent;
    await this.stop();
    if (onEvent) await this.start(onEvent);
  }

  async stop(): Promise<void> {
    this.mock = null;
    if (!isTauri()) return;
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("machine_detach", { id: this.machineId });
  }
}

/** WebSocket transport for the browser dev mode, talking to `engine-bridge.ts`. */
export class WebSocketTransport implements Transport {
  readonly name = "websocket";
  private socket: WebSocket | null = null;
  private queue: string[] = [];
  private calls = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }
  >();
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
          | { type: "reply"; id: number; ok: boolean; value?: unknown; error?: string };
        try {
          event = JSON.parse(String(message.data));
        } catch {
          return; // The bridge only sends JSON events.
        }
        if (event.type === "reply") {
          const call = this.calls.get(event.id);
          this.calls.delete(event.id);
          if (call) clearTimeout(call.timer);
          if (event.ok) call?.resolve(event.value);
          else call?.reject(new Error(event.error ?? "The bridge could not run it."));
          return;
        }
        onEvent(event);
      };
      socket.onclose = () => {
        onEvent({ type: "closed", reason: "The server bridge disconnected." });
        if (!opened) reject(new Error(`No server bridge at ${this.url}. Run \`bun run dev:web\`.`));
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
      // The timeout goes with the answer: a reply in time leaves no 15 s timer behind it.
      const timer = setTimeout(() => {
        if (this.calls.delete(id)) reject(new Error(`${command} timed out`));
      }, 15_000);
      this.calls.set(id, { resolve, reject, timer });
      this.send(JSON.stringify({ bridge: "call", id, command, args }));
    });
  }

  async stop(): Promise<void> {
    this.socket?.close();
    this.socket = null;
  }
}

/** The browser session the hub signed this page in with, once known. */
let hubSessionId: string | null = null;
export function hubSession(): string | null {
  return hubSessionId;
}

/**
 * The app served from a hub (`orchestration/outside_web.rs`). The page's
 * cookie buys a single-use ticket at `/app/ticket`; the ticket opens
 * `/app/acp`, which relays ACP lines to the hub's server as they are. Not
 * signed in (or signed out in Settings) sends the page to `/login`. There is
 * no native helper here, so `call` is absent and the shell's features say so.
 */
export class HubTransport implements Transport {
  readonly name = "hub";
  private socket: WebSocket | null = null;
  private queue: string[] = [];
  private ready = false;
  private onEvent: ((event: TransportEvent) => void) | null = null;

  constructor(private readonly origin: string = globalThis.location?.origin ?? "") {}

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    const answer = await fetch(`${this.origin}/app/ticket`, { method: "POST", credentials: "same-origin" });
    if (answer.status === 401) {
      globalThis.location?.assign("/login");
      throw new Error("Sign in with your passkey to use RustCode here.");
    }
    if (!answer.ok) throw new Error(`The hub did not open a connection (HTTP ${answer.status}).`);
    const { ticket, session } = (await answer.json()) as { ticket: string; session?: string };
    hubSessionId = session ?? null;
    const url = new URL(`/app/acp?ticket=${encodeURIComponent(ticket)}`, this.origin);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(url);
      this.socket = socket;
      this.ready = false;
      socket.onmessage = (message) => {
        const line = String(message.data);
        if (!this.ready) {
          // The relay's first frame says the hub's server is there.
          this.ready = true;
          onEvent({ type: "open", engine: url.host });
          for (const queued of this.queue.splice(0)) socket.send(queued);
          resolve();
          if (line.includes('"acp_opened"')) return;
        }
        onEvent({ type: "line", line });
      };
      socket.onclose = (event) => {
        if (this.socket !== socket) return;
        this.socket = null;
        const why = event.reason || "The connection to the hub closed.";
        onEvent({ type: "closed", reason: why === "signed out" ? "This browser was signed out." : why });
        if (!this.ready) reject(new Error(why));
        if (event.reason === "signed out") globalThis.location?.assign("/login");
      };
    });
  }

  send(line: string): void {
    if (this.ready && this.socket?.readyState === WebSocket.OPEN) this.socket.send(line);
    else this.queue.push(line);
  }

  async restart(): Promise<void> {
    await this.stop();
    if (this.onEvent) await this.start(this.onEvent);
  }

  async stop(): Promise<void> {
    const socket = this.socket;
    this.socket = null;
    this.ready = false;
    socket?.close();
  }
}

/** Sign this browser out of the hub and go back to its login page. */
export function signOutOfHub() {
  const form = document.createElement("form");
  form.method = "post";
  form.action = "/logout";
  document.body.appendChild(form);
  form.submit();
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Picks the transport: Tauri inside the app, else `?transport=` / `VITE_TRANSPORT`, else the bridge. */
export async function defaultTransport(): Promise<Transport> {
  if (isTauri()) return new TauriTransport();
  const query = new URLSearchParams(window.location.search).get("transport");
  const choice = query ?? import.meta.env.VITE_TRANSPORT;
  if (choice === "perf") {
    // `bun run perf`: a load generator; numbers in the query size it.
    const { PerfTransport } = await import("./perf");
    const q = new URLSearchParams(window.location.search);
    const n = (key: string) => (q.has(key) ? Number(q.get(key)) : undefined);
    return new PerfTransport({
      threads: n("threads"),
      messages: n("messages"),
      rate: n("rate"),
      seconds: n("seconds"),
    });
  }
  if (choice === "hub") return new HubTransport();
  if (choice === "mock") {
    const { MockTransport } = await import("./mock");
    const q = new URLSearchParams(window.location.search);
    return new MockTransport({
      scenario: q.get("scenario") ?? undefined,
      // The inbox's demo rows (a question, approvals, a failed turn); `?needsYou=0` starts clean.
      needsYou: q.get("needsYou") !== "0",
      bootMs: Number(q.get("boot")) || undefined,
      crashAfterMs: Number(q.get("crash")) || undefined,
      claudeUsage: (["expired", "refused", "forbidden", "denied", "recorded"] as const).find(
        (f) => f === q.get("claudeUsage"),
      ),
      // Remote access states the happy path does not reach (`?remote=disconnected|expired|connect|refused`).
      remote: (["disconnected", "expired", "connect", "refused"] as const).find((f) => f === q.get("remote")),
      // This app paired with other computers (`?machines=paired|connect|offline|revoked|forward`).
      machines: (["paired", "connect", "offline", "revoked", "forward"] as const).find(
        (f) => f === q.get("machines"),
      ),
      // Six identical quiet notices after the first thread's turn (`?notices=repeat`).
      notices: q.get("notices") === "repeat" ? "repeat" : undefined,
      // Tool row cases on a thread of their own (`?tools=background`).
      tools: (["background"] as const).find((f) => f === q.get("tools")),
    });
  }
  const port = import.meta.env.VITE_BRIDGE_PORT ?? "1421";
  return new WebSocketTransport(`ws://127.0.0.1:${port}`);
}
