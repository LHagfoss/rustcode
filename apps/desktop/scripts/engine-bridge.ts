// Dev-only bridge: serves `rustcode --acp` to the browser UI over a local
// WebSocket, one engine per connection, so `bun run dev:web` works without
// the Tauri shell. Lines go through unchanged; engine stderr arrives as `log`
// events. Only pages from the Vite dev server may connect.
//
// Engine: $RUSTCODE_ENGINE, else ../../target/debug/rustcode, else `rustcode` on PATH.

import { existsSync } from "node:fs";
import { resolve } from "node:path";
import type { ServerWebSocket, Subprocess } from "bun";
import { closePtys, nativeCommands } from "./bridge-native";

const PORT = Number(process.env.BRIDGE_PORT ?? 1421);
const ALLOWED_ORIGINS = new Set(
  (process.env.BRIDGE_ORIGINS ?? "http://localhost:1420,http://127.0.0.1:1420").split(","),
);

export function resolveEngine(): string {
  const fromEnv = process.env.RUSTCODE_ENGINE;
  if (fromEnv) return fromEnv;
  const dev = resolve(import.meta.dir, "../../../target/debug/rustcode");
  if (existsSync(dev)) return dev;
  return Bun.which("rustcode") ?? "rustcode";
}

interface Session {
  engine: Subprocess<"pipe", "pipe", "pipe"> | null;
  closed: boolean;
  backoffMs: number;
  startedAt: number;
}

type Socket = ServerWebSocket<Session>;

export function send(ws: Socket, event: unknown) {
  if (!ws.data.closed) ws.send(JSON.stringify(event));
}

async function pump(stream: ReadableStream<Uint8Array>, onLine: (line: string) => void) {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of stream) {
    buffer += decoder.decode(chunk, { stream: true });
    let newline = buffer.indexOf("\n");
    while (newline >= 0) {
      onLine(buffer.slice(0, newline));
      buffer = buffer.slice(newline + 1);
      newline = buffer.indexOf("\n");
    }
  }
  if (buffer) onLine(buffer);
}

function start(ws: Socket) {
  const path = resolveEngine();
  let engine: Subprocess<"pipe", "pipe", "pipe">;
  try {
    engine = Bun.spawn([path, "--acp"], { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  } catch (error) {
    send(ws, { type: "closed", reason: `Could not start ${path}: ${String(error)}` });
    return;
  }
  ws.data.engine = engine;
  ws.data.startedAt = Date.now();
  send(ws, { type: "log", line: `bridge: started ${path} (pid ${engine.pid})` });
  send(ws, { type: "open", engine: path });
  void pump(engine.stdout, (line) => send(ws, { type: "line", line }));
  void pump(engine.stderr, (line) => send(ws, { type: "log", line }));
  void engine.exited.then((code) => {
    if (ws.data.engine !== engine) return;
    ws.data.engine = null;
    if (ws.data.closed) return;
    send(ws, { type: "closed", reason: `The engine exited (code ${code}).` });
    // Restart with backoff; a run that stayed up a minute resets it.
    if (Date.now() - ws.data.startedAt > 60_000) ws.data.backoffMs = 500;
    const wait = ws.data.backoffMs;
    ws.data.backoffMs = Math.min(30_000, wait * 2);
    send(ws, { type: "log", line: `bridge: restarting in ${wait} ms` });
    setTimeout(() => !ws.data.closed && !ws.data.engine && start(ws), wait);
  });
}

function stop(ws: Socket) {
  const engine = ws.data.engine;
  ws.data.engine = null;
  engine?.kill();
}

if (import.meta.main) {
  Bun.serve<Session>({
    hostname: "127.0.0.1",
    port: PORT,
    fetch(req, server) {
      const origin = req.headers.get("origin") ?? "";
      if (!ALLOWED_ORIGINS.has(origin)) return new Response("Forbidden origin", { status: 403 });
      if (server.upgrade(req, { data: { engine: null, closed: false, backoffMs: 500, startedAt: 0 } }))
        return;
      return new Response("RustCode engine bridge", { status: 426 });
    },
    websocket: {
      open: start,
      message(ws, message) {
        const line = String(message);
        if (line.startsWith('{"bridge":"call"')) {
          const { id, command, args } = JSON.parse(line) as {
            id: number;
            command: string;
            args: Record<string, unknown>;
          };
          const run = nativeCommands[command];
          if (!run) {
            send(ws, { type: "reply", id, ok: false, error: `Unknown command ${command}` });
            return;
          }
          run({ ...args, __socket: ws }).then(
            (value) => send(ws, { type: "reply", id, ok: true, value: value ?? null }),
            (error) => send(ws, { type: "reply", id, ok: false, error: String(error?.message ?? error) }),
          );
          return;
        }
        if (line.startsWith('{"bridge"')) {
          stop(ws);
          ws.data.backoffMs = 500;
          start(ws);
          return;
        }
        ws.data.engine?.stdin.write(`${line}\n`);
        ws.data.engine?.stdin.flush();
      },
      close(ws) {
        closePtys();
        ws.data.closed = true;
        stop(ws);
      },
    },
  });
  console.log(`engine bridge on ws://127.0.0.1:${PORT} → ${resolveEngine()} --acp`);
}
