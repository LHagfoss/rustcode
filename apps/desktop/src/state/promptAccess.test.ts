// The access level (Ask, Auto-edit, Full access) is the app's, not the harness's. Every prompt
// carries it in `_meta.rustcode.access`, so an engine on this machine or another one can run a
// harness that sandboxes its own shell (Codex) without the sandbox under Full access (#3438).

import { beforeEach, describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import { Method } from "../acp/types";
import { emptyExtras } from "./compose";
import { connect, resetForTests, runPrompt, selectThread, setPermissionMode, useApp } from "./store";

class RecordingTransport extends MockTransport {
  readonly requests: JsonObject[] = [];
  override send(line: string): void {
    this.requests.push(JSON.parse(line) as JsonObject);
    super.send(line);
  }
  prompts(): JsonObject[] {
    return this.requests.filter((request) => request.method === Method.sessionPrompt);
  }
}

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

beforeEach(() => resetForTests());

describe("a prompt names the thread's access level", () => {
  test("Full access, then Ask, each on the prompt that follows the change", async () => {
    const transport = new RecordingTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const id = useApp.getState().sessions[0]!.id;
    selectThread(id);
    await until(() => useApp.getState().threads[id]?.loaded === true);

    const access = (index: number) => {
      const params = (transport.prompts()[index]?.params ?? {}) as JsonObject;
      return ((params._meta ?? {}) as JsonObject).rustcode as JsonObject | undefined;
    };

    setPermissionMode(id, "full");
    await runPrompt(id, "hello", emptyExtras);
    expect(access(0)?.access).toBe("full");

    setPermissionMode(id, "ask");
    // Ask may stop on an approval the mock raises, so the prompt is not awaited: what it sent is checked.
    void runPrompt(id, "thanks", emptyExtras);
    await until(() => transport.prompts().length === 2);
    expect(access(1)?.access).toBe("ask");
  });
});
