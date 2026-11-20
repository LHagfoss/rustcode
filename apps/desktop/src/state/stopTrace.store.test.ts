// A Stop press traced from intent to outcome (issue 4086), driven through the store against the
// mock engine. The transport stands between them and decides what happens to the Stop request:
// answered, dropped by the shell, never answered, refused as unknown (an older engine), or
// answered by a harness that then ignores it. Each case checks the stage the trace ends on.

import { beforeEach, describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import type { Transport, TransportEvent } from "../acp/transport";
import { Method } from "../acp/types";
import { lastStage, setStopAckMsForTests, stopPending, stopTrace } from "./stopTrace";
import {
  cancel,
  connect,
  resetForTests,
  setProjectCwd,
  stopNow,
  submit,
  switchNewThreadHarness,
  useApp,
} from "./store";
import { isRunning } from "./thread";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

/** What the engine does with `_rustcode/session/stop`. */
type StopMode = "answer" | "drop" | "silent" | "legacy" | "ignore";

class StopTransport implements Transport {
  readonly name = "mock" as const;
  private readonly mock = new MockTransport({ delayMs: 0 });
  private onEvent: ((event: TransportEvent) => void) | null = null;
  readonly sent: JsonObject[] = [];
  mode: StopMode = "answer";
  private readonly held: string[] = [];

  /** The permission answers held back reach the mock now. */
  release(): void {
    for (const line of this.held.splice(0)) this.mock.send(line);
  }

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    await this.mock.start(onEvent);
  }

  private answer(id: unknown, body: JsonObject): void {
    queueMicrotask(() =>
      this.onEvent?.({ type: "line", line: JSON.stringify({ jsonrpc: "2.0", id, ...body }) }),
    );
  }

  send(line: string): void {
    const message = JSON.parse(line) as JsonObject;
    this.sent.push(message);
    if (message.method === Method.sessionStop && this.mode !== "answer") {
      if (this.mode === "drop")
        this.answer(message.id, { error: { code: -32000, message: "the machine is not connected" } });
      else if (this.mode === "legacy")
        this.answer(message.id, { error: { code: -32601, message: "Method not found" } });
      // The engine took it and told the harness, which goes on as if it had not.
      else if (this.mode === "ignore")
        this.answer(message.id, { result: { route: "harness", active: true } });
      return;
    }
    // A turn that goes on: neither the cancel nor the cancelled permission reaches the mock, until
    // `release` lets the permission answers through.
    if (this.mode !== "answer" && this.mode !== "legacy") {
      if (message.method === Method.sessionCancel) return;
      if (!message.method && "result" in message) {
        this.held.push(line);
        return;
      }
    }
    this.mock.send(line);
  }

  async restart(): Promise<void> {
    await this.mock.restart();
  }

  async stop(): Promise<void> {
    await this.mock.stop();
  }
}

/** A running turn that waits on a permission, so it keeps running until stopped. */
async function runningTurn(mode: StopMode) {
  const transport = new StopTransport();
  transport.mode = mode;
  await connect(transport);
  await until(() => useApp.getState().engine.phase === "ready");
  setProjectCwd("/tmp/project");
  switchNewThreadHarness("codex");
  const done = submit("change the greeting");
  await until(
    () => (useApp.getState().threads[useApp.getState().selectedId ?? ""]?.permissions.length ?? 0) > 0,
  );
  const id = useApp.getState().selectedId!;
  return { transport, id, done };
}

const stages = (id: string) =>
  stopTrace(id)
    .at(-1)
    ?.events.map((e) => e.stage) ?? [];
const stopsSent = (transport: StopTransport) =>
  transport.sent.filter((m) => m.method === Method.sessionStop || m.method === Method.sessionCancel);

beforeEach(() => resetForTests());

describe("a traced Stop", () => {
  test("an answered Stop ends with the turn, and repeated presses send nothing more", async () => {
    const { transport, id, done } = await runningTurn("answer");
    await Promise.all([cancel(id), cancel(id), cancel(id)]);
    await done;
    await until(() => !stopPending(id), 3000);
    expect(stopsSent(transport)).toHaveLength(1);
    const params = stopsSent(transport)[0]!.params as JsonObject;
    expect(params.attempt).toBe(stopTrace(id)[0]!.id);
    expect(stopTrace(id)).toHaveLength(1);
    expect(stages(id)).toEqual(["intent", "dispatched", "coalesced", "coalesced", "acknowledged", "ended"]);
    expect(lastStage(stopTrace(id)[0]!)?.reason).toBe("cancelled");
    expect(stopPending(id)).toBe(false);
  });

  test("Stopping… shows from the press until the turn ends", async () => {
    const { id, done } = await runningTurn("answer");
    const pressed = cancel(id);
    expect(stopPending(id)).toBe(true);
    await pressed;
    await done;
    await until(() => !stopPending(id), 3000);
    expect(isRunning(useApp.getState().threads[id])).toBe(false);
  });

  test("a dropped Stop fails, says so, and the next press tries again", async () => {
    const { transport, id, done } = await runningTurn("drop");
    await cancel(id);
    await until(() => !stopPending(id));
    expect(stages(id)).toEqual(["intent", "dispatched", "failed"]);
    expect(lastStage(stopTrace(id)[0]!)?.reason).toBe("dropped");
    expect(stopPending(id)).toBe(false);
    expect(useApp.getState().toasts.some((t) => t.text.includes("Press Stop to try again"))).toBe(true);
    expect(isRunning(useApp.getState().threads[id])).toBe(true);
    transport.mode = "answer";
    await cancel(id);
    transport.release();
    await done;
    await until(() => !stopPending(id), 3000);
    expect(stopTrace(id)).toHaveLength(2);
    // The turn may end before the engine's answer arrives: both are recorded, in the order seen.
    expect([...stages(id)].sort()).toEqual(["acknowledged", "dispatched", "ended", "intent"]);
  });

  test("an unanswered Stop says it timed out and stays pending", async () => {
    setStopAckMsForTests(20);
    const { id } = await runningTurn("silent");
    await cancel(id);
    await until(() => stages(id).includes("timed_out"));
    expect(stages(id)).toEqual(["intent", "dispatched", "timed_out"]);
    expect(stopPending(id)).toBe(true);
  });

  test("an engine without the request gets session/cancel with the attempt", async () => {
    const { transport, id, done } = await runningTurn("legacy");
    await cancel(id);
    await done;
    await until(() => !stopPending(id), 3000);
    const notice = transport.sent.find((m) => m.method === Method.sessionCancel)!;
    const meta = (notice.params as JsonObject)._meta as JsonObject;
    expect((meta.rustcode as JsonObject).stopAttempt).toBe(stopTrace(id)[0]!.id);
    expect(stages(id)).toEqual(["intent", "dispatched", "acknowledged", "ended"]);
    expect(stopTrace(id)[0]!.events[2]!.reason).toBe("legacy");
  });

  test("a harness that ignores the Stop is forced, and the trace says so", async () => {
    const { id } = await runningTurn("ignore");
    expect(await stopNow(id, 50)).toBe(true);
    expect(stages(id)).toEqual(["intent", "dispatched", "acknowledged", "forced"]);
    expect(stopTrace(id)[0]!.events[2]!.reason).toBe("harness");
  });

  test("a Stop with nothing running is suppressed", async () => {
    const transport = new StopTransport();
    await connect(transport);
    await until(() => useApp.getState().engine.phase === "ready" && useApp.getState().sessions.length > 0);
    const id = useApp.getState().sessions[0]!.id;
    await cancel(id);
    expect(stages(id)).toEqual(["intent", "suppressed"]);
    expect(stopsSent(transport)).toHaveLength(0);
  });
});
