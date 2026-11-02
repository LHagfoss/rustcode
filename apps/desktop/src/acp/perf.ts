// A load generator for `bun run perf` (`?transport=perf`): the mock engine plus
// many threads, a very long thread, a recorded real replay and a fast stream.
// Each engine line arrives as its own task, as Tauri channel messages do, so
// the UI pays the same per-message cost it does in the app.

import { arr, type Json, type JsonObject, obj, str } from "./jsonrpc";
import { MockEngine } from "./mock";
import type { Transport, TransportEvent } from "./transport";
import { BUILTIN_CAPABILITIES } from "./types";

export interface PerfOptions {
  /** Extra threads in the list. */
  threads?: number;
  /** Messages (user and assistant) in `perf-long`. */
  messages?: number;
  /** Streamed chunks per second, and for how long, in `perf-stream`. */
  rate?: number;
  seconds?: number;
}

export const PERF_LONG = "perf-long";
export const PERF_REAL = "perf-real";
export const PERF_STREAM = "perf-stream";

const text = (t: string) => ({ type: "text", text: t });
const update = (sessionId: string, body: JsonObject): JsonObject => ({ sessionId, update: body });

const WORDS =
  "the engine reads each line and folds it into the thread so the transcript shows what the agent did while a turn runs and after it ends".split(
    " ",
  );

function prose(seed: number, words: number): string {
  let out = "";
  for (let i = 0; i < words; i++) out += `${WORDS[(seed * 7 + i * 13) % WORDS.length]} `;
  return out.trim();
}

const CODE = [
  "```ts",
  "export function fold(state: ThreadState, update: SessionUpdate): ThreadState {",
  "  const last = state.items[state.items.length - 1];",
  '  if (update.kind === "agent" && last?.kind === "assistant") {',
  "    return { ...state, items: [...state.items.slice(0, -1), { ...last, text: last.text + update.text }] };",
  "  }",
  "  return state;",
  "}",
  "```",
].join("\n");

/** A reply as an agent writes one: a heading, paragraphs, a list and sometimes code. */
export function reply(seed: number): string {
  const parts = [
    `### Step ${seed}`,
    prose(seed, 40),
    `- ${prose(seed + 1, 8)}\n- ${prose(seed + 2, 10)}\n- \`inline ${seed}\` ${prose(seed + 3, 6)}`,
    prose(seed + 4, 30),
  ];
  if (seed % 5 === 0) parts.push(CODE);
  return parts.join("\n\n");
}

/** The `session/load` replay of a thread with `messages` user and assistant messages and tool calls between. */
export function longHistory(id: string, messages: number): JsonObject[] {
  const out: JsonObject[] = [];
  for (let turn = 0; turn < messages / 2; turn++) {
    out.push(
      update(id, {
        sessionUpdate: "user_message_chunk",
        content: text(`Request ${turn}: ${prose(turn, 18)}`),
      }),
    );
    out.push(
      update(id, {
        sessionUpdate: "agent_thought_chunk",
        content: text(`**Planning step ${turn}**\n${prose(turn, 20)}`),
        _meta: { rustcode: { thoughtSource: "summary", thoughtMs: 1200 } },
      }),
    );
    for (let t = 0; t < 2; t++) {
      const callId = `call-${turn}-${t}`;
      out.push(
        update(id, {
          sessionUpdate: "tool_call",
          toolCallId: callId,
          title: t ? "bun test" : "rg",
          kind: t ? "execute" : "search",
          rawInput: t ? { command: "bun test src" } : { pattern: `step${turn}`, path: "src" },
        }),
      );
      out.push(
        update(id, {
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: "completed",
          rawOutput: { content: `src/state/thread.ts:${turn}: ${prose(turn + t, 12)}`, exitCode: 0 },
        }),
      );
    }
    out.push(update(id, { sessionUpdate: "agent_message_chunk", content: text(reply(turn)) }));
  }
  return out;
}

function list(count: number): Json[] {
  const now = Date.now();
  const special = [PERF_STREAM, PERF_LONG, PERF_REAL].map((id, i) => ({
    sessionId: id,
    cwd: "/Users/you/code/rustcode",
    title: id,
    updatedAt: new Date(now - i * 1000).toISOString(),
    _meta: { model: "chatgpt/gpt-6-luna", rustcode: { harness: "rustcode" } },
  }));
  const rest = Array.from({ length: count }, (_, i) => ({
    sessionId: `perf-${i}`,
    cwd: `/Users/you/code/project-${i % 12}`,
    title: `Thread ${i}: ${prose(i, 6)}`,
    updatedAt: new Date(now - (i + 10) * 600_000).toISOString(),
    _meta: { model: "chatgpt/gpt-6-luna", rustcode: { harness: "rustcode" } },
  }));
  return [...special, ...rest];
}

/** Lines recorded from a real `session/load` (`window.__perfReplay`), moved onto `perf-real`. */
function realReplay(): JsonObject[] {
  const lines = (globalThis as { __perfReplay?: string[] }).__perfReplay ?? [];
  return lines.flatMap((line) => {
    try {
      const params = obj((JSON.parse(line) as JsonObject).params);
      return params ? [{ ...params, sessionId: PERF_REAL }] : [];
    } catch {
      return [];
    }
  });
}

export class PerfTransport implements Transport {
  readonly name = "mock";
  private engine: MockEngine | null = null;
  private onEvent: ((event: TransportEvent) => void) | null = null;
  private outbox: string[] = [];
  private posting = false;
  private readonly channel = new MessageChannel();
  private streaming = new Map<string, { cancelled: boolean }>();

  constructor(private readonly options: PerfOptions = {}) {
    // One line per task, in order, like a Tauri channel without batching.
    this.channel.port1.onmessage = () => {
      const line = this.outbox.shift();
      if (line !== undefined) this.onEvent?.({ type: "line", line });
      if (this.outbox.length) this.channel.port2.postMessage(0);
      else this.posting = false;
    };
  }

  private emit = (line: string) => {
    this.outbox.push(line);
    if (!this.posting) {
      this.posting = true;
      this.channel.port2.postMessage(0);
    }
  };

  private notify(params: JsonObject) {
    this.emit(JSON.stringify({ jsonrpc: "2.0", method: "session/update", params }));
  }

  private respond(id: Json | undefined, result: Json) {
    this.emit(JSON.stringify({ jsonrpc: "2.0", id: id ?? null, result }));
  }

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    this.engine = new MockEngine(this.emit, { scenario: "default", delayMs: 40 });
    onEvent({ type: "open", engine: "perf" });
  }

  async restart(): Promise<void> {
    if (!this.onEvent) return;
    this.onEvent({ type: "closed", reason: "Restarting the engine." });
    await this.start(this.onEvent);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }

  async call(command: string): Promise<unknown> {
    if (command === "link_meta") return null;
    throw new Error(`The perf mock does not run ${command}.`);
  }

  send(line: string): void {
    let message: JsonObject;
    try {
      message = JSON.parse(line) as JsonObject;
    } catch {
      return;
    }
    const method = str(message.method);
    const params = obj(message.params) ?? {};
    const sid = str(params.sessionId) ?? "";
    if (method === "session/list") {
      this.respond(message.id, { sessions: list(this.options.threads ?? 500) });
      return;
    }
    if (!sid.startsWith("perf-") || !method) {
      this.engine?.receive(line);
      return;
    }
    switch (method) {
      case "session/load": {
        const history =
          sid === PERF_LONG
            ? longHistory(sid, this.options.messages ?? 2000)
            : sid === PERF_REAL
              ? realReplay()
              : [];
        for (const params of history) this.notify(params);
        this.respond(message.id, {
          configOptions: [],
          _meta: {
            rustcode: { harness: { id: "rustcode", capabilities: BUILTIN_CAPABILITIES as unknown as Json } },
          },
        });
        return;
      }
      case "session/prompt":
        void this.stream(message.id, sid, arr(params.prompt).length);
        return;
      case "session/cancel": {
        const run = this.streaming.get(sid);
        if (run) run.cancelled = true;
        return;
      }
      default:
        this.respond(message.id, {});
    }
  }

  /** Reply chunks at `rate` per second for `seconds`, with a running tool call every 40 chunks. */
  private async stream(id: Json | undefined, sid: string, _blocks: number) {
    const rate = this.options.rate ?? 100;
    const total = rate * (this.options.seconds ?? 30);
    const run = { cancelled: false };
    this.streaming.set(sid, run);
    const body = Array.from({ length: 40 }, (_, i) => reply(i)).join("\n\n");
    const pieces = body.match(/[\s\S]{1,24}/g) ?? [];
    const started = performance.now();
    let tool = 0;
    for (let i = 0; i < total && !run.cancelled; i++) {
      const due = started + (i * 1000) / rate;
      const wait = due - performance.now();
      if (wait > 1) await new Promise((r) => setTimeout(r, wait));
      if (i % 40 === 20) {
        const callId = `stream-${++tool}`;
        this.notify(
          update(sid, {
            sessionUpdate: "tool_call",
            toolCallId: callId,
            title: "bun test",
            kind: "execute",
            rawInput: { command: "bun test src" },
          }),
        );
        for (let k = 0; k < 5; k++) {
          this.notify(
            update(sid, {
              sessionUpdate: "tool_call_update",
              toolCallId: callId,
              status: "in_progress",
              _meta: { rustcode: { outputDelta: `ok ${k} ${prose(k, 8)}\n` } },
            }),
          );
        }
        this.notify(
          update(sid, {
            sessionUpdate: "tool_call_update",
            toolCallId: callId,
            status: "completed",
            rawOutput: { exitCode: 0 },
          }),
        );
        continue;
      }
      this.notify(
        update(sid, {
          sessionUpdate: "agent_message_chunk",
          content: text(pieces[i % pieces.length] ?? " "),
        }),
      );
    }
    this.streaming.delete(sid);
    this.respond(id, { stopReason: run.cancelled ? "cancelled" : "end_turn" });
  }
}
