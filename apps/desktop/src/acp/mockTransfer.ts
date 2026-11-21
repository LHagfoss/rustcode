// Mock handlers for moving a thread between machines (`_rustcode/session/{export,import,remove}`).
// Split out of acp/mock.ts so mock.ts stays within its file-size budget.

import { arr, type Json, type JsonObject, obj, str } from "./jsonrpc";
import { configOptions } from "./mockFixtures";
import { type MockThread, update } from "./mockSeeds";
import type { HarnessSnapshot } from "./types";

type TransferMockThread = MockThread & { origins?: Record<string, JsonObject> };

export function isMockTransferMethod(method: string): boolean {
  return (
    method === "_rustcode/session/export" ||
    method === "_rustcode/session/import" ||
    method === "_rustcode/session/remove" ||
    method === "_rustcode/session/harness"
  );
}

export function handleMockTransfer(
  method: string,
  params: JsonObject,
  state: {
    getThread: (id: Json | undefined) => TransferMockThread;
    threads: () => TransferMockThread[];
    setThreads: (next: TransferMockThread[]) => void;
    isRunning: (id: string) => boolean;
    harnesses: HarnessSnapshot;
    nextId: () => string;
    notify: (msg: JsonObject) => void;
  },
): JsonObject {
  if (method === "_rustcode/session/harness") {
    const thread = state.getThread(params.sessionId);
    const id = str(params.harness) ?? "";
    const h = state.harnesses.harnesses.find((x) => x.id === id);
    if (!h || h.builtin) throw Object.assign(new Error(`cannot switch to '${id}'`), { code: -32602 });
    const account = str(params.account) ?? null;
    if (account && !(h.accounts ?? []).some((a) => a.id === account))
      throw Object.assign(new Error(`unknown ${id} account '${account}'`), { code: -32602 });
    const from = thread.harness;
    const fromAccount = (state.harnesses.harnesses.find((x) => x.id === from)?.accounts ?? []).find(
      (a) => a.id === thread.account,
    )?.name;
    thread.harness = id;
    thread.account = account ?? undefined;
    thread.model = str(params.model) ?? h.models[0]?.id ?? thread.model;
    const toAccount = (h.accounts ?? []).find((a) => a.id === account)?.name;
    const label = (name: string, login?: string) => (login ? `${name} · ${login}` : name);
    const divider = update(thread.id, {
      sessionUpdate: "agent_message_chunk",
      content: {
        type: "text",
        text: `Switched from ${label(from, fromAccount)} to ${label(id, toAccount)}`,
      },
      _meta: {
        rustcode: {
          kind: "handoff",
          from,
          to: id,
          from_account: fromAccount ?? null,
          to_account: toAccount ?? null,
          model: thread.model,
        },
      },
    });
    thread.history.push(divider);
    queueMicrotask(() => state.notify(divider));
    return {
      configOptions: configOptions(thread, state.harnesses),
      _meta: {
        rustcode: {
          harness: { id, capabilities: h.capabilities as unknown as Json },
          ...(account ? { account: { id: account } } : {}),
        },
      },
    };
  }
  if (method === "_rustcode/session/export") {
    const thread = state.getThread(params.sessionId);
    return mockExportSession(thread, state.isRunning(thread.id));
  }
  if (method === "_rustcode/session/import") {
    return mockImportSession(params, state.threads(), state.harnesses, state.nextId);
  }
  const sessionId = str(params.sessionId) ?? "";
  const before = state.threads();
  const next = before.filter((t) => t.id !== sessionId);
  state.setThreads(next);
  return { sessionId, removed: next.length !== before.length };
}

export function mockExportSession(thread: TransferMockThread, running: boolean): JsonObject {
  if (running) {
    throw Object.assign(new Error("stop the running turn before moving this thread"), { code: -32602 });
  }
  const payload = JSON.stringify({
    version: 1,
    sessionId: thread.id,
    title: thread.title,
    cwd: thread.cwd,
    harness: thread.harness,
    account: thread.account ?? null,
    model: thread.model,
    history: thread.history,
    origins: thread.origins ?? {},
  });
  const data = btoa(unescape(encodeURIComponent(payload)));
  return {
    exportId: `mock-exp-${thread.id}`,
    offset: 0,
    nextOffset: payload.length,
    totalBytes: payload.length,
    data,
    done: true,
  };
}

export function mockImportSession(
  params: JsonObject,
  threads: TransferMockThread[],
  harnesses: HarnessSnapshot,
  nextId: () => string,
): JsonObject {
  if (!params.done) {
    return { uploadId: str(params.uploadId) ?? "mock-imp-1", receivedBytes: 0, done: false };
  }
  const raw = decodeURIComponent(escape(atob(str(params.data) ?? "")));
  const bundle = obj(JSON.parse(raw) as Json) ?? {};
  const sessionId = str(bundle.sessionId) ?? nextId();
  const origins: Record<string, JsonObject> = {
    ...(obj(bundle.origins) as Record<string, JsonObject>),
  };
  const fromId = str(params.fromMachineId);
  if (fromId) {
    origins[fromId] = {
      cwd: str(bundle.cwd) ?? "",
      harness: str(bundle.harness) ?? "claude-code",
      model: str(bundle.model) ?? null,
    };
  }
  const toId = str(params.toMachineId) ?? "local";
  const prior = obj(origins[toId]);
  const cwd = str(params.cwd) || str(prior?.cwd) || str(params.suggestedCwd) || "/Users/demo/code/rustcode";
  const fromHarness = str(bundle.harness) ?? "claude-code";
  const wanted = str(params.harness) ?? fromHarness;
  const harness =
    harnesses.harnesses.find((h) => h.id === wanted && h.enabled)?.id ?? harnesses.default.harness;
  const model =
    str(params.model) ??
    harnesses.harnesses.find((h) => h.id === harness)?.models[0]?.id ??
    str(bundle.model) ??
    "sonnet-5.5";
  const fromMachine = str(params.fromMachineName) ?? "another machine";
  const toMachine = str(params.toMachineName) ?? "This Mac";
  const divider = update(sessionId, {
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: `Moved from ${fromMachine} to ${toMachine}` },
    _meta: {
      rustcode: {
        kind: "handoff",
        from: fromHarness,
        to: harness,
        from_machine: fromMachine,
        to_machine: toMachine,
        model,
      },
    },
  });
  const history = arr(bundle.history)
    .map((item) => {
      const p = obj(obj(item)?.params);
      const u = obj(p?.update);
      return u ? update(sessionId, u) : null;
    })
    .filter((item): item is JsonObject => item !== null);
  history.push(divider);
  const title = str(bundle.title) ?? "Moved thread";
  const imported: TransferMockThread = {
    id: sessionId,
    cwd,
    title,
    updatedAt: Date.now(),
    model,
    harness,
    account: str(params.account) ?? undefined,
    effort: str(params.effort) ?? "medium",
    history,
    checkpoints: [],
    origins,
  };
  const existingIdx = threads.findIndex((t) => t.id === sessionId);
  if (existingIdx >= 0) threads[existingIdx] = imported;
  else threads.unshift(imported);
  return {
    sessionId,
    cwd,
    title,
    harness,
    model,
    harnessChanged: harness !== fromHarness,
    resumedNative: false,
    usedFallbackCwd: !str(params.cwd) && !str(prior?.cwd) && !str(params.suggestedCwd),
  };
}
