// JSON-RPC 2.0 peer for ACP over newline-delimited JSON. Transport-agnostic:
// feed it lines with `receive`, and it writes lines through `write`.

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type JsonObject = { [key: string]: Json };
export type RpcId = number | string;

export type Inbound =
  | { type: "notification"; method: string; params: JsonObject }
  | { type: "request"; id: RpcId; method: string; params: JsonObject }
  | { type: "closed"; reason: string };

export class RpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: Json,
  ) {
    super(message);
    this.name = "RpcError";
  }

  /** The peer does not implement the method (JSON-RPC -32601). */
  get isMethodNotFound(): boolean {
    return this.code === -32601;
  }
}

export class ConnectionClosedError extends Error {
  constructor(reason: string) {
    super(`The RustCode engine connection closed: ${reason}`);
    this.name = "ConnectionClosedError";
  }
}

export class TimeoutError extends Error {
  constructor(method: string) {
    super(`The RustCode engine did not answer ${method} in time.`);
    this.name = "TimeoutError";
  }
}

interface Pending {
  resolve: (value: Json) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
}

export class AcpConnection {
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private closedReason: string | null = null;
  private listeners = new Set<(message: Inbound) => void>();

  constructor(private readonly write: (line: string) => void) {}

  get isClosed(): boolean {
    return this.closedReason !== null;
  }

  onInbound(listener: (message: Inbound) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Send a request and wait for its result. Prompts pass no timeout: a turn may run for hours. */
  request<T = Json>(method: string, params: JsonObject = {}, timeoutMs?: number): Promise<T> {
    if (this.closedReason !== null) return Promise.reject(new ConnectionClosedError(this.closedReason));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      const entry: Pending = { resolve: resolve as (value: Json) => void, reject };
      if (timeoutMs !== undefined) {
        entry.timer = setTimeout(() => {
          this.pending.delete(id);
          reject(new TimeoutError(method));
        }, timeoutMs);
      }
      this.pending.set(id, entry);
      try {
        this.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
      } catch (error) {
        this.settle(id)?.reject(new Error(`Could not write to the RustCode engine: ${String(error)}`));
      }
    });
  }

  notify(method: string, params: JsonObject = {}): void {
    if (this.closedReason !== null) throw new ConnectionClosedError(this.closedReason);
    this.write(JSON.stringify({ jsonrpc: "2.0", method, params }));
  }

  respond(id: RpcId, result: Json): void {
    if (this.closedReason !== null) return;
    this.write(JSON.stringify({ jsonrpc: "2.0", id, result }));
  }

  respondError(id: RpcId, code: number, message: string): void {
    if (this.closedReason !== null) return;
    this.write(JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }));
  }

  /** Feed one line from the engine. A stray non-JSON line is ignored, not fatal. */
  receive(line: string): void {
    if (this.closedReason !== null) return;
    const trimmed = line.trim();
    if (!trimmed) return;
    let message: JsonObject;
    try {
      const parsed = JSON.parse(trimmed) as Json;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return;
      message = parsed;
    } catch {
      return;
    }
    const { id, method } = message;
    const hasId = typeof id === "number" || typeof id === "string";
    if (typeof method === "string") {
      const params = isObject(message.params) ? message.params : {};
      this.emit(hasId ? { type: "request", id, method, params } : { type: "notification", method, params });
      return;
    }
    if (typeof id !== "number") return;
    const waiter = this.settle(id);
    if (!waiter) return;
    if (isObject(message.error)) {
      const error = message.error;
      waiter.reject(
        new RpcError(
          typeof error.code === "number" ? error.code : -32603,
          typeof error.message === "string" ? error.message : "Engine error",
          error.data,
        ),
      );
    } else {
      waiter.resolve(message.result ?? null);
    }
  }

  /** Fail every outstanding request and tell listeners. Idempotent. */
  close(reason: string): void {
    if (this.closedReason !== null) return;
    this.closedReason = reason;
    for (const [id, waiter] of this.pending) {
      if (waiter.timer) clearTimeout(waiter.timer);
      waiter.reject(new ConnectionClosedError(reason));
      this.pending.delete(id);
    }
    this.emit({ type: "closed", reason });
  }

  private settle(id: number): Pending | undefined {
    const waiter = this.pending.get(id);
    if (!waiter) return undefined;
    this.pending.delete(id);
    if (waiter.timer) clearTimeout(waiter.timer);
    return waiter;
  }

  private emit(message: Inbound): void {
    for (const listener of this.listeners) listener(message);
  }
}

export function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function str(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

export function num(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function bool(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

export function obj(value: unknown): JsonObject | undefined {
  return isObject(value) ? value : undefined;
}

export function arr(value: unknown): Json[] {
  return Array.isArray(value) ? (value as Json[]) : [];
}

/** `value._meta.rustcode`, where RustCode puts its extensions. */
export function rustcodeMeta(value: unknown): JsonObject | undefined {
  return obj(obj(obj(value)?._meta)?.rustcode);
}
