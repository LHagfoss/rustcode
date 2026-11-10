import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { type JsonObject, RpcError } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import { harnessPlanKey, parsePlans, type UsagePlan } from "../acp/rustcode";
import type { Transport, TransportEvent } from "../acp/transport";
import { type HarnessModel, type HarnessSnapshot, Method } from "../acp/types";
import { V2Method } from "../acp/v2";
import { branchesOf, originOf } from "./branches";
import { type ComposeExtras, emptyExtras } from "./compose";
import { preferredDefault, rememberedEffort } from "./harness";
import type { AgentUsage } from "./harnessUsage";
import {
  answerOrchestration,
  linkFinished,
  linkStarted,
  linksFinished,
  linksResumed,
  orchestrationCallDropped,
  resolveGrant,
  threadTurnEnded,
  wakeTick,
} from "./host";
import { deriveInbox } from "./inbox";
import { badgeCount } from "./notify";
import {
  DEFAULT_DELEGATES_PER_THREAD,
  DEFAULT_DELEGATES_TOTAL,
  emptyBook,
  isActive,
  type Link,
} from "./orchestration";
import { pendingRequestsOn } from "./pending";
import { projectEntry, updateProjectEntry } from "./projectBook";
import { QUEUE_REASON } from "./queue";
import {
  type AppState,
  addHarnessAccount,
  agentHistory,
  answerConnectRequest,
  answerPermission,
  answerQuestionOptions,
  backgroundPlansAsk,
  type CachedPlan,
  cancel,
  cancelEdit,
  cancelQueuedEdit,
  closeReport,
  commandsFor,
  commitPlansAnswer,
  completeOnboarding,
  connect,
  createSession,
  DEFAULT_PANEL,
  dismissPendingSetup,
  editMessage,
  effortMemory,
  ensureHarnessModels,
  errorText,
  fetchHarnessModels,
  forgetPlans,
  forkThread,
  loadTotalLimit,
  localChoices,
  MAX_PANELS,
  markFinished,
  mergePlans,
  moveThread,
  newThreadChoice,
  onlyCachedPlan,
  openSideChat,
  openThread,
  PLAN_CACHE_MAX_AGE_MS,
  PLAN_CACHE_POLL_MS,
  PLAN_STALE_MARGIN_MS,
  pairRemoteDevice,
  parsePanels,
  peekThread,
  permissionModeOf,
  pickNewThreadAccountOn,
  pickNewThreadChoice,
  planCacheStale,
  planIsStale,
  planKeyFor,
  planNow,
  plansToCache,
  projectThreadChoice,
  prunePanels,
  prunePlans,
  quoteMessage,
  RESOLVED_APPROVAL_MS,
  RESUME_TEXT,
  refreshAgents,
  refreshHarnesses,
  refreshReadPlanLogins,
  refreshRemote,
  refreshSessions,
  refreshSessionsOnFocus,
  removeQueued,
  renameThread,
  request,
  resetForTests,
  resumeQueue,
  retryPendingSetup,
  returnToAttempt,
  revokeGrant,
  rewindCheckpoint,
  runPrompt,
  SETTINGS_DEFAULTS,
  saveQueuedEdit,
  selectThread,
  sendDecisionFor,
  sendQueuedNow,
  setActiveTheme,
  setArchived,
  setConfigOption,
  setDefaultCwd,
  setDefaultHarness,
  setDefaultPermissionMode,
  setDraft,
  setHarnessEnabled,
  setNewThreadChoice,
  setOrchestration,
  setPanel,
  setPermissionMode,
  setProjectCwd,
  setReduceTransparency,
  setSectionOrder,
  setSnoozed,
  setThreadLimit,
  setThreadStatus,
  setTotalLimit,
  settleThread,
  settleThreads,
  setView,
  shownPlan,
  showPanel,
  snoozeThread,
  snoozeWakeEvent,
  startNewThread,
  startQueuedEdit,
  statusOf,
  steeringOf,
  steerOrSubmit,
  submit,
  switchNewThreadHarness,
  threadCreated,
  toast,
  togglePin,
  toggleRightPanel,
  trashThread,
  unsettleThreads,
  updateQueued,
  updateThread,
  useApp,
  wakeSnoozed,
  windowNow,
} from "./store";
import { turnCompletion } from "./thread";
import { type DelegatedThread, topLevel } from "./topLevel";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

class HeldSessionLoadTransport extends MockTransport {
  readonly requests: JsonObject[] = [];
  target: string | undefined;
  heldLoad: string | undefined;

  override send(line: string): void {
    const request = JSON.parse(line) as JsonObject;
    this.requests.push(request);
    const params = request.params as JsonObject | undefined;
    if (
      request.method === Method.sessionLoad &&
      params?.sessionId === this.target &&
      this.heldLoad === undefined
    ) {
      this.heldLoad = line;
      return;
    }
    super.send(line);
  }

  releaseLoad(): void {
    const line = this.heldLoad;
    this.heldLoad = undefined;
    if (line) super.send(line);
  }
}

/**
 * The app's storage in a test (there is no window): a map `store()` writes into, seeded with an
 * entry an earlier build left. `done()` takes it away again, so the next test starts empty.
 */
function testStorage(seed: Record<string, string> = {}) {
  const data = new Map<string, string>(Object.entries(seed));
  const scope = globalThis as unknown as Record<string, unknown>;
  scope.localStorage = {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  } as unknown as Storage;
  return { data, done: () => delete scope.localStorage };
}

/** Holds one machine-status response so a test can deliver it after a newer answer. */
class DelayedRemoteStatusTransport implements Transport {
  readonly name = "mock" as const;
  private readonly mock = new MockTransport({ delayMs: 0, remote: "connect" });
  private onEvent: ((event: TransportEvent) => void) | null = null;
  private readonly methods = new Map<number, string>();
  holdNextStatus = false;
  heldResponse: string | null = null;

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    await this.mock.start((event) => {
      if (event.type === "line") {
        const response = JSON.parse(event.line) as { id?: number };
        if (
          this.holdNextStatus &&
          response.id !== undefined &&
          this.methods.get(response.id) === "_rustcode/remote"
        ) {
          this.holdNextStatus = false;
          this.heldResponse = event.line;
          return;
        }
      }
      onEvent(event);
    });
  }

  send(line: string): void {
    const message = JSON.parse(line) as { id?: number; method?: string };
    if (message.id !== undefined && message.method) this.methods.set(message.id, message.method);
    this.mock.send(line);
  }

  releaseStatus(): void {
    if (!this.heldResponse) throw new Error("no remote status response is being held");
    const line = this.heldResponse;
    this.heldResponse = null;
    this.onEvent?.({ type: "line", line });
  }

  async restart(): Promise<void> {
    await this.mock.restart();
  }

  async stop(): Promise<void> {
    await this.mock.stop();
  }
}

/** A running thread whose own record still reads no steering: the harness learned it after the thread started. */
const setStaleSteer = (id: string) =>
  updateThread(id, (t) => ({
    ...t,
    harness: t.harness
      ? {
          ...t.harness,
          capabilities: {
            ...t.harness.capabilities,
            steer: "none" as const,
            steering: { ...t.harness.capabilities.steering, live: false, attachments: false },
          },
        }
      : t.harness,
  }));

const selected = () => {
  const s = useApp.getState();
  return s.selectedId ? s.threads[s.selectedId] : undefined;
};

beforeEach(() => resetForTests());

describe("engine controller with the mock engine", () => {
  test("an old status read cannot restore a connection prompt after Accept", async () => {
    const transport = new DelayedRemoteStatusTransport();
    await connect(transport);
    await until(() => useApp.getState().status === "ready");
    try {
      const initial = await refreshRemote();
      const id = initial?.connectRequests[0]?.id;
      if (!id) throw new Error("the mock must seed a connection request");

      transport.holdNextStatus = true;
      const staleRead = refreshRemote();
      await until(() => transport.heldResponse !== null);
      expect(await answerConnectRequest(id, true)).toBe(true);
      expect(useApp.getState().remote?.connectRequests[0]?.state).toBe("approved");

      transport.releaseStatus();
      await staleRead;
      expect(useApp.getState().remote?.connectRequests[0]?.state).toBe("approved");
    } finally {
      await transport.stop();
    }
  });

  test("accepting a phone's request takes down the QR code it replaced", async () => {
    const transport = new MockTransport({ delayMs: 0, remote: "connect" });
    await connect(transport);
    await until(() => useApp.getState().status === "ready");
    try {
      const initial = await refreshRemote();
      const id = initial?.connectRequests[0]?.id;
      if (!id) throw new Error("the mock must seed a connection request");
      await pairRemoteDevice();
      expect(useApp.getState().remoteChallenge).not.toBeNull();

      expect(await answerConnectRequest(id, true)).toBe(true);
      expect(useApp.getState().remoteChallenge).toBeNull();
      expect(useApp.getState().connectCode?.requestId).toBe(id);
    } finally {
      await transport.stop();
    }
  });

  test("the migrated delegate total is written once, so a later per-thread change does not move it", () => {
    // An earlier build kept only the per-thread limit; its old twice-it tree ceiling becomes the
    // total. The migration is one-shot: the total is stored, not recomputed on every launch.
    const storage = testStorage({ "rustcode.threadLimit": "16" });
    try {
      expect(loadTotalLimit()).toBe(32);
      expect(storage.data.get("rustcode.totalLimit")).toBe("32");
      storage.data.set("rustcode.threadLimit", "32");
      expect(loadTotalLimit()).toBe(32);
      // Once a total is stored it wins, kept to its own 2..64 range.
      storage.data.set("rustcode.totalLimit", "12");
      expect(loadTotalLimit()).toBe(12);
      storage.data.set("rustcode.totalLimit", "999");
      expect(loadTotalLimit()).toBe(64);
    } finally {
      storage.done();
    }
    // A fresh install starts at the default and keeps it.
    const fresh = testStorage();
    try {
      expect(loadTotalLimit()).toBe(DEFAULT_DELEGATES_TOTAL);
      expect(fresh.data.get("rustcode.totalLimit")).toBe(String(DEFAULT_DELEGATES_TOTAL));
    } finally {
      fresh.done();
    }
  });

  test("connects, lists threads and replays one", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const s = useApp.getState();
    expect(s.sessions[0]?.title).toBe("Fix the composer focus ring");
    expect(s.projectCwd).toBe("/Users/you/code/rustcode");
    expect(s.harnesses?.onboarded).toBe(true);
    selectThread(s.sessions[0]!.id);
    await until(() => selected()?.loaded === true);
    const kinds = selected()!.items.map((i) => i.kind);
    expect(kinds).toEqual([
      "user",
      "thought",
      "tool",
      "tool",
      "tool",
      "tool",
      "tool",
      "tool",
      "assistant",
      "turnEnd",
    ]);
    expect(selected()!.configOptions.map((o) => o.id)).toEqual(["model", "effort"]);
  });

  test("commands the app answers itself never reach the engine", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const first = useApp.getState().sessions[0]!;
    selectThread(first.id);
    await until(() => selected()?.loaded === true);
    await until(() => !!useApp.getState().sessionCommands[first.id]);
    const items = selected()!.items.length;

    expect(await submit("/model")).toBe(true);
    expect(useApp.getState().modelPickerAsk).toBe(1);
    expect(await submit("/resume")).toBe(true);
    expect(useApp.getState().switcherOpen).toBe(true);

    let copied = "";
    const nav = globalThis.navigator as unknown as { clipboard?: unknown };
    const saved = nav.clipboard;
    nav.clipboard = {
      writeText: async (text: string) => {
        copied = text;
      },
    };
    expect(await submit("/copy")).toBe(true);
    await until(() => copied.length > 0);
    nav.clipboard = saved;
    const last = [...selected()!.items].reverse().find((i) => i.kind === "assistant");
    expect(copied).toBe(last?.kind === "assistant" ? last.text : "");
    expect(selected()!.items.length).toBe(items);

    expect(await submit("/clear")).toBe(true);
    const s = useApp.getState();
    expect(s.selectedId).toBeNull();
    expect(s.newThread?.harness).toBe(first.harness ?? "rustcode");
    expect(s.threads[first.id]!.items.length).toBe(items);
  });

  test("a new thread's drawer has commands before it starts, and follows its harness", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    // The catalog is asked for on connect, so a thread with no session has a menu.
    await until(() => commandsFor(useApp.getState(), null).some((c) => c.name === "/status"));
    startNewThread();
    expect(useApp.getState().selectedId).toBeNull();
    expect(commandsFor(useApp.getState(), null).map((c) => c.name)).toContain("/clear");

    // Opening a thread remembers what its harness offered, and its session list is the thread's.
    const first = useApp.getState().sessions[0]!;
    selectThread(first.id);
    await until(() => !!useApp.getState().sessionCommands[first.id]);
    expect(useApp.getState().harnessCommands[first.harness ?? "rustcode"]?.length).toBeGreaterThan(0);

    // Back to a new thread, then to a harness that has not listed anything: only the app's own
    // commands, never the list the last thread's harness left behind.
    startNewThread();
    switchNewThreadHarness("claude-code");
    const menu = commandsFor(useApp.getState(), null).map((c) => c.name);
    expect(menu).not.toContain("/status");
    expect(menu).toContain("/clear");
    expect(menu).toContain("/model");
    // The open thread still reads its own session, whatever the new-thread picker shows.
    expect(commandsFor(useApp.getState(), first.id).map((c) => c.name)).toContain("/status");
  });

  test("a chip of another harness moves the new thread there, on that account", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    switchNewThreadHarness("codex");
    expect(useApp.getState().newThread?.harness).toBe("codex");
    // The mock's Claude Code lists the `paral-2` login; pressing its chip is
    // the same move the picker makes for a new thread.
    pickNewThreadAccountOn("claude-code", "paral-2");
    const s = useApp.getState();
    expect(s.newThread?.harness).toBe("claude-code");
    expect(s.newThreadAccount).toBe("paral-2");
  });

  test("a full-access request for an unloaded thread is answered, not shown in the parent", async () => {
    const transport = new MockTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready");
    useApp.setState((s) => ({
      links: {
        ...s.links,
        links: [
          {
            id: "mock-thread-1/full",
            orchestrator: "mock-thread-1",
            target: "mock-thread-3",
            orchestratorTitle: "Parent",
            targetTitle: "Child",
            message: "do it",
            state: "working",
            createdAt: 1,
          },
        ],
      },
    }));
    setPermissionMode("mock-thread-3", "full");
    expect(useApp.getState().threads["mock-thread-3"]).toBeUndefined();
    const engine = (
      transport as unknown as {
        engine: { ask(method: string, params: JsonObject): Promise<JsonObject> };
      }
    ).engine;
    const answer = await engine.ask("session/request_permission", {
      sessionId: "mock-thread-3",
      toolCall: {
        toolCallId: "call-full",
        title: "bash",
        kind: "execute",
        rawInput: { command: "bun test" },
      },
      options: [
        { optionId: "reject", name: "Reject", kind: "reject_once" },
        { optionId: "allow", name: "Allow once", kind: "allow_once" },
      ],
    });
    // Full access answered it; loading the thread first did not turn it into a card.
    expect(answer).toMatchObject({ outcome: { outcome: "selected", optionId: "allow" } });
    const s = useApp.getState();
    expect(s.threads["mock-thread-3"]?.loaded).toBe(true);
    expect(s.threads["mock-thread-3"]?.permissions).toEqual([]);
    expect(
      pendingRequestsOn({
        id: "mock-thread-1",
        links: s.links.links,
        requests: (id) => s.threads[id]?.permissions,
        title: (id) => s.threads[id]?.title,
      }),
    ).toEqual([]);
  });

  test("a message with images queues whole while a turn runs, and stays editable", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    const image = (n: number) => ({
      id: `img${n}`,
      kind: "image" as const,
      name: `shot${n}.png`,
      mimeType: "image/png",
      data: "AAAA",
    });
    // ⌘⏎ with images on the built-in harness: the engine reads a steer as text, so the whole
    // prompt queues, in order.
    expect(await steerOrSubmit("look at this", { ...emptyExtras, attachments: [image(1), image(2)] })).toBe(
      true,
    );
    expect(await submit("", { ...emptyExtras, attachments: [image(3)] })).toBe(true);
    expect(await submit("then this")).toBe(true);
    let queue = useApp.getState().threads[id]!.queue;
    expect(queue.map((q) => [q.text, q.extras?.attachments.length ?? 0])).toEqual([
      ["look at this", 2],
      ["", 1],
      ["then this", 0],
    ]);
    // Send now refuses it by the same rule (the built-in harness takes pictures only with a turn),
    // and it stays queued with them, saying why.
    expect(await sendQueuedNow(id, queue[0]!.id)).toBe(false);
    expect(useApp.getState().threads[id]!.queue.length).toBe(3);
    expect(useApp.getState().toasts.at(-1)?.text).toBe(QUEUE_REASON.awaiting);
    // Its text can be edited or emptied; the images stay.
    updateQueued(id, queue[0]!.id, "look at these two");
    updateQueued(id, queue[1]!.id, "");
    queue = useApp.getState().threads[id]!.queue;
    expect(queue[0]).toMatchObject({
      text: "look at these two",
      extras: { attachments: [image(1), image(2)] },
    });
    expect(queue[1]).toMatchObject({ text: "", extras: { attachments: [image(3)] } });
    removeQueued(id, queue[1]!.id);
    expect(useApp.getState().threads[id]!.queue.map((q) => q.text)).toEqual([
      "look at these two",
      "then this",
    ]);
    await cancel();
    await done;
  });

  test("Send now steers pictures where the harness takes them, and a file waits", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    // Codex in the mock steers live and takes images, and is not the built-in harness.
    switchNewThreadHarness("codex");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    expect(selected()!.harness?.id).toBe("codex");
    expect(selected()!.harness?.capabilities.steer).toBe("live");
    const picture = {
      id: "img1",
      kind: "image" as const,
      name: "shot1.png",
      mimeType: "image/png",
      data: "AAAA",
    };
    const file = { id: "file1", kind: "file" as const, name: "notes.txt", path: "/tmp/notes.txt" };
    await submit("look at this", { ...emptyExtras, attachments: [picture] });
    await submit("read that", { ...emptyExtras, attachments: [file] });
    expect(selected()!.queue.map((q) => q.text)).toEqual(["look at this", "read that"]);

    // Send now takes the pictures into the running turn: the transcript shows the steered message
    // with them, and the row leaves the queue.
    expect(await sendQueuedNow(id, selected()!.queue[0]!.id)).toBe(true);
    expect(selected()!.queue.map((q) => q.text)).toEqual(["read that"]);
    const steered = selected()!.items.flatMap((i) =>
      i.kind === "user" && i.steered ? [[i.text, i.attachments?.length ?? 0] as const] : [],
    );
    expect(steered).toEqual([["look at this", 1]]);

    // A file is not steered, and stays queued with its reason.
    expect(await sendQueuedNow(id, selected()!.queue[0]!.id)).toBe(false);
    expect(selected()!.queue.map((q) => q.text)).toEqual(["read that"]);
    expect(useApp.getState().toasts.at(-1)?.text).toBe("Files go with the next turn");
    await cancel();
    await done;
  });

  test("⌘⏎ and the queued row's Send now agree, case by case", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    switchNewThreadHarness("codex");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    const picture = {
      id: "img9",
      kind: "image" as const,
      name: "shot9.png",
      mimeType: "image/png",
      data: "AAAA",
    };
    const file = { id: "file9", kind: "file" as const, name: "notes.txt", path: "/tmp/notes.txt" };
    const cases: { why: string; extras: ComposeExtras }[] = [
      { why: "plain text", extras: emptyExtras },
      { why: "a picture", extras: { links: [], attachments: [picture] } },
      { why: "a file", extras: { links: [], attachments: [file] } },
      {
        why: "a command",
        extras: {
          links: [],
          attachments: [],
          command: {
            name: "/review",
            description: "Review the diff",
            takesArguments: false,
            report: false,
            action: "execute",
          },
        },
      },
      { why: "a link chip", extras: { links: ["https://example.com"], attachments: [] } },
    ];
    const run = async (threadId: string, why: string, extras: ComposeExtras) => {
      const text = `steer ${why}`;
      // The one rule, read here exactly as ⌘⏎ and the row read it.
      const composer = sendDecisionFor(selected()!, extras).action === "steer";
      const steeredBefore = selected()!.items.filter((i) => i.kind === "user" && i.steered).length;
      expect(await steerOrSubmit(text, extras)).toBe(true);
      const steered = selected()!.items.filter((i) => i.kind === "user" && i.steered).length;
      const queued = selected()!.queue.find((q) => q.text === text);
      expect([why, steered - steeredBefore, !!queued]).toEqual([why, composer ? 1 : 0, !composer]);
      if (!queued) return;
      // The row on that same message says the same thing.
      const row = sendDecisionFor(selected()!, extras).action === "steer";
      expect([why, row]).toEqual([why, composer]);
      expect([why, await sendQueuedNow(threadId, queued.id)]).toEqual([why, row]);
      const still = selected()!.queue.some((q) => q.id === queued.id);
      expect([why, still]).toEqual([why, !row]);
      if (still) removeQueued(threadId, queued.id);
    };
    for (const { why, extras } of cases) await run(id, why, extras);
    // A thread started before its harness learned it can steer still reads "none" in its own
    // record, and the snapshot knows better: ⌘⏎ and the row both read the fresh one.
    setStaleSteer(id);
    expect(selected()!.harness?.capabilities.steering.live).toBe(false);
    expect(steeringOf(selected()!).live).toBe(true);
    for (const { why, extras } of cases) await run(id, `${why} (stale thread)`, extras);
    await cancel();
    await done;
  });

  test("⌘⏎ and Send now read the harness's freshest steering record", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    switchNewThreadHarness("codex");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    // The harness learned to steer after this thread started, so the thread's own record still
    // reads "none"; the snapshot does not, and both the composer and the row read the snapshot.
    setStaleSteer(id);
    expect(selected()!.harness?.capabilities.steering.live).toBe(false);
    expect(sendDecisionFor(selected()!, emptyExtras)).toEqual({ action: "steer" });
    expect(await steerOrSubmit("try it anyway")).toBe(true);
    expect(selected()!.queue).toEqual([]);
    expect(selected()!.items.flatMap((i) => (i.kind === "user" && i.steered ? [i.text] : []))).toEqual([
      "try it anyway",
    ]);
    // Send now agrees: a queued message the rule steers goes into the turn too.
    updateThread(id, (t) => ({ ...t, queue: [...t.queue, { id: "queued-stale", text: "still waiting" }] }));
    expect(await sendQueuedNow(id, "queued-stale")).toBe(true);
    expect(selected()!.queue).toEqual([]);
    await cancel();
    await done;
  });

  test("a stopped turn holds the queue, and Resume starts its first message", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    // Two messages wait behind the running turn, in order.
    // (The mock's turns ask for an approval unless the prompt says "yolo": these must run on.)
    await submit("yolo second");
    await submit("yolo third");
    expect(selected()!.queue.map((q) => q.text)).toEqual(["yolo second", "yolo third"]);
    expect(selected()!.queueHeld).toBeFalsy();
    // The user stops the turn: what waited stays where it is, and says why.
    await cancel();
    await done;
    await until(() => !selected()!.running);
    expect(selected()!.queueHeld).toBe(true);
    expect(selected()!.queue.map((q) => q.text)).toEqual(["yolo second", "yolo third"]);
    const sent = () =>
      selected()!
        .items.filter((i) => i.kind === "user")
        .map((i) => (i as { text: string }).text);
    expect(sent()).not.toContain("yolo second");
    // Resume starts the first one where it waits; the rest follow it as each turn completes.
    expect(await resumeQueue(id)).toBe(true);
    expect(selected()!.queueHeld).toBeUndefined();
    await until(() => !selected()!.running && selected()!.queue.length === 0, 5000);
    expect(sent().slice(-2)).toEqual(["yolo second", "yolo third"]);
  });

  test("a harness that can neither steer nor queue stops the turn, and the message runs next", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    // A harness of that shape: it takes no message mid-turn and has no queue of its own, so the
    // rule's third answer is to stop the turn and send the message in its place.
    const only = {
      live: false,
      attachments: false,
      while_awaiting_user: true,
      interrupt_restart: true,
      queue: false,
    };
    updateThread(id, (t) => ({
      ...t,
      harness: t.harness
        ? {
            ...t.harness,
            capabilities: { ...t.harness.capabilities, steer: "none" as const, steering: only },
          }
        : t.harness,
    }));
    useApp.setState((s) => ({
      harnesses: s.harnesses && {
        ...s.harnesses,
        harnesses: s.harnesses.harnesses.map((h) =>
          h.id === (selected()!.harness?.id ?? "rustcode")
            ? { ...h, capabilities: { ...h.capabilities, steering: only } }
            : h,
        ),
      },
    }));
    expect(sendDecisionFor(selected()!, emptyExtras)).toEqual({ action: "interruptRestart" });
    expect(await steerOrSubmit("do this instead")).toBe(true);
    expect(selected()!.queue.map((q) => [q.text, !!q.restart])).toEqual([["do this instead", true]]);
    // A stopped turn holds the queue for Resume; this message started the stop, so it runs on.
    await done;
    await until(() =>
      selected()!.items.some((i) => i.kind === "user" && (i as { text: string }).text === "do this instead"),
    );
    expect(selected()!.queue.map((q) => q.text)).toEqual([]);
    await cancel();
  });

  test("a message sent while idle goes behind a held queue and resumes it", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    await submit("yolo held message");
    await cancel();
    await done;
    await until(() => !selected()!.running && selected()!.queue.length === 1);
    expect(selected()!.queueHeld).toBe(true);
    await submit("yolo a new message");
    // Nothing the user wrote is skipped: the held message starts, the new one waits behind it.
    expect(selected()!.queue.map((q) => q.text)).toEqual(["yolo a new message"]);
    await until(() => selected()!.running);
    await until(() => selected()!.queue.length === 0 && !selected()!.running, 5000);
    const sent = selected()!
      .items.filter((i) => i.kind === "user")
      .map((i) => (i as { text: string }).text);
    expect(sent.slice(-2)).toEqual(["yolo held message", "yolo a new message"]);
  });

  test("a queued message is edited in the composer and saved where it waits", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    await submit("second message");
    const item = selected()!.queue[0]!;
    // What the user had typed comes back after the edit, whichever way it ends.
    setDraft(id, "my own draft");
    expect(startQueuedEdit(id, item.id)).toBe(true);
    expect(useApp.getState().drafts[id]).toBe("second message");
    setDraft(id, "second message, edited");
    expect(saveQueuedEdit()).toBe(true);
    expect(selected()!.queue[0]!.text).toBe("second message, edited");
    expect(useApp.getState().drafts[id]).toBe("my own draft");
    expect(useApp.getState().queuedEdit).toBeNull();
    // Esc leaves it as it was, and the item stays in the queue.
    expect(startQueuedEdit(id, item.id)).toBe(true);
    setDraft(id, "thrown away");
    expect(cancelQueuedEdit()).toBe(true);
    expect(selected()!.queue[0]!.text).toBe("second message, edited");
    expect(useApp.getState().drafts[id]).toBe("my own draft");
    // Removed while it is open: the edit ends, and what was typed stays when the composer was empty.
    setDraft(id, "");
    expect(startQueuedEdit(id, item.id)).toBe(true);
    setDraft(id, "kept in the composer");
    removeQueued(id, item.id);
    expect(useApp.getState().queuedEdit).toBeNull();
    expect(useApp.getState().drafts[id]).toBe("kept in the composer");
    await cancel();
    await done;
  });

  test("a report command answers in the panel and leaves the transcript as it was", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    selectThread(useApp.getState().sessions[0]!.id);
    await until(() => selected()?.loaded === true);
    await until(() => !!useApp.getState().sessionCommands[useApp.getState().sessions[0]!.id]);
    const items = selected()!.items.length;
    await submit("/status");
    await until(() => !selected()!.running && (useApp.getState().report?.text.includes("Folder") ?? false));
    expect(useApp.getState().report?.text.startsWith("**Status**")).toBe(true);
    expect(selected()!.items.length).toBe(items);
    closeReport();
    expect(useApp.getState().report).toBeNull();
  });

  test("two starts at a limit's edge take one slot, not two", async () => {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 2);
    const [caller, target] = useApp.getState().sessions.map((x) => x.id) as [string, string];
    selectThread(caller);
    await until(() => selected()?.loaded === true);
    // Full access answers the approval itself, so both calls reach the book without a user.
    setPermissionMode(caller, "full");
    setThreadLimit(2);
    setTotalLimit(16);
    try {
      const staged: Link = {
        id: `${caller}/staged`,
        orchestrator: caller,
        target,
        orchestratorTitle: "Parent",
        targetTitle: "Child",
        message: "already working",
        state: "working",
        createdAt: Date.now(),
      };
      useApp.setState({ links: { ...emptyBook(), links: [staged], counters: { [caller]: 1 } } });
      const start = (prompt: string) =>
        answerOrchestration({ sessionId: caller, tool: "start_session", args: { prompt, title: prompt } });
      // Both calls check the one free slot before either writes its link.
      const answers = await Promise.all([start("one"), start("two")]);
      const refused = answers.filter((answer) => answer.error);
      expect(refused).toHaveLength(1);
      expect((refused[0]!.error as { code?: string }).code).toBe("busy");
      const live = useApp
        .getState()
        .links.links.filter((link) => link.orchestrator === caller && isActive(link.state));
      expect(live).toHaveLength(2);
    } finally {
      setThreadLimit(DEFAULT_DELEGATES_PER_THREAD);
      setTotalLimit(DEFAULT_DELEGATES_TOTAL);
    }
  });

  test("a delegated thread must name a harness that can run and a model it lists", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const caller = useApp.getState().sessions[0]!.id;
    const start = (args: Record<string, string>) =>
      answerOrchestration({ sessionId: caller, tool: "start_session", args: { prompt: "do it", ...args } });
    const error = (r: Awaited<ReturnType<typeof start>>) =>
      (r.error as { message: string } | undefined)?.message ?? "";

    expect(error(await start({ harness: "nope" }))).toContain("the ones that can run now are rustcode");
    expect(error(await start({ harness: "claude-code" }))).toContain("Claude Code cannot run a thread");
    const wrong = error(await start({ harness: "rustcode", model: "gpt-6" }));
    expect(wrong).toContain("has no model “gpt-6”");
    expect(wrong).toContain("chatgpt/gpt-6-");
    // A named login it cannot have is refused rather than quietly ignored.
    expect(error(await start({ harness: "rustcode", account: "work" }))).toContain(
      "RustCode has no named accounts",
    );
  });

  test("list_harnesses names the accounts start_session can target, and which are signed out", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    const caller = useApp.getState().sessions[0]!.id;
    await addHarnessAccount("codex", "work", "Work");
    // The plan read found this login signed out, so naming it will be refused.
    useApp.setState({ signedOutLogins: { "codex#work": true } });
    const answer = await answerOrchestration({ sessionId: caller, tool: "list_harnesses", args: {} });
    const listed = answer.harnesses as {
      id: string;
      accounts?: { id: string; name: string; signed_out?: boolean }[];
    }[];
    expect(listed.find((h) => h.id === "claude-code")?.accounts?.map((a) => a.id)).toEqual(["paral-2"]);
    expect(listed.find((h) => h.id === "codex")?.accounts?.find((a) => a.id === "work")).toMatchObject({
      id: "work",
      name: "Work",
      signed_out: true,
    });
    // A harness with no named logins says nothing rather than an empty list.
    expect(listed.find((h) => h.id === "rustcode")?.accounts).toBeUndefined();
  });

  test("list_harnesses carries each login's usage windows from the plan cache", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
      const caller = useApp.getState().sessions[0]!.id;
      type Listed = {
        id: string;
        usage?: AgentUsage;
        accounts?: { id: string; usage?: AgentUsage }[];
      };
      const listed = async () =>
        (await answerOrchestration({ sessionId: caller, tool: "list_harnesses", args: {} }))
          .harnesses as Listed[];
      useApp.setState({ planCache: {} });
      let harnesses = await listed();
      expect(harnesses.find((h) => h.id === "claude-code")?.usage).toBeUndefined();
      expect(harnesses.find((h) => h.id === "rustcode")?.usage).toBeUndefined();

      const fetchedAt = Date.now();
      const resetsAt = fetchedAt + 4 * 3_600_000;
      const entry = (usedPercent: number, account: string | null): CachedPlan => ({
        source: "claude-code",
        harness: "claude-code",
        account,
        name: account ?? "Claude",
        plan: "max",
        windows: [{ label: "5-hour", usedPercent, minutes: 300, resetsAt }],
        fetchedAt,
      });
      useApp.setState({
        planCache: { "claude-code": entry(48, null), "claude-code#paral-2": entry(71, "paral-2") },
      });
      harnesses = await listed();
      const claude = harnesses.find((h) => h.id === "claude-code");
      expect(claude?.usage).toEqual({
        status: "ok",
        as_of: new Date(fetchedAt).toISOString(),
        windows: [
          {
            label: "5-hour",
            used_percent: 48,
            window_minutes: 300,
            resets_at: new Date(resetsAt).toISOString(),
          },
        ],
      });
      expect(claude?.accounts?.find((a) => a.id === "paral-2")?.usage).toMatchObject({
        status: "ok",
        as_of: new Date(fetchedAt).toISOString(),
        windows: [{ label: "5-hour", used_percent: 71 }],
      });
    } finally {
      storage.done();
    }
  });

  test("list_harnesses refreshes the logins it read before, never one it did not, and once per interval", async () => {
    const storage = testStorage();
    try {
      const transport = new MockTransport({ delayMs: 0 });
      type Ask = { claude?: boolean; claudeAccounts?: boolean; claudeAccount?: string; force?: boolean };
      const asked: Ask[] = [];
      const send = transport.send.bind(transport);
      transport.send = (line: string) => {
        if (line.includes("_rustcode/usage/plans")) {
          asked.push(((JSON.parse(line) as { params?: Ask }).params ?? {}) as Ask);
        }
        return send(line);
      };
      await connect(transport);
      await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
      const stale = Date.now() - PLAN_CACHE_POLL_MS - 1_000;
      const entry = (account: string | null, fetchedAt: number, usedPercent: number): CachedPlan => ({
        source: "claude-code",
        harness: "claude-code",
        account,
        name: account ?? "Claude",
        windows: [{ label: "5-hour", usedPercent, minutes: 300 }],
        fetchedAt,
      });
      // The default login was just read, one named login is old, and one is old
      // but a denied Keychain read paused it. A login nothing was read for has
      // no entry at all — reading an unseen item may prompt macOS, so it is
      // never asked for.
      useApp.setState({
        planCache: {
          "claude-code": entry(null, Date.now(), 18),
          "claude-code#paral-2": entry("paral-2", stale, 71),
          "claude-code#paused": entry("paused", stale, 5),
        },
      });
      commitPlansAnswer([
        {
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          harness: "claude-code",
          account: "paused",
          status: "unavailable",
          reason: "denied",
          windows: [],
        },
      ]);
      asked.length = 0;
      const at = Date.now();
      refreshReadPlanLogins(at);
      await until(() => asked.some((p) => p.claudeAccount === "paral-2"));
      // One login at a time, and never every account at once (that is the
      // page's own explicit look, which can read an unseen item).
      expect(asked.filter((p) => p.claudeAccounts)).toEqual([]);
      expect(asked.filter((p) => p.force)).toEqual([]);
      expect(asked.map((p) => p.claudeAccount).filter(Boolean)).toEqual(["paral-2"]);
      // Its numbers land in the cache the answer is drawn from.
      await until(() => useApp.getState().planCache["claude-code#paral-2"]?.windows[0]?.usedPercent === 4);
      // The same call again reads nothing: at most once per poll interval,
      // however often an agent asks.
      const sent = asked.length;
      refreshReadPlanLogins();
      await new Promise((r) => setTimeout(r, 20));
      expect(asked.length).toBe(sent);
      // The app's own ask is rate-limited with the rest: the logins it read are
      // stale again by the next interval, so only the throttle is in question
      // here — a call a moment later reads nothing.
      refreshReadPlanLogins(at + PLAN_CACHE_POLL_MS - 1);
      await new Promise((r) => setTimeout(r, 20));
      expect(asked.length).toBe(sent);
      // The paused login was never asked for.
      expect(asked.some((p) => p.claudeAccount === "paused")).toBe(false);
    } finally {
      storage.done();
    }
  });

  test("startup does not read Claude until a user opens Usage or selects it", async () => {
    resetForTests();
    const storage = testStorage();
    try {
      const transport = new MockTransport({ delayMs: 0 });
      const asks: { claude?: boolean }[] = [];
      const send = transport.send.bind(transport);
      transport.send = (line: string) => {
        if (line.includes("_rustcode/usage/plans")) asks.push(JSON.parse(line).params);
        return send(line);
      };
      await connect(transport);
      await until(() => useApp.getState().status === "ready" && asks.length > 0);
      expect(asks.every((ask) => !ask.claude)).toBe(true);
      commitPlansAnswer([], true);
      expect(backgroundPlansAsk().claude).toBe(true);
    } finally {
      storage.done();
    }
  });

  test("the page's own interval asks what a background read asks, never every login", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready");
      // Consent on, nothing paused: the default login and whatever account the
      // thread on screen runs on — never `claudeAccounts`, which is the page's
      // explicit look and can read a login nobody asked about.
      // Opening Usage enables the subsequent background reads.
      commitPlansAnswer([], true);
      expect(backgroundPlansAsk()).toMatchObject({ claude: true });
      expect(backgroundPlansAsk().force).toBeUndefined();
      // A denied Keychain read pauses that login, and the tick follows it.
      commitPlansAnswer([
        {
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          harness: "claude-code",
          status: "unavailable",
          reason: "denied",
          windows: [],
        },
      ]);
      expect(backgroundPlansAsk()).toEqual({ claude: false });
    } finally {
      storage.done();
    }
  });

  test("read_session says why a thread could not be opened instead of reporting no turns", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const s = useApp.getState();
    const caller = s.sessions[0]!;
    // A listed thread the engine cannot load, as one whose folder is gone is.
    const gone = { ...caller, id: "mock-gone-folder", title: "Gone folder" };
    useApp.setState({
      sessions: [...s.sessions, gone],
      permissionModes: { ...s.permissionModes, [caller.id]: "full" },
    });
    const answer = await answerOrchestration({
      sessionId: caller.id,
      tool: "read_session",
      args: { session_id: gone.id },
    });
    const message = (answer.error as { message: string } | undefined)?.message ?? "";
    expect(message).toContain("Could not open this thread");
    expect(message).toContain("Unknown session");
    expect(answer.text).toBeUndefined();
  });

  test("a thread moves to another folder, and a new thread starts in the default one", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const id = useApp.getState().sessions[0]!.id;
    selectThread(id);
    await until(() => selected()?.loaded === true);
    expect(await moveThread(id, "/tmp/elsewhere")).toBe(true);
    expect(useApp.getState().sessions.find((x) => x.id === id)?.cwd).toBe("/tmp/elsewhere");
    await until(() => selected()?.loaded === true);
    expect(selected()!.cwd).toBe("/tmp/elsewhere");

    // With no default chosen, a new thread from an open one starts in that thread's folder.
    expect(useApp.getState().defaultCwd).toBe("");
    startNewThread();
    expect(useApp.getState().projectCwd).toBe("/tmp/elsewhere");

    // The default wins over the open thread: ⌘N, the sidebar's button and the command palette start
    // where the reader said, not where the thread they were reading lives.
    selectThread(id);
    setDefaultCwd("/tmp/default");
    startNewThread();
    expect(useApp.getState().projectCwd).toBe("/tmp/default");
    // Picking another folder for this one thread keeps the default, and the next new thread starts
    // there again.
    setProjectCwd("/tmp/just-this-one");
    expect(useApp.getState().defaultCwd).toBe("/tmp/default");
    startNewThread();
    expect(useApp.getState().projectCwd).toBe("/tmp/default");
  });

  test("Full access answers the edit approval itself and the turn finishes", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    setPermissionMode(undefined, "full");
    try {
      let asked = false;
      const unsubscribe = useApp.subscribe(() => {
        if (selected()?.permissions.length) asked = true;
      });
      await submit("change the greeting");
      await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
      unsubscribe();
      expect(asked).toBe(false);
      expect(selected()!.items.at(-1)).toMatchObject({ kind: "turnEnd", outcome: "completed" });
    } finally {
      resetForTests();
    }
  });

  test("a timed approval is listed from the engine and can be revoked", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    await until(() => useApp.getState().grants.length > 0);
    const grant = useApp.getState().grants[0]!;
    expect(grant.prefix).toBe("bun test");
    await revokeGrant(grant.id);
    expect(useApp.getState().grants).toEqual([]);
  });

  test("an answered approval settles into one line, then leaves", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const thread = selected()!;
    const once = thread.permissions[0]!.options.find((o) => o.kind === "allow_once")!;
    answerPermission(thread.permissions[0]!, once);
    expect(useApp.getState().resolvedApprovals[thread.id]).toMatchObject({
      text: "Approved edit src/app.ts · once",
      allowed: true,
    });
    await done;
    await until(() => !useApp.getState().resolvedApprovals[thread.id], RESOLVED_APPROVAL_MS + 1000);
  });

  test("opening the inbox dismisses the attention toasts it lists", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    toast("A thread needs approval", "info");
    toast("Copied", "info");
    setView({ kind: "inbox" });
    expect(useApp.getState().toasts.find((t) => t.text === "A thread needs approval")?.leaving).toBe(true);
    expect(useApp.getState().toasts.some((t) => t.text === "Copied")).toBe(true);
    await until(() => !useApp.getState().toasts.some((t) => t.text === "A thread needs approval"));
  });

  test("a new thread streams a turn, asks for approval and finishes", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const thread = selected()!;
    expect(thread.running).toBe(true);
    expect(thread.permissions[0]?.diffs[0]?.path).toBe("src/app.ts");
    answerPermission(thread.permissions[0]!, thread.permissions[0]!.options[0]!);
    await done;
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
    const after = selected()!;
    expect(after.running).toBe(false);
    expect(after.items.at(-1)).toMatchObject({ kind: "turnEnd", outcome: "completed" });
    expect(
      after.items.some((i) => i.kind === "tool" && i.status === "completed" && i.diffs.length === 1),
    ).toBe(true);
    expect(after.plan.every((p) => p.status === "completed")).toBe(true);
    expect(after.usage?.size).toBe(272_000);
    expect(useApp.getState().sessions[0]?.id).toBe(after.id);
  });

  test("a question is answered with an option, and cancel answers pending requests", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const done = submit("ask me a question");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const question = selected()!.permissions[0]!;
    expect(question.question?.text).toBe("Which test runner should I use?");
    answerQuestionOptions(question, [question.options[0]!]);
    await until(
      () => selected()?.permissions[0]?.question === undefined && (selected()?.permissions.length ?? 0) > 0,
    );
    await cancel();
    await done;
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
    expect(selected()!.items.at(-1)).toMatchObject({ kind: "turnEnd", outcome: "cancelled" });
  });

  test("first run: onboarding opens, a harness is enabled, tested and made the default", async () => {
    await connect(new MockTransport({ delayMs: 0, scenario: "onboarding" }));
    await until(() => useApp.getState().harnesses !== null);
    expect(useApp.getState().onboardingOpen).toBe(true);
    await setHarnessEnabled("codex", true);
    await fetchHarnessModels("codex");
    await until(
      () => (useApp.getState().harnesses?.harnesses.find((h) => h.id === "codex")?.models.length ?? 0) > 0,
    );
    await expect(fetchHarnessModels("claude-code")).rejects.toThrow(/sign in/);
    await setDefaultHarness({ harness: "codex", model: "gpt-6-luna", effort: "low" });
    await completeOnboarding();
    const s = useApp.getState();
    expect(s.onboardingOpen).toBe(false);
    expect(s.harnesses?.default).toEqual({ harness: "codex", model: "gpt-6-luna", effort: "low" });
    // A new thread starts on the chosen harness and carries its capabilities.
    setProjectCwd("/tmp/project");
    setNewThreadChoice({ harness: "codex", model: "gpt-6-luna", effort: "low" });
    const done = submit("yolo hello");
    await until(() => selected() !== undefined);
    expect(selected()!.harness?.id).toBe("codex");
    expect(selected()!.harness?.capabilities.steer).toBe("live");
    await done;
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
  });

  test("thread actions: pin, archive, rename and trash", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().sessions.length === 6);
    const id = useApp.getState().sessions[1]!.id;
    togglePin(id);
    expect(useApp.getState().pinned).toEqual([id]);
    setArchived(id, true);
    expect(id in useApp.getState().archived).toBe(true);
    expect(await renameThread(id, "  New\n name ")).toBe(true);
    expect(useApp.getState().sessions.find((x) => x.id === id)?.title).toBe("New name");
    expect(await trashThread(id)).toBe(true);
    const s = useApp.getState();
    expect(s.sessions.some((x) => x.id === id)).toBe(false);
    expect(s.pinned).toEqual([]);
    expect(id in s.archived).toBe(false);
  });

  test("themes come from the engine, apply with their mode, and follow a change made elsewhere", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().features.themes === true);
    const names = useApp.getState().themes?.themes.map((t) => t.name);
    expect(names).toEqual(["rustcode", "oled", "light", "graphite"]);
    await setActiveTheme("light");
    expect(useApp.getState().themeActive).toBe("light");
    expect(useApp.getState().theme).toBe("light");
    // The mock announces `_rustcode/themes/changed` after a set, as the engine does for a CLI change.
    await setActiveTheme("graphite");
    await until(() => useApp.getState().themes?.active === "graphite");
    expect(useApp.getState().theme).toBe("dark");
  });

  test("a thread the engine started for a paired device joins the list once", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().sessions.length > 0);
    // `_rustcode/threads/created`: Remote access started it while a phone asked for a thread.
    threadCreated({
      sessionId: "phone-thread",
      cwd: "/tmp/project",
      harness: "claude-code",
      model: "sonnet",
    });
    const row = useApp.getState().sessions.find((x) => x.id === "phone-thread");
    expect(row?.cwd).toBe("/tmp/project");
    expect(row?.harness).toBe("claude-code");
    expect(row?.model).toBe("sonnet");
    // A second notice, or a notice without a folder, changes nothing.
    threadCreated({ sessionId: "phone-thread", cwd: "/tmp/project" });
    threadCreated({ sessionId: "no-folder" });
    const rows = useApp.getState().sessions.filter((x) => x.id === "phone-thread");
    expect(rows).toHaveLength(1);
    expect(useApp.getState().sessions.some((x) => x.id === "no-folder")).toBe(false);
  });

  test("pins live in the engine when it has them; Settled is threads marked Done", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().features.pins === true && useApp.getState().sessions.length === 6);
    const id = useApp.getState().sessions[2]!.id;
    togglePin(id);
    await new Promise((r) => setTimeout(r, 10));
    await refreshSessions();
    expect(useApp.getState().sessions.find((x) => x.id === id)?.pinned).toBe(true);
    await setThreadStatus(id, "done");
    expect(statusOf(useApp.getState(), id)).toBe("done");
  });

  test("a rewind keeps the turns it leaves as an earlier attempt", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const id = "mock-thread-attempts";
    selectThread(id);
    await until(() => useApp.getState().threads[id]?.loaded === true);
    await until(() => (useApp.getState().checkpoints[id]?.length ?? 0) === 3);
    const second = useApp.getState().checkpoints[id]!.find((c) => c.turn === 2)!;

    // The engine keeps the abandoned state, so this rewinds without asking first.
    await rewindCheckpoint(id, second);
    expect(useApp.getState().confirm).toBeNull();
    await until(() => (useApp.getState().attempts[id]?.length ?? 0) === 1);
    await until(() => useApp.getState().threads[id]?.loaded === true);

    // The live transcript is the turn above the cut; the two it left read as the attempt.
    const live = useApp.getState().threads[id]!;
    expect(live.items.filter((i) => i.kind === "turnEnd")).toHaveLength(1);
    const attempt = useApp.getState().attempts[id]![0]!;
    expect(attempt.turn).toBe(2);
    expect(attempt.items.filter((i) => i.kind === "turnEnd")).toHaveLength(2);
    // Its head was kept, as a checkpoint of its own, and the turns above it are still listed.
    expect(attempt.head).toBe(4);
    const after = useApp.getState().checkpoints[id]!;
    expect(after.map((c) => c.turn)).toEqual([1, 2, 3, 4]);
    expect(after.find((c) => c.turn === 4)?.attempt).toBe(true);

    // Going back puts the files on that attempt, and says the conversation cannot follow.
    returnToAttempt(id, attempt.id);
    const confirm = useApp.getState().confirm;
    expect(confirm?.action).toBe("Restore the Files");
    confirm!.run();
    await until(() => useApp.getState().toasts[0]?.text === "Files restored to the earlier attempt");
    expect(useApp.getState().attempts[id]).toHaveLength(1);
  });

  test("edit and resend rewinds the thread to the message first", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    await submit("yolo first");
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
    await submit("yolo second");
    await until(() => selected()?.items.filter((i) => i.kind === "turnEnd").length === 2);
    const thread = selected()!;
    const first = thread.items.find((i) => i.kind === "user")!;
    editMessage(thread.id, first.id);
    expect(useApp.getState().drafts[thread.id]).toBe("yolo first");
    expect(useApp.getState().editing[thread.id]?.userIndex).toBe(0);
    await submit("yolo first, edited");
    // Later messages go too, so it asks first.
    const confirm = useApp.getState().confirm;
    expect(confirm?.action).toBe("Send");
    confirm!.run();
    await until(
      () => selected()?.items.filter((i) => i.kind === "turnEnd").length === 1 && !selected()!.running,
    );
    const users = selected()!
      .items.filter((i) => i.kind === "user")
      .map((i) => (i as { text: string }).text);
    expect(users).toEqual(["yolo first, edited"]);
    // The turn changed a file: its completion summary says so.
    const end = selected()!.items.findIndex((i) => i.kind === "turnEnd");
    expect(turnCompletion(selected()!.items, end)?.files).toEqual([
      { path: "src/app.ts", added: 1, removed: 1 },
    ]);
    expect(turnCompletion(selected()!.items, end)?.checks[0]?.outcome).toBe("passed");
  });

  test("editing a message sent with a long paste keeps the paste in its text", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    const paste = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    const typed = "yolo why does this fail?";
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "p1", kind: "text" as const, text: paste, at: typed.length }],
    };
    await submit(typed, extras);
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
    const thread = selected()!;
    const first = thread.items.find((i) => i.kind === "user")!;
    // The message says the paste where it was pasted, as plain text, and keeps no chip for it.
    expect((first as { text: string }).text).toBe(`${typed}\n${paste}`);
    expect(first.attachments).toBeUndefined();
    editMessage(thread.id, first.id);
    expect(useApp.getState().drafts[thread.id]).toBe(`${typed}\n${paste}`);
    // The paste is already in the text: the composer does not hold a second copy of it.
    expect(useApp.getState().extras[thread.id]?.attachments).toEqual([]);
    // Editing another message leaves that one's paste behind: it belongs to the message it came with.
    cancelEdit(thread.id);
    await submit("yolo second");
    await until(() => selected()?.items.filter((i) => i.kind === "turnEnd").length === 2);
    const second = selected()!
      .items.filter((i) => i.kind === "user")
      .at(-1)!;
    editMessage(thread.id, second.id);
    expect(useApp.getState().drafts[thread.id]).toBe("yolo second");
    expect(useApp.getState().extras[thread.id]?.attachments).toEqual([]);
  });

  test("a turn that starts agents lists them, and their conversation reads", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    await submit("yolo split this between agents");
    await until(() => selected()?.items.at(-1)?.kind === "turnEnd");
    const id = selected()!.id;
    const agents = await refreshAgents(id);
    expect(agents.map((a) => a.agentType)).toEqual(["explorer", "worker"]);
    expect(agents.every((a) => a.status === "completed")).toBe(true);
    const history = await agentHistory(id, 1);
    expect(history.items.map((i) => i.kind)).toEqual(["user", "tool", "assistant"]);
    await until(() => useApp.getState().planUsage[id] !== undefined);
    expect(useApp.getState().planUsage[id]?.windows).toHaveLength(2);
  });

  test("a new thread gets the chosen effort even when session/new ignores it", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready");
    setProjectCwd("/tmp/project");
    setNewThreadChoice({ harness: "rustcode", model: "chatgpt/gpt-6-luna", effort: "low" });
    const done = submit("yolo hi");
    await until(() => selected()?.configOptions.find((o) => o.id === "effort")?.current === "low");
    await done;
  });

  test("a fan-out asks once, starts a worktree thread per harness and returns their answers", async () => {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.worktrees === true);
    setProjectCwd("/Users/you/code/rustcode");
    await submit("fan out: update the greeting");
    await until(() => useApp.getState().grant !== null);
    expect(useApp.getState().grant?.kind).toBe("fan_out");
    resolveGrant("always");
    await until(() => useApp.getState().links.fanOuts.length === 1);
    const fan = useApp.getState().links.fanOuts[0]!;
    expect(fan.links).toHaveLength(2);
    await until(() => !selected()?.running, 5000);
    const s = useApp.getState();
    const targets = s.links.links.map((l) => l.target);
    expect(targets.every((t) => s.threads[t]?.worktree?.branch.startsWith("feat/"))).toBe(true);
    expect(s.links.links.every((l) => l.state === "done")).toBe(true);
    const reply = selected()!
      .items.filter((i) => i.kind === "assistant")
      .at(-1);
    expect(reply?.kind === "assistant" && reply.text).toContain("Here is what came back");
    // "Allow for This Thread" is kept: the next start does not ask.
    expect(s.links.grants[selected()!.id]).toContain("start");
  });

  test("a wait_session its ended turn left behind stops swallowing the settled child's wake", async () => {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.worktrees === true);
    setProjectCwd("/Users/you/code/rustcode");
    const parent = useApp.getState().sessions[0]!.id;
    selectThread(parent);
    await until(() => selected()?.loaded === true);
    // The turn that asks: it runs, so the wait it opens belongs to one.
    updateThread(parent, (t) => ({ ...t, running: true }));
    const starting = answerOrchestration({
      sessionId: parent,
      tool: "start_session",
      args: { prompt: "do it (yolo)", title: "Child" },
    });
    await until(() => useApp.getState().grant !== null);
    resolveGrant("once");
    const link = String((await starting).link_id);
    const linkOf = () => useApp.getState().links.links.find((l) => l.id === link);
    // A wait that outlives the turn that opened it: the user stopped it, or the harness dropped the call.
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      args: { link_ids: [link], timeout_ms: 5000 },
    });
    updateThread(parent, (t) => ({ ...t, running: false }));
    // The child settling is the parent's news, not a wait's answer nobody reads: it is woken with it.
    const woken = () =>
      useApp.getState().threads[parent]!.items.some((i) => i.kind === "user" && !!i.wake) ||
      useApp.getState().threads[parent]!.queue.some((q) => !!q.wake);
    await until(woken, 3000);
    expect(linkOf()?.notifiedAt).toBeDefined();
    await waited;
  });

  /**
   * A thread with one live link to another, staged in the book: these tests drive
   * the link's turn and the engine's word on the call by hand, so no mock turn
   * race decides what they check.
   */
  const stagedWait = async () => {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [parent, target] = useApp.getState().sessions.map((x) => x.id) as [string, string];
    selectThread(parent);
    await until(() => selected()?.loaded === true);
    // The turn that asks: it runs, so the wait belongs to one.
    updateThread(parent, (t) => ({ ...t, running: true }));
    const link: Link = {
      id: `${parent}/1`,
      orchestrator: parent,
      target,
      orchestratorTitle: "Parent",
      targetTitle: "Child",
      message: "do it",
      state: "working",
      createdAt: Date.now(),
    };
    useApp.setState({
      links: { ...emptyBook(), links: [link], counters: { [parent]: 1 } },
    });
    const linkOf = () => useApp.getState().links.links.find((l) => l.id === link.id);
    return { parent, target, link, linkOf };
  };

  /** The wakes the app queued or wrote for a thread: one message per batch of children. */
  const wakes = (thread: string) =>
    useApp.getState().threads[thread]?.queue.filter((q) => q.wake).length ?? 0;

  const wakeText = (thread: string) =>
    (useApp.getState().threads[thread]?.queue ?? [])
      .filter((q) => q.wake)
      .map((q) => q.text)
      .join("\n");

  /** The links the queued wake rows already report: delivered when their turn starts, not before. */
  const wakeLinks = (thread: string) =>
    (useApp.getState().threads[thread]?.queue ?? []).flatMap((q) => q.wakeLinks ?? []);

  /** What each wake turn this thread ran was told: the app's own words, never a user message. */
  const wakeTurns = (thread: string) =>
    (useApp.getState().threads[thread]?.items ?? [])
      .filter((i) => i.kind === "user" && !!i.wake)
      .map((i) => (i as { text: string }).text);

  /**
   * Runs the wake row waiting at the head of `thread`'s queue. The mock asks before it edits, and a
   * wake that reports one thing can start another one after it, so every approval is answered.
   */
  const runQueuedWake = async (thread: string) => {
    updateThread(thread, (t) => ({ ...t, running: false }));
    let done = false;
    const run = resumeQueue(thread).then(() => {
      done = true;
    });
    for (let i = 0; i < 250 && !done; i++) {
      const request = useApp.getState().threads[thread]?.permissions[0];
      if (request) answerPermission(request, request.options[0]!);
      await new Promise((r) => setTimeout(r, 20));
    }
    if (!done) throw new Error("the queued wake never finished");
    await run;
  };

  test("a wait whose call is dropped frees its links, and the child's answer wakes the parent", async () => {
    // The method the engine names in `orchestration::CANCELLED_METHOD`.
    expect(V2Method.orchestrationCancelled).toBe("_rustcode/orchestration/cancelled");
    const { parent, link, target, linkOf } = await stagedWait();
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-1",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    // The harness stopped waiting for the call: the wait ends there and then, not in an hour.
    orchestrationCallDropped({ sessionId: parent, callId: "call-1" });
    const answer = (await waited) as { status?: string; links?: unknown[] };
    expect(answer.status).toBe("dropped");
    expect(answer.links).toEqual([]);
    // Nothing was marked heard: nobody read that answer.
    expect(linkOf()?.notifiedAt).toBeUndefined();
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    await until(() => wakes(parent) === 1, 3000);
    expect(wakes(parent)).toBe(1);
    expect(wakeText(parent)).toContain("the child's answer");
    // The row reports the link; the mark is written when its turn starts, not when it is queued
    // (a queue is memory only: a mark now would outlive a restart that the row does not).
    expect(wakeLinks(parent)).toEqual([link.id]);
    expect(linkOf()?.notifiedAt).toBeUndefined();
  });

  /** Records what the app asks the engine, and holds the answer to a merge until released. */
  class MergeTransport extends MockTransport {
    readonly requests: JsonObject[] = [];
    held: string | undefined;
    override send(line: string): void {
      const request = JSON.parse(line) as JsonObject;
      this.requests.push(request);
      if (request.method === Method.pullMerge) {
        this.held = line;
        return;
      }
      super.send(line);
    }
    release(): void {
      const line = this.held;
      this.held = undefined;
      if (line) super.send(line);
    }
    asked(method: string): JsonObject[] {
      return this.requests.filter((r) => r.method === method);
    }
  }

  const stagedMerge = async () => {
    setOrchestration(true);
    const transport = new MergeTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [parent, target] = useApp.getState().sessions.map((x) => x.id) as [string, string];
    // One project: a merge reaches only this project's threads.
    useApp.setState((st) => ({
      sessions: st.sessions.map((x) => ({ ...x, cwd: "/Users/you/code/rustcode" })),
    }));
    selectThread(parent);
    await until(() => selected()?.loaded === true);
    updateThread(parent, (t) => ({ ...t, running: true, cwd: "/Users/you/code/rustcode" }));
    updateThread(target, (t) => ({ ...t, cwd: "/Users/you/code/rustcode" }));
    const link: Link = {
      id: `${parent}/1`,
      orchestrator: parent,
      target,
      orchestratorTitle: "Parent",
      targetTitle: "Child",
      message: "do it",
      state: "working",
      createdAt: Date.now(),
    };
    useApp.setState({ links: { ...emptyBook(), links: [link], counters: { [parent]: 1 } } });
    return { parent, target, transport };
  };

  test("thread status reports a running delegate's idle time and provider failure", async () => {
    const { parent, target, transport } = await stagedMerge();
    await openThread(target);
    const at = Date.now() - 5000;
    updateThread(target, (thread) => ({
      ...thread,
      running: true,
      items: [{ kind: "notice", id: "last-activity", at, text: "Working", tone: "info" }],
    }));
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "thread_status",
      args: { session_id: target },
    });
    const delegate = (answer.delegates as JsonObject[])[0]!;
    expect(delegate.idle_ms).toBeGreaterThanOrEqual(5000);
    expect(delegate.last_provider_error).toMatchObject({ status_code: 429, error_kind: "rate_limit" });
    expect(transport.asked(Method.dataActivity)[0]?.params).toMatchObject({
      sessionId: target,
      order: "newest",
    });
    updateThread(target, (thread) => ({
      ...thread,
      items: [
        { kind: "notice", id: "older-activity", at: Date.now() - 120_000, text: "Working", tone: "info" },
      ],
    }));
    const updated = await answerOrchestration({
      sessionId: parent,
      tool: "thread_status",
      args: { session_id: target },
    });
    expect((updated.delegates as JsonObject[])[0]!.idle_ms).toBeLessThan(90_000);
  });

  test("read_activity forwards newest order to the data store", async () => {
    const { parent, target, transport } = await stagedMerge();
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "read_activity",
      args: { session_id: target, order: "newest", limit: 2 },
    });
    expect(answer.error).toBeUndefined();
    expect(transport.asked(Method.dataActivity)[0]?.params).toMatchObject({
      sessionId: target,
      order: "newest",
      limit: 2,
    });
  });

  test("a merge whose call is dropped while it waits for the approval never merges", async () => {
    const { parent, target, transport } = await stagedMerge();
    const merged = answerOrchestration({
      sessionId: parent,
      tool: "merge_when_green",
      callId: "call-m1",
      args: { session_id: target },
    });
    await until(() => useApp.getState().grant !== null, 3000);
    // The MCP server told the agent "pending" and dropped the call: its card goes with it.
    orchestrationCallDropped({ sessionId: parent, callId: "call-m1" });
    expect(useApp.getState().grant).toBeNull();
    const answer = (await merged) as { error?: { code?: string } };
    expect(answer.error?.code).toBe("cancelled");
    // A late click on a card that is gone changes nothing: no merge was ever asked for.
    resolveGrant("once");
    await new Promise((r) => setTimeout(r, 20));
    expect(transport.asked(Method.pullMerge)).toHaveLength(0);
  });

  test("a merge whose call is dropped while the engine waits for checks is cancelled there", async () => {
    const { parent, target, transport } = await stagedMerge();
    const merged = answerOrchestration({
      sessionId: parent,
      tool: "merge_when_green",
      callId: "call-m2",
      args: { session_id: target },
    });
    await until(() => useApp.getState().grant !== null, 3000);
    resolveGrant("once");
    await until(() => transport.asked(Method.pullMerge).length === 1, 3000);
    expect((transport.asked(Method.pullMerge)[0]!.params as JsonObject).mergeId).toBe("call-m2");
    orchestrationCallDropped({ sessionId: parent, callId: "call-m2" });
    await until(() => transport.asked(Method.pullMergeCancel).length === 1, 3000);
    expect((transport.asked(Method.pullMergeCancel)[0]!.params as JsonObject).mergeId).toBe("call-m2");
    transport.release();
    await merged;
  });

  test("a finished link records its harness and how long its turn worked", async () => {
    const { link, target, linkOf } = await stagedWait();
    linkStarted(link.id);
    expect(linkOf()?.startedAt).toBeDefined();
    // The turn began 3m 49s before it answered.
    const started = Date.now() - 229_000;
    useApp.setState((s) => ({
      links: {
        ...s.links,
        links: s.links.links.map((l) => (l.id === link.id ? { ...l, startedAt: started } : l)),
      },
    }));
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    const settled = linkOf();
    expect(settled?.workedMs).toBeGreaterThanOrEqual(229_000);
    // The target's own harness names it in the notice ("Pi", "Codex", …).
    expect(settled?.harness).toBeDefined();
    expect(settled?.harnessName).toBeDefined();
  });

  test("a child that finishes as the call drops is delivered exactly once", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-2",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    // The child settles: the wait hands its answer over, and marks it heard.
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    const answer = (await waited) as { links?: { status?: string }[] };
    expect(answer.links?.[0]?.status).toBe("done");
    expect(linkOf()?.notifiedAt).toBeDefined();
    // At that very moment the call is dropped too: the engine threw that answer away.
    orchestrationCallDropped({ sessionId: parent, callId: "call-2" });
    await until(() => wakes(parent) === 1, 3000);
    // One delivery, not two, and not none.
    await new Promise((r) => setTimeout(r, 500));
    expect(wakes(parent)).toBe(1);
    expect(wakeText(parent)).toContain("the child's answer");
    expect(wakeLinks(parent)).toEqual([link.id]);
  });

  test("a wake queued for a busy parent marks its links when its turn starts", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    await until(() => wakes(parent) === 1, 3000);
    // Queued and unmarked: the report is the row's, not the book's, until it runs.
    expect(linkOf()?.notifiedAt).toBeUndefined();
    await runQueuedWake(parent);
    // Its turn started with the answer reported, and nothing reports it a second time.
    expect(linkOf()?.notifiedAt).toBeDefined();
    expect(wakes(parent)).toBe(0);
    await new Promise((r) => setTimeout(r, 500));
    expect(wakes(parent)).toBe(0);
  });

  test("a tick does not mark a replaced answer for good: a stopped successor leaves it due", async () => {
    const { parent, target } = await stagedWait();
    const at = Date.now() - 60_000;
    const first: Link = {
      id: `${parent}/1`,
      orchestrator: parent,
      target,
      orchestratorTitle: "Parent",
      targetTitle: "Child",
      message: "do it",
      state: "done",
      answer: "the first answer",
      createdAt: at,
      settledAt: at,
    };
    const followUp: Link = { ...first, id: `${parent}/2`, state: "working", createdAt: at + 1_000 };
    useApp.setState({ links: { ...emptyBook(), links: [first, followUp], counters: { [parent]: 2 } } });
    // The parent messaged that thread since: the first answer is out of date, so no wake for it…
    wakeTick();
    expect(wakes(parent)).toBe(0);
    // …and nothing is written down for it: a mark here would outlive the successor that replaced it.
    expect(useApp.getState().links.links[0]?.notifiedAt).toBeUndefined();
    // The follow-up is interrupted by a restart without ever answering: the first answer is due.
    const interrupted: Link = { ...followUp, state: "interrupted" };
    useApp.setState({
      links: { ...emptyBook(), links: [first, interrupted], counters: { [parent]: 2 } },
    });
    wakeTick();
    await until(() => wakes(parent) === 1, 3000);
    expect(wakeText(parent)).toContain("the first answer");
  });

  test('a row queued as "needs you" keeps the child\'s finish, which arrives once', async () => {
    const { parent, link, target } = await stagedWait();
    // The child asks the user while the parent's turn still runs: the notice waits in its queue.
    const request = {
      rpcId: 42,
      sessionId: target,
      toolCallId: "call-q",
      title: "Run the checks",
      options: [{ id: "allow", name: "Allow", kind: "allow_once" as const }],
      diffs: [],
    };
    // The child's thread has to be open for its pending request to be the app's to see.
    await openThread(target);
    await until(() => !!useApp.getState().threads[target], 3000);
    updateThread(target, (t) => ({ ...t, permissions: [request] }));
    wakeTick();
    await until(() => wakes(parent) === 1, 3000);
    expect(wakeText(parent)).toContain("Delegate asks:");
    expect(wakeLinks(parent)).toEqual([link.id]);
    // The user answers, and the child finishes before the parent's turn ends: the queued row still
    // says "Delegate asks", and the finish has not been reported to anyone.
    updateThread(target, (t) => ({ ...t, permissions: [] }));
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    await new Promise((r) => setTimeout(r, 200));
    expect(wakes(parent)).toBe(1);
    expect(wakeTurns(parent)).toHaveLength(0);
    // The row runs: it told the parent about a question, so the finish is still owed, and comes once.
    await runQueuedWake(parent);
    await until(() => wakeTurns(parent).some((t) => t.includes("the child's answer")), 5000);
    const finishes = wakeTurns(parent).filter((t) => t.includes("Delegate finished:"));
    expect(finishes).toHaveLength(1);
    expect(finishes[0]).toContain("the child's answer");
    expect(useApp.getState().links.links.find((l) => l.id === link.id)?.notifiedAt).toBeDefined();
    // Nothing reports it a second time.
    await new Promise((r) => setTimeout(r, 500));
    expect(wakeTurns(parent).filter((t) => t.includes("the child's answer"))).toHaveLength(1);
  }, 20_000);

  test("a start-up does not wake a result that was already delivered", async () => {
    const transport = new MockTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    setOrchestration(true);
    const [parent, target] = useApp.getState().sessions.map((x) => x.id) as [string, string];
    const at = Date.now() - 600_000;
    await request(Method.dataLinksSave, {
      book: {
        links: [
          {
            id: `${parent}/1`,
            orchestrator: parent,
            target,
            orchestratorTitle: "Parent",
            targetTitle: "Child",
            message: "do it",
            state: "done",
            answer: "the child's answer",
            createdAt: at,
            settledAt: at,
            notifiedAt: at + 1_000,
          },
        ],
        grants: {},
        counters: {},
        fanOuts: [],
      },
    });
    resetForTests();
    setOrchestration(true);
    await connect(transport);
    await until(() => useApp.getState().status === "ready");
    // Long enough for the start-up tick to open the parent and start a turn, were it going to.
    await new Promise((r) => setTimeout(r, 2500));
    expect(wakeTurns(parent)).toHaveLength(0);
    expect(useApp.getState().threads[parent]?.queue ?? []).toHaveLength(0);
    expect(useApp.getState().links.links[0]?.notifiedAt).toBe(at + 1_000);
  }, 20_000);

  test("a start-up reads the stored book and wakes what nobody was told about", async () => {
    // An engine whose stored book holds a child result that never reached its parent: the window that
    // queued it was closed first, so only the book knows.
    const transport = new MockTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    setOrchestration(true);
    const [parent, target] = useApp.getState().sessions.map((x) => x.id) as [string, string];
    const at = Date.now() - 60_000;
    await request(Method.dataLinksSave, {
      book: {
        links: [
          {
            id: `${parent}/1`,
            orchestrator: parent,
            target,
            orchestratorTitle: "Parent",
            targetTitle: "Child",
            message: "do it",
            state: "done",
            answer: "the child's answer",
            createdAt: at,
            settledAt: at,
          },
        ],
        grants: {},
        counters: {},
        fanOuts: [],
      },
    });
    // The same engine, a new window: its queue is memory and starts empty, its book is what it reads.
    resetForTests();
    setOrchestration(true);
    await connect(transport);
    await until(() => useApp.getState().status === "ready");
    // Nothing here ticks the wake machinery: getting ready is what does it (`syncLinkBook` then tick).
    const wakeTurn = () =>
      (useApp.getState().threads[parent]?.items ?? []).some((i) => i.kind === "user" && !!i.wake);
    await until(wakeTurn, 8000);
    expect(wakeTurn()).toBe(true);
    expect(useApp.getState().links.links.find((l) => l.id === `${parent}/1`)?.notifiedAt).toBeDefined();
  }, 20_000);

  test("a child a restart interrupted answers its parent once the app resumes it", async () => {
    // The window was quit with the child mid-turn: the book it saved still says the link works, and
    // app storage still lists the thread the quit cut off (state/store.ts `interruptedThreads`).
    const parent = "mock-thread-1";
    const target = "mock-thread-2";
    const at = Date.now() - 60_000;
    const link: Link = {
      id: `${parent}/1`,
      orchestrator: parent,
      target,
      orchestratorTitle: "Fix the composer focus ring",
      targetTitle: "Explain the sidecar restart backoff",
      message: "look into the backoff",
      state: "working",
      createdAt: at,
      startedAt: at,
    };
    const book = { links: [link] as unknown as JsonObject[], grants: {}, counters: {}, fanOuts: [] };
    const transport = new MockTransport({ delayMs: 0, links: book as unknown as JsonObject });
    const storage = testStorage({
      "rustcode.threadLinks": JSON.stringify({ ...book, counters: { [parent]: 1 } }),
      "rustcode.runningThreads": JSON.stringify([target]),
    });
    try {
      setOrchestration(true);
      await connect(transport);
      await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
      setOrchestration(true);
      const linkOf = () => useApp.getState().links.links.find((l) => l.id === link.id);
      // The restart severed it on load, and the resume turn the app runs takes it back as its own
      // (state/host.ts `linksResumed`): from here the link works again, counting from the resume, so
      // only that turn settles it.
      await until(() => linkOf()?.resumedAt !== undefined, 8000);
      expect(linkOf()?.state).toBe("working");
      expect(linkOf()?.startedAt).toBe(linkOf()?.resumedAt);
      const resumed = () =>
        (useApp.getState().threads[target]?.items ?? []).some(
          (i) => i.kind === "user" && (i as { text?: string }).text === RESUME_TEXT,
        );
      await until(resumed, 8000);
      // The resumed turn asks before it edits, as a real harness does: allow it, so the turn can
      // end as the branch's answer rather than wait on the user.
      for (let i = 0; i < 300 && linkOf()?.state === "working"; i++) {
        const request = useApp.getState().threads[target]?.permissions[0];
        if (request) answerPermission(request, request.options[0]!);
        await new Promise((r) => setTimeout(r, 20));
      }
      // Its continue turn ended as the child's answer, and the parent reads it in a wake.
      await until(() => wakeTurns(parent).length > 0, 10_000);
      expect(wakeTurns(parent).join("\n")).toContain("Delegate finished");
      expect(wakeTurns(parent).join("\n")).toContain("Explain the sidecar restart backoff");
      expect(linkOf()?.state).toBe("done");
      expect(linkOf()?.notifiedAt).toBeDefined();
      // Once. Nothing reports the same branch again on the next tick.
      wakeTick();
      await new Promise((r) => setTimeout(r, 500));
      expect(wakeTurns(parent)).toHaveLength(1);
    } finally {
      storage.done();
    }
  }, 30_000);

  /**
   * A book staged in the app, as a restart leaves it: `links` are interrupted links whose turn the
   * quit cut off, and the threads they name are opened so a wake for one of them lands here.
   */
  const stagedRestart = async (links: Link[], open: readonly string[]) => {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 2);
    setOrchestration(true);
    for (const id of open) {
      selectThread(id);
      await until(() => selected()?.loaded === true, 5000);
    }
    useApp.setState((s) => ({ links: { ...emptyBook(), links, counters: s.links.counters } }));
    const linkOf = (id: string) => useApp.getState().links.links.find((l) => l.id === id);
    return { linkOf };
  };

  /** Answers whatever the mock asks in `threads`, so the turns it runs can end by themselves. */
  const answerAsking = async (threads: readonly string[], untilDone: () => boolean) => {
    for (let i = 0; i < 400 && !untilDone(); i++) {
      for (const id of threads) {
        const request = useApp.getState().threads[id]?.permissions[0];
        if (request) answerPermission(request, request.options[0]!);
      }
      await new Promise((r) => setTimeout(r, 20));
    }
  };

  test("the links a resume turn answers settle in one write, and their clock starts at the resume", async () => {
    // A restart caught this child working on two links: the user had queued a follow-up in one and
    // an approval had stopped the clock of the other. Both are the resume turn's to answer.
    const parent = "mock-thread-1";
    const child = "mock-thread-2";
    const at = Date.now() - 600_000;
    const link = (n: number, extra: Partial<Link> = {}): Link => ({
      id: `${parent}/${n}`,
      orchestrator: parent,
      target: child,
      orchestratorTitle: "Fix the composer focus ring",
      targetTitle: "Explain the sidecar restart backoff",
      message: "look into the backoff",
      state: "interrupted",
      createdAt: at,
      startedAt: at,
      ...extra,
    });
    const staged = await stagedRestart([link(1, { waitingSince: at, waitedMs: 30_000 }), link(2)], [parent]);
    const say = [{ kind: "assistant" as const, id: "m", text: "the resume answered" }];
    const resumedAt = Date.now();
    linksResumed([`${parent}/1`, `${parent}/2`]);
    for (const id of [`${parent}/1`, `${parent}/2`]) {
      // Working again from the resume, with the clock and the wait the restart left behind cleared.
      const resumed = staged.linkOf(id);
      expect(resumed?.state).toBe("working");
      expect(resumed?.resumedAt).toBeGreaterThanOrEqual(resumedAt);
      expect(resumed?.startedAt).toBe(resumed?.resumedAt);
      expect(resumed?.waitingSince).toBeUndefined();
      expect(resumed?.waitedMs).toBeUndefined();
    }
    await new Promise((r) => setTimeout(r, 5));
    linksFinished([`${parent}/1`, `${parent}/2`], child, "completed", undefined, say);
    const [first, second] = [`${parent}/1`, `${parent}/2`].map(staged.linkOf);
    // One turn, one write to the book: both settled together, and each worked from the resume on.
    expect(first?.state).toBe("done");
    expect(second?.state).toBe("done");
    expect(first?.settledAt).toBe(second?.settledAt);
    expect(first?.workedMs).toBeLessThan(60_000);
    expect(second?.workedMs).toBe(first?.workedMs);
    // One coalesced wake, naming both, and the marker that says the answer came after a restart.
    await until(() => wakeTurns(parent).length === 1, 5000);
    const told = wakeTurns(parent).join("\n");
    expect(told).toContain("2 delegates finished");
    expect(told).toContain(`${parent}/1`);
    expect(told).toContain(`${parent}/2`);
    expect(told).toContain("resumed after a restart");
  }, 30_000);

  test("a turn the user starts later never settles a link a restart interrupted", async () => {
    const parent = "mock-thread-1";
    const child = "mock-thread-2";
    const at = Date.now() - 600_000;
    const staged = await stagedRestart(
      [
        {
          id: `${parent}/1`,
          orchestrator: parent,
          target: child,
          orchestratorTitle: "Parent",
          targetTitle: "Child",
          message: "do it",
          state: "interrupted",
          createdAt: at,
          startedAt: at,
        },
      ],
      [parent],
    );
    // The user asks the child something of their own: that turn is their work, not the branch's
    // answer, so the link stays interrupted and the parent is not woken for it.
    threadTurnEnded(child, false, "completed", undefined, [
      { kind: "assistant", id: "m", text: "something the user asked for" },
    ]);
    await new Promise((r) => setTimeout(r, 300));
    expect(staged.linkOf(`${parent}/1`)?.state).toBe("interrupted");
    expect(staged.linkOf(`${parent}/1`)?.notifiedAt).toBeUndefined();
    expect(wakeTurns(parent)).toEqual([]);
  }, 30_000);

  test("a resume that finds the thread busy waits in its queue, and its turn then answers the link", async () => {
    // The engine outlived the window, so the child's turn still runs when the app resumes it: the
    // Continue message queues behind that turn, and nothing is settled until a turn carries it.
    const parent = "mock-thread-1";
    const child = "mock-thread-2";
    const at = Date.now() - 60_000;
    const staged = await stagedRestart(
      [
        {
          id: `${parent}/1`,
          orchestrator: parent,
          target: child,
          orchestratorTitle: "Parent",
          targetTitle: "Child",
          message: "do it",
          state: "interrupted",
          createdAt: at,
          startedAt: at,
        },
      ],
      [parent, child],
    );
    updateThread(child, (t) => ({ ...t, running: true }));
    await runPrompt(child, RESUME_TEXT, undefined, { automatic: true, resumeLinks: [`${parent}/1`] });
    // Not dropped: the row waits behind the turn, with the links it will answer when it starts.
    const row = useApp.getState().threads[child]?.queue.at(-1);
    expect(row?.text).toBe(RESUME_TEXT);
    expect(row?.resume).toEqual([`${parent}/1`]);
    expect(staged.linkOf(`${parent}/1`)?.state).toBe("interrupted");
    // That turn ends, the row starts, and from there the link works for it.
    updateThread(child, (t) => ({ ...t, running: false }));
    const run = resumeQueue(child);
    await until(() => staged.linkOf(`${parent}/1`)?.state === "working", 3000);
    expect(staged.linkOf(`${parent}/1`)?.resumedAt).toBeDefined();
    for (let i = 0; i < 300 && staged.linkOf(`${parent}/1`)?.state === "working"; i++) {
      const request = useApp.getState().threads[child]?.permissions[0];
      if (request) answerPermission(request, request.options[0]!);
      await new Promise((r) => setTimeout(r, 20));
    }
    await run;
    expect(staged.linkOf(`${parent}/1`)?.state).toBe("done");
    await until(() => wakeTurns(parent).length === 1, 5000);
  }, 30_000);

  test("a resumed middle thread holds its parent back until its own leaf's resume is done", async () => {
    // Three levels, all cut off by the restart: the top's link to the middle and the middle's own
    // link to the leaf. The app resumes both; the middle's continue ends first.
    const top = "mock-thread-1";
    const middle = "mock-thread-2";
    const leaf = "mock-thread-3";
    const at = Date.now() - 600_000;
    const link = (orchestrator: string, target: string): Link => ({
      id: `${orchestrator}/${target}`,
      orchestrator,
      target,
      orchestratorTitle: orchestrator,
      targetTitle: target,
      message: "go on",
      state: "interrupted",
      createdAt: at,
      startedAt: at,
    });
    const staged = await stagedRestart([link(top, middle), link(middle, leaf)], [top, middle, leaf]);
    linksResumed([`${top}/${middle}`, `${middle}/${leaf}`]);
    // The middle's resume turn ends: its link to the top settles, and the top hears nothing yet —
    // the leaf's own resume still runs, and the middle has not finished for its parent.
    linksFinished([`${top}/${middle}`], middle, "completed", undefined, [
      { kind: "assistant", id: "m", text: "I am back at it" },
    ]);
    threadTurnEnded(middle, false, "completed", undefined, [
      { kind: "assistant", id: "m", text: "I am back at it" },
    ]);
    await new Promise((r) => setTimeout(r, 400));
    expect(staged.linkOf(`${top}/${middle}`)?.state).toBe("done");
    expect(wakeTurns(top)).toEqual([]);
    // The leaf's resume ends too: the middle is told, its wake turn ends, and only then does the
    // top hear — with the middle's latest answer, once.
    linksFinished([`${middle}/${leaf}`], leaf, "completed", undefined, [
      { kind: "assistant", id: "l", text: "the leaf is done" },
    ]);
    threadTurnEnded(leaf, false, "completed", undefined, [
      { kind: "assistant", id: "l", text: "the leaf is done" },
    ]);
    wakeTick();
    await answerAsking([middle, top], () => wakeTurns(top).length > 0);
    const told = wakeTurns(top).join("\n");
    expect(told).toContain(`Delegate finished after a restart: ${middle}`);
    expect(told).toContain("resumed after a restart");
    // The answer the top reads is the middle's turn after the leaf's result, not "I am back at it".
    expect(told).not.toContain("I am back at it");
    expect(staged.linkOf(`${top}/${middle}`)?.notifiedAt).toBeDefined();
    expect(wakeTurns(top)).toHaveLength(1);
  }, 30_000);

  test("a wake the queue lost in a restart is delivered again, not dropped", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    await until(() => wakes(parent) === 1, 3000);
    expect(linkOf()?.notifiedAt).toBeUndefined();
    // A restart: the queue lives in this window's memory, so it is gone, and so is the row.
    updateThread(parent, (t) => ({ ...t, queue: [] }));
    // The book never heard that the result was delivered, so the wake is due again.
    wakeTick();
    await until(() => wakes(parent) === 1, 3000);
    expect(wakeText(parent)).toContain("the child's answer");
    expect(wakeLinks(parent)).toEqual([link.id]);
    // And it runs: the mark lands with the turn, once.
    await runQueuedWake(parent);
    expect(linkOf()?.notifiedAt).toBeDefined();
    wakeTick();
    expect(wakes(parent)).toBe(0);
  });

  test("an ordinary wait still answers with the child's result, and no wake repeats it", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-3",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    const answer = (await waited) as { status?: string; links?: { status?: string; answer?: string }[] };
    expect(answer.status).toBeUndefined();
    expect(answer.links?.[0]?.status).toBe("done");
    expect(answer.links?.[0]?.answer).toContain("the child's answer");
    expect(linkOf()?.notifiedAt).toBeDefined();
    await new Promise((r) => setTimeout(r, 500));
    expect(wakes(parent)).toBe(0);
  });

  test("a timeout is unchanged: it says the child still runs and keeps the link free", async () => {
    const { parent, link, linkOf } = await stagedWait();
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-4",
      args: { link_ids: [link.id], timeout_ms: 60 },
    });
    const answer = (await waited) as { status?: string; note?: string; waited_until?: string };
    expect(answer.status).toBe("timeout");
    expect(answer.note).toContain("Still running");
    expect(answer.waited_until).toBeDefined();
    expect(linkOf()?.notifiedAt).toBeUndefined();
    await new Promise((r) => setTimeout(r, 400));
    expect(wakes(parent)).toBe(0);
  });

  test("a wait whose own transcript row ends is released too, for a drop the engine never saw", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    // The harness's own row for the call, in flight: the app can watch it when the engine cannot tell.
    const row = "row-1";
    updateThread(parent, (t) => ({
      ...t,
      items: [
        ...t.items,
        {
          kind: "tool",
          id: row,
          title: "mcp__rustcode__wait_session",
          status: "in_progress",
          diffs: [],
        },
      ],
    }));
    const waited = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    // The harness timed the call out on its own side: the row fails, and nobody reads the wait.
    updateThread(parent, (t) => ({
      ...t,
      items: t.items.map((i) => (i.kind === "tool" && i.id === row ? { ...i, status: "failed" } : i)),
    }));
    const answer = (await waited) as { status?: string; links?: unknown[] };
    expect(answer.status).toBe("dropped");
    expect(answer.links).toEqual([]);
    expect(linkOf()?.notifiedAt).toBeUndefined();
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    await until(() => wakes(parent) === 1, 3000);
    expect(wakeText(parent)).toContain("the child's answer");
  });

  test("a second wait on the same link takes over the first, with one delivery", async () => {
    const { parent, link, target, linkOf } = await stagedWait();
    const first = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-5",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    const second = answerOrchestration({
      sessionId: parent,
      tool: "wait_session",
      callId: "call-6",
      args: { link_ids: [link.id], timeout_ms: 3_600_000 },
    });
    // The older wait stops at once, without an answer: the newer one is the one being read.
    const taken = (await first) as { status?: string; links?: unknown[] };
    expect(taken.status).toBe("taken_over");
    expect(taken.links).toEqual([]);
    expect(linkOf()?.notifiedAt).toBeUndefined();
    linkFinished(link.id, target, "completed", undefined, [
      { kind: "assistant", id: "m", text: "the child's answer" },
    ]);
    const answer = (await second) as { links?: { status?: string; answer?: string }[] };
    expect(answer.links?.[0]?.status).toBe("done");
    expect(answer.links?.[0]?.answer).toContain("the child's answer");
    await new Promise((r) => setTimeout(r, 500));
    expect(wakes(parent)).toBe(0);
    expect(linkOf()?.notifiedAt).toBeDefined();
  });

  test("review and done marks, and the engine's auto-done", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.status === true);
    const id = useApp.getState().sessions[0]!.id;
    await setThreadStatus(id, "review");
    expect(statusOf(useApp.getState(), id)).toBe("review");
    await setThreadStatus(id, "idle");
    expect(statusOf(useApp.getState(), id)).toBe("idle");
    expect(statusOf(useApp.getState(), "mock-thread-4")).toBe("review");
  });

  test("sending in a settled thread unsettles it without a toast", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.status === true);
    const id = useApp.getState().sessions[0]!.id;
    await setThreadStatus(id, "done");
    selectThread(id);
    await until(() => !!useApp.getState().threads[id]?.loaded);
    expect(statusOf(useApp.getState(), id)).toBe("done");
    await submit("yolo hello");
    // Writing to it puts it back on the active list, and that is all the send has to say.
    expect(useApp.getState().statusMarks[id]).toBeUndefined();
    expect(useApp.getState().sessions.find((s) => s.id === id)?.status).toBe("idle");
    expect(useApp.getState().toasts.map((t) => t.text)).not.toContain("Unsettled");
    await until(() => useApp.getState().sessions.find((s) => s.id === id)?.status === "idle");
    await refreshSessions();
    expect(useApp.getState().sessions.find((s) => s.id === id)?.status).toBe("idle");
  });

  test("a queued, steered or Send now message unsettles a settled thread quietly", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.status === true);
    setProjectCwd("/tmp/project");
    // Codex in the mock steers live, so a busy thread takes both a queue and a steer.
    switchNewThreadHarness("codex");
    const done = submit("change the greeting");
    await until(() => (selected()?.permissions.length ?? 0) > 0);
    const id = selected()!.id;
    const unsettleToasts = () =>
      useApp
        .getState()
        .toasts.filter((t) => t.text.startsWith("Unsettled"))
        .map((t) => t.text);
    // A thread on screen reports its live turn first, so the mark is what says it is unsettled.
    const settledAgain = async () => {
      await setThreadStatus(id, "done");
      expect(useApp.getState().statusMarks[id]).toBe("done");
    };
    const active = () => {
      const s = useApp.getState();
      return [s.statusMarks[id], s.sessions.find((x) => x.id === id)?.status];
    };

    // Queued behind the running turn (the send path that only waits).
    await settledAgain();
    expect(await submit("yolo queued")).toBe(true);
    expect(active()).toEqual([undefined, "idle"]);
    expect(unsettleToasts()).toEqual([]);
    expect(selected()!.queue.map((q) => q.text)).toEqual(["yolo queued"]);

    // Steered into the running turn (⌘⏎'s own route).
    await settledAgain();
    expect(await steerOrSubmit("steer this")).toBe(true);
    expect(active()).toEqual([undefined, "idle"]);
    expect(unsettleToasts()).toEqual([]);
    expect(selected()!.items.flatMap((i) => (i.kind === "user" && i.steered ? [i.text] : []))).toEqual([
      "steer this",
    ]);

    // The queued row's Send now, which steers it without going through either route.
    await settledAgain();
    expect(await sendQueuedNow(id, selected()!.queue[0]!.id)).toBe(true);
    expect(active()).toEqual([undefined, "idle"]);
    expect(unsettleToasts()).toEqual([]);
    expect(selected()!.queue).toEqual([]);

    // The explicit Unsettle action is the user asking, so it still says so.
    await settledAgain();
    unsettleThreads([id]);
    expect(active()).toEqual([undefined, "idle"]);
    expect(useApp.getState().toasts.at(-1)).toMatchObject({ text: "Unsettled", tone: "info" });

    await cancel();
    await done;
  });

  test("unsettling a mixed selection leaves the reviewed threads in review", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.status === true);
    const [a, b, c] = useApp.getState().sessions;
    await setThreadStatus(a!.id, "review");
    await setThreadStatus(b!.id, "done");
    await setThreadStatus(c!.id, "done");
    expect(statusOf(useApp.getState(), a!.id)).toBe("review");

    unsettleThreads([a!.id, b!.id, c!.id]);
    let s = useApp.getState();
    expect(statusOf(s, a!.id)).toBe("review");
    expect(statusOf(s, b!.id)).toBe("idle");
    expect(statusOf(s, c!.id)).toBe("idle");
    expect(s.toasts.at(-1)?.text).toBe("Unsettled 2 threads");
    expect(s.toasts.at(-1)?.tone).toBe("info");

    // The single-thread route moves a settled thread back to idle as well.
    await setThreadStatus(b!.id, "done");
    await setThreadStatus(b!.id, "idle");
    s = useApp.getState();
    expect(statusOf(s, b!.id)).toBe("idle");
  });

  test("a status changed outside the window shows on the next list", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().features.status === true);
    const id = useApp.getState().sessions[0]!.id;
    await setThreadStatus(id, "review");
    expect(statusOf(useApp.getState(), id)).toBe("review");

    // `rustcode threads settle <id>` in a terminal stores the same status.json without a turn, so
    // no update announces it: the app's mark must not outlive the first list that disagrees.
    await request(V2Method.sessionStatus, { session_id: id, status: "done" }, 10_000);
    await refreshSessions();
    expect(useApp.getState().statusMarks[id]).toBeUndefined();
    expect(statusOf(useApp.getState(), id)).toBe("done");

    // A mark for a thread the list does not carry (a starting thread, an older engine) is all
    // there is, so it stays until the list says something about it.
    useApp.setState((s) => ({ statusMarks: { ...s.statusMarks, "mock-thread-unlisted": "review" } }));
    await refreshSessions();
    expect(useApp.getState().statusMarks["mock-thread-unlisted"]).toBe("review");
  });

  test("a window focus re-lists threads at most once per five seconds", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length === 6);
    const at = Date.now();

    // Nothing is listed twice in the gap: a list per focus would read every thread each time.
    useApp.setState({ sessions: [] });
    await refreshSessionsOnFocus(at);
    expect(useApp.getState().sessions.length).toBe(6);
    useApp.setState({ sessions: [] });
    await refreshSessionsOnFocus(at + 4_000);
    expect(useApp.getState().sessions.length).toBe(0);
    await refreshSessionsOnFocus(at + 6_000);
    expect(useApp.getState().sessions.length).toBe(6);
  });

  test("the panel is remembered per thread and never opens on its own", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [a, b] = useApp.getState().sessions;

    // The view ⌘⌥B opens with when a thread has none is the thread's own overview.
    expect(DEFAULT_PANEL).toBe("details");

    // A thread never seen opens closed, and picking it again does not change that.
    selectThread(a!.id);
    expect(useApp.getState().panel).toBeNull();
    setPanel(null);
    expect(useApp.getState().panel).toBeNull();
    selectThread(a!.id);
    expect(useApp.getState().panel).toBeNull();

    // Changes opened on A is kept on A alone: B was never seen, so it opens closed.
    setPanel("changes");
    expect(useApp.getState().panel).toBe("changes");
    selectThread(b!.id);
    expect(useApp.getState().panel).toBeNull();
    selectThread(a!.id);
    expect(useApp.getState().panel).toBe("changes");

    // A close by hand is kept on that thread too: it survives leaving and returning.
    setPanel(null);
    expect(useApp.getState().panel).toBeNull();
    selectThread(b!.id);
    selectThread(a!.id);
    expect(useApp.getState().panel).toBeNull();

    // ⌘⌥B opens the thread's own view; closing it with the key is kept on that
    // thread, and the other thread keeps the view it had.
    setPanel("changes");
    selectThread(b!.id);
    expect(useApp.getState().panel).toBeNull();
    toggleRightPanel();
    expect(useApp.getState().panel).toBe(DEFAULT_PANEL);
    toggleRightPanel();
    expect(useApp.getState().panel).toBeNull();
    selectThread(a!.id);
    expect(useApp.getState().panel).toBe("changes");
    selectThread(b!.id);
    expect(useApp.getState().panel).toBeNull();

    // Each thread keeps its own view, and a view the user closed opens nothing
    // by itself when the thread comes back.
    showPanel("changes");
    selectThread(a!.id);
    showPanel("details");
    selectThread(b!.id);
    expect(useApp.getState().panel).toBe("changes");
    selectThread(a!.id);
    expect(useApp.getState().panel).toBe("details");

    // A stored view this project or thread cannot show opens nothing: once the
    // peek is over, the Thread tab has nothing to show on A.
    peekThread(b!.id);
    expect(useApp.getState().panel).toBe("peek");
    selectThread(b!.id);
    selectThread(a!.id);
    expect(useApp.getState().panel).toBeNull();

    // Opening the thread the Thread tab shows ends the peek, so the panel is
    // not left showing the thread that is now the one on screen, and B opens
    // on the view B had.
    showPanel("changes");
    peekThread(b!.id);
    expect(useApp.getState().panel).toBe("peek");
    selectThread(b!.id);
    expect(useApp.getState().peekId).toBeNull();
    expect(useApp.getState().panel).toBe("changes");

    // A new thread has nothing beside it to show in the Thread tab.
    peekThread(a!.id);
    startNewThread();
    expect(useApp.getState().peekId).toBeNull();
    expect(useApp.getState().panel).toBeNull();
  });

  test("Reduce transparency defaults off and is kept in storage", () => {
    expect(SETTINGS_DEFAULTS.reduceTransparency).toBe(false);
    const storage = testStorage();
    try {
      setReduceTransparency(true);
      expect(useApp.getState().reduceTransparency).toBe(true);
      expect(storage.data.get("rustcode.reduceTransparency")).toBe("1");
      setReduceTransparency(SETTINGS_DEFAULTS.reduceTransparency);
      expect(useApp.getState().reduceTransparency).toBe(false);
      expect(storage.data.get("rustcode.reduceTransparency")).toBe("0");
    } finally {
      storage.done();
    }
  });

  test("the per-thread panel is kept in storage, bounded to the newest entries", async () => {
    // The app keeps it through `store()`; a test has no window, so it gets one map.
    const data = new Map<string, string>();
    const scope = globalThis as unknown as Record<string, unknown>;
    scope.localStorage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
    } as unknown as Storage;
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
      const [a, b] = useApp.getState().sessions;

      selectThread(a!.id);
      setPanel("changes");
      selectThread(b!.id);
      // The close is kept as well, so a thread that was closed does not come back open.
      setPanel(null);
      const kept = JSON.parse(data.get("rustcode.panelByThread") ?? "{}");
      expect(kept[a!.id]).toBe("changes");
      expect(kept[b!.id]).toBeNull();
      // A storage it cannot write to changes nothing: the memory is still in the store.
      expect(parsePanels(kept)).toEqual(useApp.getState().panelByThread);

      // Junk from another build is dropped, and the oldest entries go past the bound.
      expect(parsePanels({ [a!.id]: "changes", junk: "nope", [b!.id]: null })).toEqual({
        [a!.id]: "changes",
        [b!.id]: null,
      });
      expect(parsePanels(null)).toEqual({});
      const many = Object.fromEntries(
        Array.from({ length: MAX_PANELS + 3 }, (_, i) => [`t${i}`, i % 2 ? "changes" : null]),
      );
      const pruned = parsePanels(many);
      expect(Object.keys(pruned)).toHaveLength(MAX_PANELS);
      expect(Object.keys(pruned)[0]).toBe("t3");
      expect(prunePanels(pruned)).toBe(pruned);
    } finally {
      delete scope.localStorage;
    }
  });

  test("trashing the thread in the Thread tab closes that view, and only that view", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().sessions.length > 2);
    const [a, b] = useApp.getState().sessions;

    peekThread(a!.id);
    expect(useApp.getState().panel).toBe("peek");
    expect(await trashThread(a!.id)).toBe(true);
    let s = useApp.getState();
    expect(s.peekId).toBeNull();
    expect(s.panel).toBeNull();

    // A pane that is not the Thread tab stays open when the peeked thread goes.
    peekThread(b!.id);
    showPanel("changes");
    expect(await trashThread(b!.id)).toBe(true);
    s = useApp.getState();
    expect(s.peekId).toBeNull();
    expect(s.panel).toBe("changes");
  });

  test("settling the thread on screen leaves it for a new thread, with an Undo", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 2);
    const [a, b, c] = useApp.getState().sessions;
    selectThread(a!.id);
    await until(() => useApp.getState().threads[a!.id]?.loaded === true);
    setPanel("changes");
    expect(useApp.getState().panel).toBe("changes");

    settleThread(a!.id);
    let s = useApp.getState();
    expect(s.selectedId).toBeNull();
    expect(s.panel).toBeNull();
    expect(statusOf(s, a!.id)).toBe("done");
    // The new thread starts in the folder of the one that was settled.
    expect(s.projectCwd).toBe(a!.cwd);
    // One Undo puts the status back, without dragging the screen back with it.
    const settled = s.toasts.at(-1)!;
    expect(settled.text).toContain("Settled");
    expect(settled.action?.label).toBe("Undo");
    settled.action!.run();
    expect(statusOf(useApp.getState(), a!.id)).toBe("idle");
    expect(useApp.getState().selectedId).toBeNull();

    // An agent or the engine settling the thread on screen does not move it.
    selectThread(b!.id);
    setPanel("changes");
    await setThreadStatus(b!.id, "done");
    s = useApp.getState();
    expect(s.selectedId).toBe(b!.id);
    expect(s.panel).toBe("changes");
    expect(statusOf(s, b!.id)).toBe("done");

    // Settling a thread in the background with the user's action changes
    // nothing on screen either.
    settleThread(c!.id);
    s = useApp.getState();
    expect(s.selectedId).toBe(b!.id);
    expect(s.panel).toBe("changes");

    // Opening a thread that is already settled does not send the user away.
    selectThread(a!.id);
    expect(useApp.getState().selectedId).toBe(a!.id);
    expect(useApp.getState().panel).toBe("changes");
  });

  test("settling on the board stays on the board, and its Undo restores every status", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [a, b] = useApp.getState().sessions;
    selectThread(a!.id);
    await until(() => useApp.getState().threads[a!.id]?.loaded === true);
    setView({ kind: "board" });
    expect(useApp.getState().view.kind).toBe("board");

    settleThreads([a!.id, b!.id]);
    let s = useApp.getState();
    expect(s.view.kind).toBe("board");
    expect(s.selectedId).toBe(a!.id);
    expect(statusOf(s, a!.id)).toBe("done");
    expect(statusOf(s, b!.id)).toBe("done");
    const settled = s.toasts.at(-1)!;
    expect(settled.text).toBe("Settled 2 threads");
    settled.action!.run();
    s = useApp.getState();
    expect(statusOf(s, a!.id)).not.toBe("done");
    expect(statusOf(s, b!.id)).not.toBe("done");
    expect(s.view.kind).toBe("board");
  });

  test("settling keeps drafts and the turn that is still running", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const a = useApp.getState().sessions[0]!;
    selectThread(a.id);
    await until(() => useApp.getState().threads[a.id]?.loaded === true);
    setDraft(a.id, "half-written reply");
    setDraft("new", "the next idea");
    updateThread(a.id, (t) => ({ ...t, running: true }));

    settleThread(a.id);
    const s = useApp.getState();
    expect(s.selectedId).toBeNull();
    expect(s.drafts[a.id]).toBe("half-written reply");
    expect(s.drafts.new).toBe("the next idea");
    expect(s.threads[a.id]?.running).toBe(true);
  });

  test("snoozing the thread on screen leaves it for a new thread in its folder", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [a, b] = useApp.getState().sessions;
    selectThread(a!.id);
    await until(() => useApp.getState().threads[a!.id]?.loaded === true);
    setPanel("changes");

    snoozeThread(a!.id, Date.now() + 3_600_000);
    let s = useApp.getState();
    expect(a!.id in s.snoozed).toBe(true);
    expect(s.selectedId).toBeNull();
    expect(s.panel).toBeNull();
    // The new thread starts in the folder of the one that was snoozed.
    expect(s.projectCwd).toBe(a!.cwd);

    // Waking it, on the thread the user has opened since, does not send them away.
    selectThread(b!.id);
    snoozeThread(a!.id, null);
    s = useApp.getState();
    expect(a!.id in s.snoozed).toBe(false);
    expect(s.selectedId).toBe(b!.id);
  });

  test("snoozing another thread, or one an agent snoozes, leaves the screen alone", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [a, b] = useApp.getState().sessions;
    selectThread(b!.id);
    await until(() => useApp.getState().threads[b!.id]?.loaded === true);
    setPanel("changes");

    snoozeThread(a!.id, Date.now() + 3_600_000);
    let s = useApp.getState();
    expect(a!.id in s.snoozed).toBe(true);
    expect(s.selectedId).toBe(b!.id);
    expect(s.panel).toBe("changes");

    // An agent's `update_session` writes the time and stays put, as an agent's
    // settle does.
    setSnoozed(b!.id, Date.now() + 3_600_000);
    s = useApp.getState();
    expect(b!.id in s.snoozed).toBe(true);
    expect(s.selectedId).toBe(b!.id);
    expect(s.panel).toBe("changes");

    // The board has no new-thread screen, so snoozing there stays on the board.
    selectThread(a!.id);
    setView({ kind: "board" });
    snoozeThread(a!.id, Date.now() + 3_600_000);
    s = useApp.getState();
    expect(s.view.kind).toBe("board");
    expect(s.selectedId).toBe(a!.id);
  });

  test("the new-thread screen has no panel, and neither does the thread it starts", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    selectThread(useApp.getState().sessions[0]!.id);
    startNewThread();
    expect(useApp.getState().panel).toBeNull();

    await submit("hello there");
    await until(() => !!selected());
    // A thread never seen opens closed: nothing opens the panel on its own.
    expect(useApp.getState().panel).toBeNull();
  });
});

describe("a thread that sleeps through its work", () => {
  /** A thread loaded in the background (not on screen), and how a test sees the toasts it causes. */
  const background = async () => {
    // `resetForTests` keeps the unread marks, as other suites rely on: this one starts without them.
    useApp.setState({ finished: {}, failed: {} });
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const [a, b] = useApp.getState().sessions;
    await openThread(a!.id);
    await until(() => useApp.getState().threads[a!.id]?.loaded === true);
    return { a: a!, b: b! };
  };
  const toastIds = () => new Set(useApp.getState().toasts.map((t) => t.id));
  const toastsSince = (seen: ReadonlySet<string>) => useApp.getState().toasts.filter((t) => !seen.has(t.id));
  /** The inbox's rows, as the sidebar and the view read them from this state. */
  const inboxOf = (s: AppState) =>
    deriveInbox({
      sessions: s.sessions,
      threads: s.threads,
      failed: s.failed,
      links: s.links.links,
      archived: s.archived,
      dismissed: s.inboxDismissed,
      snoozed: s.snoozed,
    });

  test("a turn that fails while the thread sleeps neither wakes it nor says so", async () => {
    const { a } = await background();
    const seen = toastIds();
    // The turn starts first, so the snooze is the user's wish about a thread already at work.
    const running = runPrompt(a.id, "fail this turn");
    snoozeThread(a.id, Date.now() + 3_600_000);
    await running;
    const s = useApp.getState();
    expect(a.id in s.snoozed).toBe(true);
    expect(toastsSince(seen)).toEqual([]);
    // The failure is kept: the row reads Failed when the snooze is over.
    expect(a.id in s.failed).toBe(true);
    // Nothing counts it while it sleeps: no Dock badge, no row in Needs you.
    expect(badgeCount(s)).toBe(0);
    expect(inboxOf(s).some((item) => item.threadId === a.id)).toBe(false);
    // Its time up: the row is back in the inbox, the badge counts it, and one thing is said.
    wakeSnoozed(Date.now() + 3_600_001);
    const woken = useApp.getState();
    expect(woken.snoozed).toEqual({});
    expect(badgeCount(woken)).toBe(1);
    expect(inboxOf(woken).some((item) => item.kind === "failed" && item.threadId === a.id)).toBe(true);
    expect(snoozeWakeEvent(a.id)).toMatchObject({ threadId: a.id, kind: "failed" });
  });

  test("a request that waits on a sleeping thread waits there", async () => {
    const { a } = await background();
    const seen = toastIds();
    const running = runPrompt(a.id, "ask me a question");
    snoozeThread(a.id, Date.now() + 3_600_000);
    await until(() => (useApp.getState().threads[a.id]?.permissions.length ?? 0) > 0);
    const s = useApp.getState();
    expect(a.id in s.snoozed).toBe(true);
    expect(toastsSince(seen)).toEqual([]);
    // Nothing answers it for the user, and the turn stays held by the question.
    const question = s.threads[a.id]!.permissions[0]!;
    expect(question.question?.text).toBe("Which test runner should I use?");
    expect(s.threads[a.id]?.running).toBe(true);
    // The request is not a row in Needs you, nor a thread the Dock counts, until the snooze ends.
    expect(inboxOf(s).some((item) => item.threadId === a.id)).toBe(false);
    expect(badgeCount(s)).toBe(0);
    // Stopping it answers the question as cancelled, which ends the turn this test left running.
    await cancel(a.id);
    await running;
  });

  test("a turn the app starts on a sleeping thread does not bring it back", async () => {
    const { a } = await background();
    const seen = toastIds();
    setSnoozed(a.id, Date.now() + 3_600_000);
    // A turn the app starts after a restart is not the user writing in the thread.
    await runPrompt(a.id, "fail this turn", undefined, { automatic: true });
    const s = useApp.getState();
    expect(a.id in s.snoozed).toBe(true);
    expect(toastsSince(seen)).toEqual([]);
  });

  test("a message the user writes into a working sleeper brings it back at once", async () => {
    const { a } = await background();
    selectThread(a.id);
    // Long enough that the turn is still running when the message arrives: it waits in the queue.
    const running = runPrompt(a.id, "keep working");
    await until(() => useApp.getState().threads[a.id]?.running === true);
    setSnoozed(a.id, Date.now() + 3_600_000);
    expect(await submit("one more thing")).toBe(true);
    const s = useApp.getState();
    // The write is the user asking for it, so it does not wait for the queued row's turn.
    expect(a.id in s.snoozed).toBe(false);
    expect(s.threads[a.id]?.queue.length).toBe(1);
    await cancel(a.id);
    await running;
    // The row this test left behind is not the next one's to start from storage.
    updateThread(a.id, (t) => ({ ...t, queue: [] }));
  });

  test("the next message of the queue is not a new send, and lets it sleep", async () => {
    const { a } = await background();
    const seen = toastIds();
    setSnoozed(a.id, Date.now() + 3_600_000);
    // Exactly what starting the row behind a finished turn passes: typed earlier, not now.
    await runPrompt(a.id, "fail this turn", undefined, { queued: true });
    expect(a.id in useApp.getState().snoozed).toBe(true);
    expect(toastsSince(seen)).toEqual([]);
  });

  test("a sleeping thread with nothing to say comes back quietly", async () => {
    const { a } = await background();
    const seen = toastIds();
    setSnoozed(a.id, Date.now() - 1);
    wakeSnoozed();
    const s = useApp.getState();
    expect(a.id in s.snoozed).toBe(false);
    // Nothing happened while it slept: no unread mark is invented for it.
    expect(a.id in s.finished).toBe(false);
    expect(a.id in s.failed).toBe(false);
    expect(toastsSince(seen)).toEqual([]);
  });

  test("a sleeping thread that finished comes back done, and waking twice says it once", async () => {
    const { a } = await background();
    const seen = toastIds();
    setSnoozed(a.id, Date.now() - 1);
    markFinished(a.id);
    wakeSnoozed();
    let s = useApp.getState();
    expect(a.id in s.snoozed).toBe(false);
    expect(a.id in s.finished).toBe(true);
    expect(toastsSince(seen)).toEqual([]);
    // One thing to say now it is back: the finish it slept through.
    expect(snoozeWakeEvent(a.id)).toMatchObject({ threadId: a.id, kind: "finished" });
    // Its time has come and gone: a second pass says nothing again.
    wakeSnoozed();
    s = useApp.getState();
    expect(a.id in s.snoozed).toBe(false);
  });

  test("opening a sleeping thread is the user's call, and only opening it", async () => {
    const { a } = await background();
    setSnoozed(a.id, Date.now() + 3_600_000);
    selectThread(a.id);
    const s = useApp.getState();
    expect(s.selectedId).toBe(a.id);
    expect(s.threads[a.id]?.loaded).toBe(true);
    expect(a.id in s.snoozed).toBe(true);
  });
});

describe("list_harnesses on a harness with hundreds of models", () => {
  type Listed = {
    id: string;
    efforts?: string[];
    omitted_models?: number;
    filter?: string;
    models?: { id: string; name: string; efforts?: string[] }[];
  };

  /** `count` models named `vendor/m00`, `Model 00`, … as an 800-model harness has. */
  const many = (count: number, efforts = ["low", "medium"]): HarnessModel[] =>
    Array.from({ length: count }, (_, i) => {
      const n = String(i).padStart(2, "0");
      return { id: `vendor/m${n}`, name: `Model ${n}`, efforts, default_effort: efforts[0] };
    });

  /** The mock snapshot with `codex` listing `models`, its new-thread choice `configured`. */
  const withModels = (models: HarnessModel[], configured: string | null = null) => {
    const snapshot = useApp.getState().harnesses as HarnessSnapshot;
    useApp.setState({
      harnesses: {
        ...snapshot,
        // No harness is left empty: a listing would otherwise be asked for, and
        // its answer replaces the snapshot under this test.
        harnesses: snapshot.harnesses.map((h) =>
          h.id === "codex"
            ? { ...h, models, last: configured ? { model: configured, effort: null } : null }
            : h.models.length
              ? h
              : { ...h, models: [{ id: "only", name: "Only", efforts: [] }] },
        ),
      },
    });
  };

  /** This project's threads: `caller` and one thread per model it lists as recently used. */
  const usedModels = (caller: string, used: { model: string; at: number }[]) => {
    const first = useApp.getState().sessions.find((t) => t.id === caller)!;
    useApp.setState({
      sessions: [
        first,
        ...used.map((u) => ({
          ...first,
          id: `used-${u.model}`,
          harness: "codex",
          model: u.model,
          updatedAt: u.at,
        })),
      ],
    });
  };

  /** The `codex` entry of one `list_harnesses` answer, by the args it was asked with. */
  const listed = async (caller: string, args: JsonObject = {}) => {
    const answer = await answerOrchestration({ sessionId: caller, tool: "list_harnesses", args });
    return answer.harnesses as Listed[];
  };

  const ready = async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    return useApp.getState().sessions[0]!.id;
  };

  test("leads with the model a new thread starts on and the ones in use, and caps the rest", async () => {
    const caller = await ready();
    withModels(many(20), "vendor/m09");
    // m03 is newer than m05, so it comes first among the ones in use.
    usedModels(caller, [
      { model: "vendor/m03", at: 3_000 },
      { model: "vendor/m05", at: 2_000 },
    ]);
    const codex = (await listed(caller)).find((h) => h.id === "codex")!;
    expect(codex.models!.map((m) => m.id).slice(0, 5)).toEqual([
      "vendor/m09",
      "vendor/m03",
      "vendor/m05",
      "vendor/m00",
      "vendor/m01",
    ]);
    // Twelve is what an agent's context pays for; the rest is a count and a way in.
    expect(codex.models).toHaveLength(12);
    expect(codex.omitted_models).toBe(8);
    expect(codex.filter).toBe("pass model to search the omitted ones");
  });

  test("hoists efforts every listed model shares onto the harness, once", async () => {
    const caller = await ready();
    withModels(many(5));
    const codex = (await listed(caller)).find((h) => h.id === "codex")!;
    expect(codex.efforts).toEqual(["low", "medium"]);
    expect(codex.models!.every((m) => m.efforts === undefined)).toBe(true);
    // A model list that differs keeps its efforts per model, and says nothing on the harness.
    withModels([
      { id: "vendor/a", name: "A", efforts: ["low"] },
      { id: "vendor/b", name: "B", efforts: ["high"] },
    ]);
    const mixed = (await listed(caller)).find((h) => h.id === "codex")!;
    expect(mixed.efforts).toBeUndefined();
    expect(mixed.models).toEqual([
      { id: "vendor/a", name: "A", efforts: ["low"] },
      { id: "vendor/b", name: "B", efforts: ["high"] },
    ]);
  });

  test("a model filter matches an id or a name, ignores case, and lists every match", async () => {
    const caller = await ready();
    withModels(many(120));
    const byId = (await listed(caller, { harness: "codex", model: "m11" })).find((h) => h.id === "codex")!;
    expect(byId.models!.map((m) => m.id)).toEqual([
      "vendor/m11",
      "vendor/m110",
      "vendor/m111",
      "vendor/m112",
      "vendor/m113",
      "vendor/m114",
      "vendor/m115",
      "vendor/m116",
      "vendor/m117",
      "vendor/m118",
      "vendor/m119",
    ]);
    // Case-insensitive, and a name match counts too.
    const byName = (await listed(caller, { harness: "codex", model: "MODEL 07" })).find(
      (h) => h.id === "codex",
    )!;
    expect(byName.models!.map((m) => m.id)).toEqual(["vendor/m07"]);
    // Every match, still bounded, and the rest is a count a second pass can reach.
    const broad = (await listed(caller, { harness: "codex", model: "m" })).find((h) => h.id === "codex")!;
    expect(broad.models).toHaveLength(50);
    expect(broad.omitted_models).toBe(70);
    expect(broad.filter).toBe("pass model to search the omitted ones");
    // A filter that matches nothing says so, rather than answering an empty list.
    const none = (await listed(caller, { harness: "codex", model: "nope" })).find((h) => h.id === "codex")!;
    expect(none.models).toEqual([]);
    expect(none.omitted_models).toBeUndefined();
    expect(none.filter).toBe("no model matches; pass a shorter substring");
  });

  test("a harness filter answers that harness alone, and a wrong one names the ones there are", async () => {
    const caller = await ready();
    withModels(many(3));
    expect((await listed(caller, { harness: "codex" })).map((h) => h.id)).toEqual(["codex"]);
    // A display name is as good as an id.
    expect((await listed(caller, { harness: "Codex" })).map((h) => h.id)).toEqual(["codex"]);
    const refused = await answerOrchestration({
      sessionId: caller,
      tool: "list_harnesses",
      args: { harness: "nope" },
    });
    const said = (refused.error as { message?: string } | undefined)?.message ?? "";
    expect(said).toContain("no harness “nope”");
    expect(said).toContain("codex");
  });

  test("a display name two harnesses share is refused, and an id decides", async () => {
    const caller = await ready();
    withModels(many(3));
    // Two harnesses the user wrote by hand, with one display name between them.
    const snapshot = useApp.getState().harnesses as HarnessSnapshot;
    const codex = snapshot.harnesses.find((h) => h.id === "codex")!;
    useApp.setState({
      harnesses: {
        ...snapshot,
        harnesses: [
          ...snapshot.harnesses,
          { ...codex, id: "twin-a", name: "Twin", known: false },
          { ...codex, id: "twin-b", name: "Twin", known: false },
        ],
      },
    });
    const refused = await answerOrchestration({
      sessionId: caller,
      tool: "list_harnesses",
      args: { harness: "Twin" },
    });
    const said = (refused.error as { message?: string } | undefined)?.message ?? "";
    expect(said).toContain("names 2 harnesses");
    expect(said).toContain("twin-a, twin-b");
    // The id is exact, so it answers that one alone.
    expect((await listed(caller, { harness: "twin-b" })).map((h) => h.id)).toEqual(["twin-b"]);
  });

  test("never leads with a model the harness does not list", async () => {
    const caller = await ready();
    // Remembered from an engine that listed it, but this listing no longer has it.
    withModels(many(4), "vendor/gone");
    const codex = (await listed(caller)).find((h) => h.id === "codex")!;
    expect(codex.models!.map((m) => m.id)).toEqual(["vendor/m00", "vendor/m01", "vendor/m02", "vendor/m03"]);
  });
});

describe("the sidebar's arrangement", () => {
  const stored = (storage: { data: Map<string, string> }) =>
    JSON.parse(storage.data.get("rustcode.sidebarOrder") ?? "null");

  test("trashing a thread drops it from every section, in memory and in storage", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
      const [a, b] = useApp.getState().sessions;
      setSectionOrder("pinned", [a!.id, b!.id]);
      setSectionOrder("threads", [b!.id, a!.id]);
      expect(stored(storage)).toEqual({ pinned: [a!.id, b!.id], threads: [b!.id, a!.id] });

      expect(await trashThread(a!.id)).toBe(true);
      const kept = { pinned: [b!.id], threads: [b!.id] };
      expect(useApp.getState().sidebarOrder).toEqual(kept);
      expect(stored(storage)).toEqual(kept);
    } finally {
      storage.done();
    }
  });

  test("the engine's list forgets the threads it no longer carries, and keeps the rest", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
      const [a, b] = useApp.getState().sessions;
      // One thread the engine lists, one it does not (trashed from another app, or before this
      // field existed): the listed one keeps its place and the other stops taking one up.
      setSectionOrder("threads", [a!.id, "mock-thread-gone"]);
      setSectionOrder("pinned", ["mock-thread-gone", b!.id]);

      await refreshSessions();
      const kept = { threads: [a!.id], pinned: [b!.id] };
      expect(useApp.getState().sidebarOrder).toEqual(kept);
      expect(stored(storage)).toEqual(kept);

      // Nothing to forget: the stored value is left exactly as it was.
      const written = storage.data.get("rustcode.sidebarOrder");
      await refreshSessions();
      expect(storage.data.get("rustcode.sidebarOrder")).toBe(written);
    } finally {
      storage.done();
    }
  });
});

describe("model picker", () => {
  test("a new thread on an external harness lists its models once asked", async () => {
    const { pickerModels } = await import("../ui/ModelPicker");
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    const codex = () => useApp.getState().harnesses!.harnesses.find((h) => h.id === "codex")!;
    const choice = { harness: "codex", model: null, effort: null };
    // `_rustcode/harnesses` leaves an external harness's models empty until asked.
    expect(pickerModels(undefined, codex(), choice, []).models).toEqual([]);
    await ensureHarnessModels("codex");
    const picked = pickerModels(undefined, codex(), choice, []);
    expect(picked.models.length).toBeGreaterThan(0);
    expect(picked.model).toBe(picked.models[0]!.value);
    expect(picked.efforts.length).toBeGreaterThan(1);
  });

  test("an existing thread offers its config options", async () => {
    const { pickerModels } = await import("../ui/ModelPicker");
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    selectThread(useApp.getState().sessions[0]!.id);
    await until(() => selected()?.loaded === true);
    const picked = pickerModels(
      selected(),
      undefined,
      { harness: "rustcode", model: null, effort: null },
      [],
    );
    expect(picked.models.length).toBeGreaterThan(1);
    expect(picked.model).toBeTruthy();
    expect(picked.efforts.length).toBeGreaterThan(1);
  });
});

describe("model memory per harness (#2797, #2798)", () => {
  test("switching harness in the new-thread picker restores each one's last model and effort", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    expect(useApp.getState().harnesses?.remembers).toBe(true);
    await ensureHarnessModels("codex");
    pickNewThreadChoice({ harness: "codex", model: "gpt-6-astra", effort: "high" });
    switchNewThreadHarness("rustcode");
    expect(useApp.getState().newThread?.harness).toBe("rustcode");
    switchNewThreadHarness("codex");
    expect(useApp.getState().newThread).toEqual({ harness: "codex", model: "gpt-6-astra", effort: "high" });
    // The engine kept it: a fresh snapshot (as after a restart) carries it.
    const snapshot = await refreshHarnesses();
    expect(snapshot?.harnesses.find((h) => h.id === "codex")?.last).toEqual({
      model: "gpt-6-astra",
      effort: "high",
    });
    // A new thread on it starts there.
    setProjectCwd("/tmp/project");
    const done = submit("yolo hello");
    await until(() => selected() !== undefined);
    expect(selected()!.harness?.id).toBe("codex");
    expect(selected()!.configOptions.find((o) => o.id === "model")?.current).toBe("gpt-6-astra");
    await done;
  });

  test("a restored external thread asks for its harness's models and offers them", async () => {
    const { pickerModels } = await import("../ui/ModelPicker");
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    selectThread("mock-thread-2");
    await until(() => selected()?.loaded === true);
    expect(selected()!.harness?.id).toBe("codex");
    expect(selected()!.configOptions.find((o) => o.id === "model")?.choices).toHaveLength(1);
    const codex = () => useApp.getState().harnesses!.harnesses.find((h) => h.id === "codex")!;
    await until(() => codex().models.length > 0);
    const picked = pickerModels(selected(), codex(), { harness: "codex", model: null, effort: null }, []);
    expect(picked.models.map((m) => m.value)).toEqual(["gpt-6-luna", "gpt-6-astra"]);
    expect(picked.model).toBe("gpt-6-luna");
    expect(picked.efforts).toEqual(["low", "medium", "high"]);
    expect(picked.effort).toBe("medium");
  });

  test("a failed model listing is asked again, not cached", async () => {
    const transport = new MockTransport({ delayMs: 0 });
    let asked = 0;
    const send = transport.send.bind(transport);
    transport.send = (line: string) => {
      if (line.includes("_rustcode/harnesses/models")) asked++;
      return send(line);
    };
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    // Claude Code needs a sign-in in the mock: the listing fails.
    await ensureHarnessModels("claude-code");
    await ensureHarnessModels("claude-code");
    expect(asked).toBe(2);
  });

  test("the new-thread slider starts on the model's default effort", async () => {
    const { pickerModels } = await import("../ui/ModelPicker");
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    const rustcode = useApp.getState().harnesses!.harnesses.find((h) => h.id === "rustcode")!;
    const picked = pickerModels(undefined, rustcode, { harness: "rustcode", model: null, effort: null }, []);
    expect(picked.efforts.length).toBeGreaterThan(1);
    expect(picked.effort).toBe("medium");
  });
});

describe("effort memory per model", () => {
  test("each model keeps its own level, in one record a harness's logins share", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
      await ensureHarnessModels("codex");
      pickNewThreadChoice({ harness: "codex", model: "gpt-6-luna", effort: "low" });
      pickNewThreadChoice({ harness: "codex", model: "gpt-6-astra", effort: "high" });
      expect(effortMemory()).toEqual({ codex: { "gpt-6-luna": "low", "gpt-6-astra": "high" } });
      // Each model answers with its own level, never the last one picked.
      expect(rememberedEffort(effortMemory(), "codex", "gpt-6-luna")).toBe("low");
      expect(rememberedEffort(effortMemory(), "codex", "gpt-6-astra")).toBe("high");
      // Another login of the same harness reads the same memory, and does not reset the choice.
      await addHarnessAccount("codex", "work", "Work");
      pickNewThreadAccountOn("codex", "work");
      expect(newThreadChoice(useApp.getState())).toEqual({
        harness: "codex",
        model: "gpt-6-astra",
        effort: "high",
      });
      expect(effortMemory()).toEqual({ codex: { "gpt-6-luna": "low", "gpt-6-astra": "high" } });
      // One entry per harness, keyed only by the model: no login in the stored shape.
      const kept = JSON.parse(storage.data.get("rustcode.harnessChoices") ?? "{}");
      expect(kept).toEqual({
        codex: { model: null, effort: null, efforts: { "gpt-6-luna": "low", "gpt-6-astra": "high" } },
      });
    } finally {
      storage.done();
    }
  });

  test("an old stored choice keeps its model and effort, then grows the per-model memory", async () => {
    // The shape before per-model memory: a model and one effort, no `efforts` map.
    const storage = testStorage({
      "rustcode.harnessChoices": JSON.stringify({ codex: { model: "gpt-6-astra", effort: "high" } }),
    });
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
      expect(localChoices()).toEqual({ codex: { model: "gpt-6-astra", effort: "high" } });
      await ensureHarnessModels("codex");
      pickNewThreadChoice({ harness: "codex", model: "gpt-6-luna", effort: "low" });
      const kept = JSON.parse(storage.data.get("rustcode.harnessChoices") ?? "{}");
      expect(kept.codex).toEqual({
        model: "gpt-6-astra",
        effort: "high",
        efforts: { "gpt-6-luna": "low" },
      });
    } finally {
      storage.done();
    }
  });

  test("junk in storage is dropped instead of crashing the picker", async () => {
    const storage = testStorage({
      "rustcode.harnessChoices": JSON.stringify({
        codex: { model: 42, effort: true, efforts: { "gpt-6-luna": 7, "gpt-6-astra": "high" } },
        claude: "not an object",
      }),
    });
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
      expect(localChoices()).toEqual({
        codex: { model: null, effort: null, efforts: { "gpt-6-astra": "high" } },
      });
    } finally {
      storage.done();
    }
  });

  test("the new-thread picker keeps its choice across a visit to another thread", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    await ensureHarnessModels("codex");
    pickNewThreadChoice({ harness: "codex", model: "gpt-6-luna", effort: "high" });
    pickNewThreadAccountOn("codex", "paral");
    // Open an existing thread, then come back to the new-thread screen.
    selectThread(useApp.getState().sessions[0]!.id);
    await until(() => selected()?.loaded === true);
    startNewThread();
    let s = useApp.getState();
    expect(s.selectedId).toBeNull();
    expect(s.newThread).toEqual({ harness: "codex", model: "gpt-6-luna", effort: "high" });
    expect(s.newThreadAccount).toBe("paral");
    // Starting a thread from it keeps the draft too, for the next new thread.
    setProjectCwd("/tmp/project");
    expect(await submit("hello there")).toBe(true);
    expect(useApp.getState().selectedId).not.toBeNull();
    startNewThread();
    s = useApp.getState();
    expect(s.newThread).toEqual({ harness: "codex", model: "gpt-6-luna", effort: "high" });
    expect(s.newThreadAccount).toBe("paral");
  });
});

describe("the choice a new thread starts on", () => {
  test("a kept draft on a harness that cannot run falls back to the first-run default", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    const snapshot = useApp.getState().harnesses!;
    // Gemini CLI is not installed in the mock; the draft the screen kept names it anyway.
    setNewThreadChoice({ harness: "gemini", model: null, effort: null });
    const choice = newThreadChoice(useApp.getState());
    expect(choice).toEqual(preferredDefault(snapshot));
    // Not Gemini, and not `restoreChoice`'s hardcoded built-in fallback either.
    expect(choice?.harness).toBe("claude-code");
  });

  test("a kept draft on a harness that is gone falls back the same way", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    setNewThreadChoice({ harness: "ghost-harness", model: "m", effort: "high" });
    expect(newThreadChoice(useApp.getState())).toEqual(preferredDefault(useApp.getState().harnesses!));
  });

  test("a draft on a harness that can run is normalized to the model and level it lists", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    await ensureHarnessModels("codex");
    // A model and a level the harness no longer lists: the picker's own normalization, on read.
    setNewThreadChoice({ harness: "codex", model: "gpt-6-gone", effort: "turbo" });
    expect(newThreadChoice(useApp.getState())).toEqual({
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
    });
    // A draft that still fits is left exactly as it was picked.
    setNewThreadChoice({ harness: "codex", model: "gpt-6-astra", effort: "high" });
    expect(newThreadChoice(useApp.getState())).toEqual({
      harness: "codex",
      model: "gpt-6-astra",
      effort: "high",
    });
  });

  test("a new Settings default drops the draft, so the screen shows the default", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && !!useApp.getState().harnesses);
    await ensureHarnessModels("codex");
    setNewThreadChoice({ harness: "rustcode", model: "chatgpt/gpt-6-luna", effort: "low" });
    await setDefaultHarness({ harness: "codex", model: "gpt-6-astra", effort: "high" });
    expect(useApp.getState().newThread).toBeNull();
    expect(newThreadChoice(useApp.getState())).toEqual({
      harness: "codex",
      model: "gpt-6-astra",
      effort: "high",
    });
  });

  test("an agent's configure_session writes no effort memory, a person's pick does", async () => {
    const storage = testStorage();
    try {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
      await ensureHarnessModels("codex");
      // The user's own pick in the new-thread screen.
      pickNewThreadChoice({ harness: "codex", model: "gpt-6-luna", effort: "low" });
      expect(effortMemory()).toEqual({ codex: { "gpt-6-luna": "low" } });
      // An agent reconfigures an open Codex thread's effort (the mock's second thread).
      const id = "mock-thread-2";
      selectThread(id);
      await until(() => selected()?.loaded === true && !!selected()?.configOptions.length);
      const answer = await answerOrchestration({
        sessionId: id,
        tool: "configure_session",
        args: { effort: "high" },
      });
      expect(answer.error).toBeUndefined();
      expect(selected()!.configOptions.find((o) => o.id === "effort")?.current).toBe("high");
      // The agent's change is the thread's, never the memory the picker reads back.
      expect(effortMemory()).toEqual({ codex: { "gpt-6-luna": "low" } });
      // A level the user picks in the thread is remembered for that model, as before.
      await setConfigOption(id, "effort", "medium");
      expect(rememberedEffort(effortMemory(), "codex", "gpt-6-luna")).toBe("medium");
    } finally {
      storage.done();
    }
  });
});

describe("toasts", () => {
  test("the newest three show, a repeat refreshes one, an id replaces in place", async () => {
    const { toast, dismissToast, clearToasts } = await import("./store");
    clearToasts();
    toast("Copied", "success");
    const shown = useApp.getState().toasts[0]!;
    toast("Copied", "success");
    expect(useApp.getState().toasts.length).toBe(1);
    expect(useApp.getState().toasts[0]!.id).toBe(shown.id);
    expect(useApp.getState().toasts[0]!.persist).toBe(false);
    expect(useApp.getState().toasts[0]!.expiresAt).toBeGreaterThan(Date.now());
    toast("Could not save", "error");
    expect(useApp.getState().toasts[0]!.persist).toBe(false);
    // A burst keeps the newest three, newest first.
    for (let i = 0; i < 50; i++) toast(`Failed ${i}`, "error");
    expect(useApp.getState().toasts.length).toBe(3);
    expect(useApp.getState().toasts.map((t) => t.text)).toEqual(["Failed 49", "Failed 48", "Failed 47"]);
    toast("Starting the server…", "info", { id: "engine", persist: true, progress: 0.2 });
    toast("The server stopped.", "error", { id: "engine", action: { label: "Restart", run: () => {} } });
    const engine = useApp.getState().toasts.filter((t) => t.id === "engine");
    expect(engine.length).toBe(1);
    expect(engine[0]!.text).toBe("The server stopped.");
    expect(engine[0]!.action?.label).toBe("Restart");
    // Replacing a toast that is fading out keeps the new one.
    dismissToast("engine");
    toast("The engine restarted", "success", { id: "engine" });
    await new Promise((r) => setTimeout(r, 160));
    expect(useApp.getState().toasts.find((t) => t.id === "engine")?.text).toBe("The engine restarted");
  });

  test("pause stops every clock and resume keeps what was left", async () => {
    const { toast, setToastsPaused, clearToasts } = await import("./store");
    clearToasts();
    toast("A notice", "info");
    const before = useApp.getState().toasts[0]!.expiresAt!;
    setToastsPaused(true);
    expect(useApp.getState().toasts[0]!.expiresAt).toBeNull();
    expect(useApp.getState().toasts[0]!.remainingMs).toBeGreaterThan(0);
    expect(useApp.getState().toasts[0]!.remainingMs).toBeLessThanOrEqual(before - Date.now() + 50);
    setToastsPaused(false);
    expect(useApp.getState().toasts[0]!.expiresAt).not.toBeNull();
    clearToasts();
    setToastsPaused(false);
  });
});

describe("finished threads", () => {
  test("a turn that completes elsewhere is flagged until the thread is seen", async () => {
    const { markFinished, clearFinished } = await import("./store");
    useApp.setState({ selectedId: "other", finished: {} });
    markFinished("t1", 1000);
    expect(useApp.getState().finished).toEqual({ t1: 1000 });
    clearFinished("t1");
    expect(useApp.getState().finished).toEqual({});
  });

  test("a failed turn says Failed until seen, and Mark Unread brings Done back", async () => {
    const { markFinished, clearFinished, markUnread } = await import("./store");
    useApp.setState({ selectedId: "other", finished: {}, failed: {} });
    markFinished("t1", 1000, true);
    expect(useApp.getState().failed).toEqual({ t1: 1000 });
    clearFinished("t1");
    expect(useApp.getState().failed).toEqual({});
    markUnread("t1");
    expect(Object.keys(useApp.getState().finished)).toEqual(["t1"]);
  });
});

test("a mode chosen in one thread leaves the default and the other threads alone", async () => {
  resetForTests();
  useApp.setState({
    sessions: [
      { id: "a", title: "A", cwd: "/tmp" },
      { id: "b", title: "B", cwd: "/tmp" },
    ] as never,
  });
  setPermissionMode("b", "full");
  let s = useApp.getState();
  expect(permissionModeOf(s, "b")).toBe("full");
  expect(permissionModeOf(s, "a")).toBe("ask");
  // The default is the Settings row, not the last pill: a new thread still starts at Ask.
  expect(s.defaultPermissionMode).toBe("ask");
  expect(permissionModeOf(s, "new-thread")).toBe("ask");
  // The pill on a thread not started yet is for that next thread only.
  setPermissionMode(undefined, "edits");
  s = useApp.getState();
  expect(permissionModeOf(s, undefined)).toBe("edits");
  expect(s.defaultPermissionMode).toBe("ask");
  resetForTests();
});

test("changing the default access leaves the listed threads as they were", () => {
  resetForTests();
  useApp.setState({
    sessions: [
      { id: "a", title: "A", cwd: "/tmp" },
      { id: "b", title: "B", cwd: "/tmp" },
    ] as never,
  });
  setPermissionMode("b", "full");
  setDefaultPermissionMode("edits");
  const s = useApp.getState();
  expect(s.defaultPermissionMode).toBe("edits");
  expect(permissionModeOf(s, "b")).toBe("full");
  expect(permissionModeOf(s, "a")).toBe("ask");
  expect(permissionModeOf(s, "a-new-one")).toBe("edits");
  resetForTests();
});

describe("what a delegated thread may do without asking", () => {
  const ready = async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    return useApp.getState().sessions[0]!.id;
  };
  /** A call whose card, if the caller's own access does not already allow it, is allowed once. */
  const call = async (params: JsonObject) => {
    const answering = answerOrchestration(params);
    let settled = false;
    void answering.finally(() => {
      settled = true;
    });
    while (!settled) {
      if (useApp.getState().grant) resolveGrant("once");
      await new Promise((r) => setTimeout(r, 2));
    }
    return answering;
  };
  const start = (caller: string, args: JsonObject = {}) =>
    call({ sessionId: caller, tool: "start_session", args: { prompt: "do it", title: "Child", ...args } });

  test("an Ask parent's child runs at Ask, though Default access is Full", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "ask");
    const answer = await start(parent);
    const child = String(answer.session_id);
    // Reported, so the agent is not told it started a thread it may not have.
    expect(answer.access).toBe("ask");
    expect(permissionModeOf(useApp.getState(), child)).toBe("ask");
  });

  test("an Auto-edit parent's child stops at Auto-edit, and Full hands out Full", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "edits");
    expect((await start(parent)).access).toBe("edits");
    setPermissionMode(parent, "full");
    expect((await start(parent)).access).toBe("full");
  });

  test("a fan-out gives every child the same capped access", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "edits");
    const answer = await call({
      sessionId: parent,
      tool: "fan_out",
      args: { prompt: "do it", harnesses: [{ harness: "rustcode" }, { harness: "codex" }] },
    });
    const links = answer.links as unknown as { session_id: string; access: string }[];
    expect(links).toHaveLength(2);
    for (const link of links) {
      expect(link.access).toBe("edits");
      expect(permissionModeOf(useApp.getState(), link.session_id)).toBe("edits");
    }
  });

  test("a grandchild is capped by its own parent, not by Default access", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "ask");
    const child = String((await start(parent)).session_id);
    const grandchild = await start(child);
    expect(grandchild.access).toBe("ask");
    expect(permissionModeOf(useApp.getState(), String(grandchild.session_id))).toBe("ask");
  });

  test("a person can still raise a child by hand, and its own children stay capped", async () => {
    const parent = await ready();
    setDefaultPermissionMode("ask");
    setPermissionMode(parent, "ask");
    const child = String((await start(parent)).session_id);
    expect(permissionModeOf(useApp.getState(), child)).toBe("ask");
    // The shield under the child's own composer is the user's choice, which no cap overrides.
    setPermissionMode(child, "full");
    expect(permissionModeOf(useApp.getState(), child)).toBe("full");
    // What an agent starts is still only what the app hands out: Full parent, Ask default.
    expect((await start(child)).access).toBe("ask");
  });

  test("lowering a parent later leaves the children it already started as they were", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "full");
    const child = String((await start(parent)).session_id);
    expect(permissionModeOf(useApp.getState(), child)).toBe("full");
    setPermissionMode(parent, "ask");
    expect(permissionModeOf(useApp.getState(), child)).toBe("full");
  });

  test("configure_session reports a child's access and never changes it", async () => {
    const parent = await ready();
    setDefaultPermissionMode("full");
    setPermissionMode(parent, "ask");
    const child = String((await start(parent)).session_id);
    setPermissionMode(parent, "full");
    const read = await call({ sessionId: parent, tool: "configure_session", args: { session_id: child } });
    expect(read.access).toBe("ask");
    expect(permissionModeOf(useApp.getState(), child)).toBe("ask");
  });

  test("start_session starts on the login it names, and the card shows its id and name", async () => {
    const parent = await ready();
    await addHarnessAccount("codex", "work", "Work");
    const answering = answerOrchestration({
      sessionId: parent,
      tool: "start_session",
      args: { prompt: "do it", title: "Child", harness: "codex", account: "Work" },
    });
    await until(() => useApp.getState().grant !== null);
    expect(useApp.getState().grant?.harnesses).toEqual(["Codex · Work (work)"]);
    resolveGrant("once");
    const answer = await answering;
    expect(answer.error).toBeUndefined();
    expect(answer.account).toBe("work");
    expect(useApp.getState().threads[String(answer.session_id)]?.harness?.account).toBe("work");
  });

  test("fan_out runs one task on each login it names", async () => {
    const parent = await ready();
    await addHarnessAccount("codex", "work", "Work");
    await addHarnessAccount("codex", "home", "Home");
    const answer = await call({
      sessionId: parent,
      tool: "fan_out",
      args: {
        prompt: "do it",
        harnesses: [
          { harness: "codex", account: "work" },
          { harness: "codex", account: "Home" },
        ],
      },
    });
    const links = answer.links as unknown as { session_id: string }[];
    expect(links).toHaveLength(2);
    const accounts = links.map((link) => useApp.getState().threads[link.session_id]?.harness?.account);
    expect(accounts).toEqual(["work", "home"]);
    // The link keeps the login too, so the Compare card names it.
    const shown = useApp.getState().links.links.filter((link) => link.fanOut === answer.fan_out_id);
    expect(shown.map((link) => link.account)).toEqual(["work", "home"]);
  });

  test("fan_out refuses entries that resolve to the same login", async () => {
    const parent = await ready();
    await addHarnessAccount("codex", "work", "Work");
    // Two spellings the engine sees as different, but which name one login.
    const answer = await answerOrchestration({
      sessionId: parent,
      tool: "fan_out",
      args: {
        prompt: "do it",
        harnesses: [
          { harness: "codex", account: "work" },
          { harness: "codex", account: "Work" },
        ],
      },
    });
    const message = (answer.error as { message: string } | undefined)?.message ?? "";
    expect(message).toContain("same target twice");
    expect(useApp.getState().grant).toBeNull();
  });

  test("an ambiguous or signed-out login is refused before the card", async () => {
    const parent = await ready();
    await addHarnessAccount("codex", "a", "Shared");
    await addHarnessAccount("codex", "b", "Shared");
    const ambiguous = await answerOrchestration({
      sessionId: parent,
      tool: "start_session",
      args: { prompt: "do it", harness: "codex", account: "Shared" },
    });
    const said = (ambiguous.error as { message: string } | undefined)?.message ?? "";
    expect(said).toContain("more than one account matching");
    expect(useApp.getState().grant).toBeNull();
    // A login the plan read found signed out cannot run a turn.
    useApp.setState({ signedOutLogins: { "codex#a": true } });
    const signedOut = await answerOrchestration({
      sessionId: parent,
      tool: "start_session",
      args: { prompt: "do it", harness: "codex", account: "a" },
    });
    const no = (signedOut.error as { message: string } | undefined)?.message ?? "";
    expect(no).toContain("is signed out");
    expect(useApp.getState().grant).toBeNull();
  });
});

describe("retrying a call that starts or messages a thread", () => {
  const ready = async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const caller = useApp.getState().sessions[0]!.id;
    // Full access answers for the user on the card, so these tests see the
    // receipt and nothing else.
    setPermissionMode(caller, "full");
    return caller;
  };
  /** A call the engine forwards with a retry key and the hash of its arguments. */
  const keyed = (caller: string, argsHash: string, args: JsonObject = {}) => ({
    sessionId: caller,
    tool: "start_session",
    args: { prompt: "do it", title: "Child", client_request_id: "call-1", ...args },
    clientRequestId: "call-1",
    argsHash,
  });
  const started = () => useApp.getState().links.links.length;

  test("a retried start returns the first answer instead of starting another thread", async () => {
    const caller = await ready();
    const threads = useApp.getState().sessions.length;
    const first = await answerOrchestration(keyed(caller, "h1"));
    expect(first.error).toBeUndefined();
    expect(first.replayed).toBeUndefined();
    expect(started()).toBe(1);
    const session = String(first.session_id);
    const link = String(first.link_id);

    // The answer the app gave was dropped on the way back: the same call again.
    const again = await answerOrchestration(keyed(caller, "h1"));
    expect(again.error).toBeUndefined();
    expect(again.replayed).toBe(true);
    expect(again.session_id).toBe(session);
    expect(again.link_id).toBe(link);
    expect(started()).toBe(1);
    expect(useApp.getState().sessions.length).toBe(threads + 1);
  });

  test("the same key with other arguments is refused", async () => {
    const caller = await ready();
    await answerOrchestration(keyed(caller, "h1"));
    const changed = await answerOrchestration(keyed(caller, "h2", { prompt: "something else" }));
    const said = (changed.error as { message: string } | undefined)?.message ?? "";
    expect(said).toContain("client_request_id reused with different arguments");
    expect(started()).toBe(1);
    // Its own key is what another call of the same arguments may use.
    const other = await answerOrchestration({
      ...keyed(caller, "h2", { prompt: "elsewhere" }),
      clientRequestId: "call-2",
    });
    expect(other.error).toBeUndefined();
    expect(other.replayed).toBeUndefined();
    expect(started()).toBe(2);
  });

  test("two duplicates in flight at once start one thread and both hear of it", async () => {
    const caller = await ready();
    const [one, two] = await Promise.all([
      answerOrchestration(keyed(caller, "h1")),
      answerOrchestration(keyed(caller, "h1")),
    ]);
    expect(one.error).toBeUndefined();
    expect(two.error).toBeUndefined();
    expect(started()).toBe(1);
    expect(two.session_id).toBe(one.session_id);
    // Whichever finished second is the replay.
    expect([one.replayed, two.replayed].filter(Boolean)).toHaveLength(1);
  });

  test("a refused call leaves no receipt, so a corrected retry runs", async () => {
    const caller = await ready();
    const refused = await answerOrchestration(keyed(caller, "h1", { harness: "nope" }));
    expect((refused.error as { message: string } | undefined)?.message).toContain("can run now");
    const corrected = await answerOrchestration(keyed(caller, "h2", { harness: "rustcode" }));
    expect(corrected.error).toBeUndefined();
    expect(corrected.replayed).toBeUndefined();
    expect(started()).toBe(1);
  });

  test("a retried message is delivered once, not twice", async () => {
    const caller = await ready();
    const child = String((await answerOrchestration(keyed(caller, "h1"))).session_id);
    const params = {
      sessionId: caller,
      tool: "message_session",
      args: { session_id: child, message: "one more thing", client_request_id: "m1" },
      clientRequestId: "m1",
      argsHash: "hm",
    };
    const first = await answerOrchestration(params);
    expect(first.error).toBeUndefined();
    const again = await answerOrchestration(params);
    expect(again.replayed).toBe(true);
    expect(again.link_id).toBe(first.link_id);
    const sent = useApp.getState().links.links.filter((l) => l.message === "one more thing");
    expect(sent).toHaveLength(1);
  });

  test("a call with no retry key behaves as before", async () => {
    const caller = await ready();
    const plain = { sessionId: caller, tool: "start_session", args: { prompt: "do it", title: "Child" } };
    const first = await answerOrchestration(plain);
    const second = await answerOrchestration(plain);
    expect(first.error).toBeUndefined();
    expect(second.error).toBeUndefined();
    expect(second.replayed).toBeUndefined();
    // Two calls, two threads: nothing is remembered without a key.
    expect(first.session_id).not.toBe(second.session_id);
    expect(started()).toBe(2);
  });
});

describe("what an agent may do with the messages it queued", () => {
  const ready = async (delayMs = 0) => {
    await connect(new MockTransport({ delayMs }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const caller = useApp.getState().sessions[0]!.id;
    // Full access answers for the user, so the tests see the queue and nothing else.
    setPermissionMode(caller, "full");
    return caller;
  };
  /** Another thread of the same project, held busy so what arrives waits on it. */
  const elsewhere = (caller: string) =>
    useApp.getState().sessions.find((x) => x.id !== caller && x.cwd === "/Users/you/code/rustcode")!.id;
  const busy = async (caller: string) => {
    const target = elsewhere(caller);
    await openThread(target);
    updateThread(target, (t) => ({ ...t, running: true }));
    return target;
  };
  const queueOn = async (caller: string, target: string, message: string) => {
    const answer = await answerOrchestration({
      sessionId: caller,
      tool: "message_session",
      args: { session_id: target, message },
    });
    expect(answer.error).toBeUndefined();
    return String(answer.link_id);
  };
  const manage = (caller: string, target: string, args: JsonObject) =>
    answerOrchestration({ sessionId: caller, tool: "manage_queue", args: { session_id: target, ...args } });
  const said = (answer: JsonObject) => (answer.error as { message: string } | undefined)?.message ?? "";
  const queue = (target: string) => useApp.getState().threads[target]!.queue;

  test("a list shows this thread's own messages in send order, and counts the rest", async () => {
    const caller = await ready();
    const target = await busy(caller);
    await queueOn(caller, target, "first thing");
    await queueOn(caller, target, "second thing");
    // The user's own message, and one another thread queued: neither is this thread's to read.
    updateThread(target, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        { id: "user-1", text: "mine, not the agent's", queuedAt: Date.now() },
        { id: "theirs-1", text: "another thread's", from: "someone-else", queuedAt: Date.now() },
      ],
    }));
    const list = await manage(caller, target, { action: "list" });
    expect(list.error).toBeUndefined();
    expect(list.running).toBe(true);
    const items = list.queued as unknown as { queue_id: string; preview: string; queued_at: string }[];
    expect(items.map((item) => item.preview)).toEqual(["first thing", "second thing"]);
    expect(items[0]!.queue_id).not.toBe(items[1]!.queue_id);
    expect(new Date(items[0]!.queued_at).getTime()).toBeGreaterThan(0);
    // Only a count: what waits there that this thread did not queue.
    expect(list.others).toBe(2);
  });

  test("an edit changes the text where it waits, and nothing else", async () => {
    const caller = await ready();
    const target = await busy(caller);
    const first = await queueOn(caller, target, "first thing");
    await queueOn(caller, target, "second thing");
    const listed = await manage(caller, target, { action: "list" });
    const second = (listed.queued as unknown as { queue_id: string }[])[1]!.queue_id;
    const edited = await manage(caller, target, {
      action: "edit",
      queue_id: second,
      text: "  what I meant  ",
    });
    expect(edited.error).toBeUndefined();
    expect(edited.preview).toBe("what I meant");
    // It still says who asked; the edit replaced what was said, not the attribution.
    expect(queue(target).map((q) => q.text)).toEqual([
      expect.stringContaining("first thing"),
      expect.stringContaining("what I meant"),
    ]);
    // The link card says what was sent, so it says what it says now.
    const link = useApp.getState().links.links.find((l) => l.id === first);
    expect(link?.message).toContain("first thing");
    const changed = useApp
      .getState()
      .links.links.find((l) => l.target === target && l.message !== link?.message);
    expect(changed?.message).toBe("what I meant");
    // It is still the message it was: the same id, still waiting.
    expect(queue(target).find((q) => q.text.includes("what I meant"))?.id).toBe(second);
  });

  test("a cancel drops it from the queue, and its link never answers", async () => {
    const caller = await ready();
    const target = await busy(caller);
    const link = await queueOn(caller, target, "never mind");
    const id = queue(target)[0]!.id;
    const cancelled = await manage(caller, target, { action: "cancel", queue_id: id });
    expect(cancelled.error).toBeUndefined();
    expect(cancelled.cancelled).toBe(true);
    expect(queue(target)).toHaveLength(0);
    expect(useApp.getState().links.links.find((l) => l.id === link)?.state).toBe("cancelled");
    const remaining = await manage(caller, target, { action: "list" });
    expect(remaining.queued).toEqual([]);
  });

  test("a message that has started is no longer anyone's to change", async () => {
    const caller = await ready();
    const target = await busy(caller);
    await queueOn(caller, target, "gone ahead");
    const id = queue(target)[0]!.id;
    // What a turn starting does: the row leaves the queue.
    updateThread(target, (t) => ({ ...t, queue: t.queue.filter((q) => q.id !== id) }));
    for (const action of ["edit", "cancel", "steer"]) {
      const refused = await manage(caller, target, { action, queue_id: id, text: "changed" });
      expect(said(refused)).toContain("already started");
    }
    // An id that never was a message of that thread is not found instead.
    const unknown = await manage(caller, target, { action: "cancel", queue_id: "never-was" });
    expect((unknown.error as { code: string }).code).toBe("not_found");
  });

  test("another thread's message is not this one's to touch", async () => {
    const caller = await ready();
    const target = await busy(caller);
    updateThread(target, (t) => ({
      ...t,
      queue: [...t.queue, { id: "theirs-1", text: "another thread's", from: "someone-else" }],
    }));
    for (const action of ["edit", "cancel", "steer"]) {
      const refused = await manage(caller, target, { action, queue_id: "theirs-1", text: "changed" });
      expect(said(refused)).toContain("was not queued by this thread");
    }
    expect(queue(target).map((q) => q.text)).toEqual(["another thread's"]);
  });

  test("a harness that takes no message mid-turn cannot steer, and the message stays", async () => {
    const caller = await ready();
    const target = await busy(caller);
    await queueOn(caller, target, "go in now");
    // A harness that says a message waits for the next turn, in the thread's own record and in the
    // snapshot both: the fresher of the two is what the send rule reads.
    const only = {
      live: false,
      attachments: false,
      while_awaiting_user: true,
      interrupt_restart: false,
      queue: true,
    };
    updateThread(target, (t) => ({
      ...t,
      harness: t.harness
        ? {
            ...t.harness,
            capabilities: { ...t.harness.capabilities, steer: "none" as const, steering: only },
          }
        : t.harness,
    }));
    useApp.setState((s) => ({
      harnesses: s.harnesses && {
        ...s.harnesses,
        harnesses: s.harnesses.harnesses.map((h) =>
          h.id === (useApp.getState().threads[target]?.harness?.id ?? "rustcode")
            ? { ...h, capabilities: { ...h.capabilities, steering: only } }
            : h,
        ),
      },
    }));
    const id = queue(target)[0]!.id;
    const refused = await manage(caller, target, { action: "steer", queue_id: id });
    expect(said(refused)).toBe("this harness cannot steer");
    expect(queue(target).map((q) => q.text)).toEqual([expect.stringContaining("go in now")]);
  });

  test("a steer goes into the running turn now, and leaves the queue", async () => {
    const caller = await ready(200);
    const target = elsewhere(caller);
    await openThread(target);
    selectThread(target);
    const turning = submit("do the thing");
    await until(() => useApp.getState().threads[target]?.running === true);
    await queueOn(caller, target, "instead, do this");
    const id = queue(target)[0]!.id;
    const steered = await manage(caller, target, { action: "steer", queue_id: id });
    expect(steered.error).toBeUndefined();
    expect(steered.steered).toBe(true);
    expect(queue(target).find((q) => q.id === id)).toBeUndefined();
    await turning;
  });

  test("a thread of another project's queue is refused", async () => {
    const caller = await ready();
    const other = useApp.getState().sessions.find((x) => x.cwd === "/Users/you/code/website")!;
    const refused = await manage(caller, other.id, { action: "list" });
    expect((refused.error as { code: string }).code).toBe("not_allowed");
  });
});

describe("a delegated thread's place in the sidebar", () => {
  const ready = async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    return useApp.getState().sessions[0]!.id;
  };

  test("a thread an agent starts is nested from its first frame, with no link in the book yet", async () => {
    const parent = await ready();
    // The window the flash lived in: `session/new` has answered, the link is not recorded yet, and
    // the engine has not listed the thread (`refreshSessions` keeps this very summary until it does).
    const id = await createSession({
      cwd: useApp.getState().projectCwd,
      harness: "rustcode",
      parent,
      title: "Child",
    });
    const summary = useApp.getState().sessions.find((x) => x.id === id)!;
    expect(summary.parent).toMatchObject({ id: parent, relationship: "delegate" });
    const listed = new Set(useApp.getState().sessions.map((x) => x.id));
    expect(useApp.getState().links.links).toHaveLength(0);
    expect(topLevel(summary, [], listed)).toBe("nested");
    // The same summary without the record — what the app inserted before — is a row of its own.
    expect(topLevel({ ...summary, parent: undefined }, [], listed)).toBe("shown");
  });

  test("a thread a person starts has no parent, and keeps its row", async () => {
    await ready();
    const id = await createSession({ cwd: useApp.getState().projectCwd, harness: "rustcode" });
    const summary = useApp.getState().sessions.find((x) => x.id === id)!;
    expect(summary.parent).toBeUndefined();
    expect(topLevel(summary, [], new Set([id]))).toBe("shown");
  });
});

describe("forks and side chats", () => {
  const ready = async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const id = useApp.getState().sessions[0]!.id;
    await openThread(id);
    selectThread(id);
    return id;
  };
  const summaryOf = (id: string) => useApp.getState().sessions.find((x) => x.id === id)!;

  test("/fork lists a new thread that records its parent as a fork, nested under it", async () => {
    const parent = await ready();
    const before = new Set(useApp.getState().sessions.map((x) => x.id));
    await forkThread(parent);
    const fork = useApp.getState().sessions.find((x) => !before.has(x.id));
    expect(fork?.parent).toMatchObject({ id: parent, relationship: "fork" });
    const listed = new Set(useApp.getState().sessions.map((x) => x.id));
    expect(topLevel(fork!, [], listed)).toBe("nested");
    expect(branchesOf(useApp.getState().sessions, parent).map((b) => [b.id, b.kind])).toContainEqual([
      fork!.id,
      "fork",
    ]);
    expect(originOf(useApp.getState().sessions, fork!.id)).toMatchObject({ id: parent, kind: "fork" });
  });

  test("a side chat shares the parent's harness, model and folder, and opens beside it", async () => {
    const parent = await ready();
    const thread = useApp.getState().threads[parent]!;
    const id = await openSideChat(parent, "First line\nSecond line");
    expect(id).toBeDefined();
    const side = summaryOf(id!);
    expect(side.parent).toMatchObject({ id: parent, relationship: "side_chat" });
    // The same folder and harness, and no worktree of its own.
    expect(side.cwd).toBe(thread.cwd);
    expect(side.harness).toBe(thread.harness?.id);
    expect(useApp.getState().threads[id!]?.worktree).toBeUndefined();
    // The message arrives as a quote in its composer, and the panel shows it beside the parent.
    expect(useApp.getState().drafts[id!]).toBe("> First line\n> Second line\n\n");
    expect(useApp.getState().peekId).toBe(id ?? null);
    expect(useApp.getState().panel).toBe("peek");
    expect(useApp.getState().selectedId).toBe(parent);
  });

  test("closing the panel keeps the side chat, and writing in it does not give it a row of its own", async () => {
    const parent = await ready();
    const id = (await openSideChat(parent, "A reply"))!;
    setPanel(null);
    expect(useApp.getState().sessions.some((x) => x.id === id)).toBe(true);
    const listed = new Set(useApp.getState().sessions.map((x) => x.id));
    expect(topLevel({ ...summaryOf(id), messaged: true }, [], listed)).toBe("nested");
    // A fork the person writes in is theirs now, as a delegate is.
    const fork: DelegatedThread = { id: "f", parent: { id: parent, relationship: "fork" }, messaged: true };
    expect(topLevel(fork, [], listed)).toBe("shown");
  });

  test("a side chat is never broader than the thread it was asked from", async () => {
    const parent = await ready();
    useApp.setState({ defaultPermissionMode: "full" });
    setPermissionMode(parent, "ask");
    const narrow = (await openSideChat(parent, "A reply"))!;
    expect(permissionModeOf(useApp.getState(), narrow)).toBe("ask");
    // A parent on Full access with the default on Auto-edit: the default stands.
    useApp.setState({ defaultPermissionMode: "edits" });
    setPermissionMode(parent, "full");
    const same = (await openSideChat(parent, "Another"))!;
    expect(permissionModeOf(useApp.getState(), same)).toBe("edits");
  });

  test("an engine that does not record the relationship opens no side chat", async () => {
    const parent = await ready();
    const before = useApp.getState().sessions.length;
    useApp.setState((s) => ({ peekId: null, agent: s.agent && { ...s.agent, lineage: false } }));
    expect(await openSideChat(parent, "A reply")).toBeUndefined();
    expect(useApp.getState().sessions).toHaveLength(before);
    expect(useApp.getState().peekId).toBeNull();
  });

  test("a side chat of nothing is not opened", async () => {
    await ready();
    expect(await openSideChat("no-such-thread", "text")).toBeUndefined();
    const parent = useApp.getState().sessions[0]!.id;
    expect(await openSideChat(parent, "   ")).toBeUndefined();
  });

  test("a quote keeps blank lines as empty quote lines", () => {
    expect(quoteMessage("a\n\nb\n")).toBe("> a\n>\n> b\n\n");
  });
});

describe("a message sent from the new-thread screen", () => {
  const ready = async (transport = new MockTransport({ delayMs: 0 })) => {
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    startNewThread();
  };
  /**
   * A harness that will not start, once. The app no longer sends a draft on a harness that cannot
   * run (state/store.ts `newThreadChoice` replaces it), so the refusal is the engine's: its first
   * `session/new` names a harness it does not know, as a harness that cannot launch answers, and
   * the retry after it starts normally.
   */
  const refusesOnce = () => {
    const transport = new MockTransport({ delayMs: 0 });
    const send = transport.send.bind(transport);
    let refused = false;
    transport.send = (line: string) => {
      const message = JSON.parse(line) as { method?: string; params?: JsonObject };
      const meta = message.params?._meta as JsonObject | undefined;
      const rustcode = meta?.rustcode as JsonObject | undefined;
      if (message.method !== "session/new" || !rustcode || refused) return send(line);
      refused = true;
      rustcode.harness = "no-such-harness";
      return send(JSON.stringify(message));
    };
    return transport;
  };
  const picture = {
    id: "img1",
    kind: "image" as const,
    name: "shot.png",
    mimeType: "image/png",
    data: "AAAA",
  };

  test("shows the message before the session exists, and sends it once after", async () => {
    await ready();
    const before = useApp.getState().sessions.length;
    const sending = submit("rename the greeting");
    // The press is answered on the spot: the message is on screen, and no session is asked for yet.
    const pending = useApp.getState().pendingSetup;
    expect(pending?.text).toBe("rename the greeting");
    expect(pending?.title).toBe("rename the greeting");
    expect(useApp.getState().selectedId).toBeNull();
    expect(useApp.getState().sessions.length).toBe(before);

    expect(await sending).toBe(true);
    const s = useApp.getState();
    const id = s.selectedId!;
    expect(s.pendingSetup).toBeNull();
    expect(s.sessions[0]?.id).toBe(id);
    expect(s.sessions.length).toBe(before + 1);
    // One message, and no second copy waiting behind it.
    const mine = s.threads[id]!.items.filter((i) => i.kind === "user");
    expect(mine).toHaveLength(1);
    expect(mine[0]!.text).toBe("rename the greeting");
    expect(s.threads[id]!.queue).toHaveLength(0);
  });

  test("the setup goes and its thread is selected in one update", async () => {
    await ready();
    // Every state the store passes through, from the press to the answer. The chat hands the screen
    // from the setup to the thread, so it needs a state where the setup has gone and a thread is
    // selected: a gap between those two is a screen with nothing to hand over to.
    const seen: { pending: boolean; selected: string | null }[] = [];
    const stop = useApp.subscribe((s) => seen.push({ pending: !!s.pendingSetup, selected: s.selectedId }));
    expect(await submit("run the tests")).toBe(true);
    stop();
    const fromPress = seen.slice(seen.findIndex((s) => s.pending));
    // It saw the setup appear and then go: a recording that caught one state could not tell.
    expect(fromPress.length).toBeGreaterThan(1);
    expect(fromPress.some((s) => !s.pending && !s.selected)).toBe(false);

    const s = useApp.getState();
    expect(s.pendingSetup).toBeNull();
    const id = s.selectedId;
    expect(id).not.toBeNull();
    expect(s.threads[id!]!.items.some((i) => i.kind === "user" && i.text === "run the tests")).toBe(true);
  });

  test("a second send while the first sets up starts no second thread", async () => {
    await ready();
    const before = useApp.getState().sessions.length;
    const first = submit("one");
    const setupId = useApp.getState().pendingSetup?.id;
    // The composer puts its text back when the send answers false: nothing is lost either way.
    expect(await submit("two")).toBe(false);
    expect(useApp.getState().pendingSetup?.id).toBe(setupId);
    expect(await first).toBe(true);
    expect(useApp.getState().sessions).toHaveLength(before + 1);
    expect(
      useApp.getState().threads[useApp.getState().selectedId!]?.items.filter((i) => i.kind === "user"),
    ).toHaveLength(1);
  });

  test("a failure keeps the message and its pictures, and Retry sends them again", async () => {
    await ready(refusesOnce());
    const before = useApp.getState().sessions.length;
    // True: the field stays clear, because the failed bubble holds the message and it can be retried.
    expect(await submit("look at this", { ...emptyExtras, attachments: [picture] })).toBe(true);
    const failed = useApp.getState().pendingSetup;
    expect(failed?.failure?.text).toContain("no-such-harness");
    expect(failed?.text).toBe("look at this");
    expect(failed?.extras?.attachments).toHaveLength(1);
    expect(useApp.getState().sessions.length).toBe(before);
    expect(useApp.getState().selectedId).toBeNull();

    // Retry on a harness that starts sends the same message, with its picture, once.
    switchNewThreadHarness("rustcode");
    expect(await retryPendingSetup()).toBe(true);
    const s = useApp.getState();
    expect(s.pendingSetup).toBeNull();
    const mine = s.threads[s.selectedId!]!.items.filter((i) => i.kind === "user");
    expect(mine).toHaveLength(1);
    expect(mine[0]!.text).toBe("look at this");
    expect(mine[0]!.attachments).toHaveLength(1);
  });

  test("Dismiss gives up the failed send and puts what it held back in the field", async () => {
    await ready(refusesOnce());
    await submit("look at this", { ...emptyExtras, attachments: [picture] });
    dismissPendingSetup();
    expect(useApp.getState().pendingSetup).toBeNull();
    expect(useApp.getState().drafts.new).toBe("look at this");
    expect(useApp.getState().extras.new?.attachments).toHaveLength(1);
  });
});

describe("plan usage cache", () => {
  // `commitPlansAnswer` keeps no Claude row while the meter is off, so these
  // tests read as if the user had allowed it.
  let storage: ReturnType<typeof testStorage>;
  beforeEach(() => {
    storage = testStorage();
  });
  afterEach(() => {
    storage.done();
  });

  const plan = (used: number, fetchedAt: number): CachedPlan => ({
    source: "claude-code",
    windows: [{ usedPercent: used, minutes: 300 }],
    fetchedAt,
  });

  test("keeps the newer numbers for each harness and never erases them", () => {
    const before = { "claude-code": plan(20, 1000) };
    // A newer answer replaces the numbers, and is the same map shape.
    expect(
      mergePlans(before, { "claude-code": plan(40, 2000) })["claude-code"]?.windows[0]?.usedPercent,
    ).toBe(40);
    expect(before["claude-code"]?.windows[0]?.usedPercent).toBe(20);
    // A slow answer from before the numbers already shown does not go back in time.
    expect(mergePlans(before, { "claude-code": plan(10, 500) })).toBe(before);
    // The same instant keeps what is shown: no churn, no re-render.
    expect(mergePlans(before, { "claude-code": plan(10, 1000) })).toBe(before);
    // A harness whose plan cannot be read (Claude Code's limits answer null) adds nothing.
    const empty = mergePlans(before, { codex: { source: "codex", windows: [], fetchedAt: 3000 } });
    expect(empty).toBe(before);
    // Each harness is its own entry.
    const both = mergePlans(before, { codex: plan(70, 3000) });
    expect(Object.keys(both).sort()).toEqual(["claude-code", "codex"]);
    expect((before as Record<string, CachedPlan>).codex).toBeUndefined();
  });

  test("staleness follows the three-minute poll, with room for a read that ends after its tick", () => {
    const now = 10_000_000;
    expect(PLAN_CACHE_POLL_MS).toBe(180_000);
    expect(PLAN_STALE_MARGIN_MS).toBe(15_000);
    expect(planIsStale(undefined, now)).toBe(true);
    const whole = PLAN_CACHE_POLL_MS - PLAN_STALE_MARGIN_MS;
    expect(planIsStale(plan(20, now - whole + 1), now)).toBe(false);
    expect(planIsStale(plan(20, now - whole), now)).toBe(true);
    // A read stamps `fetchedAt` a moment after the tick that asked for it, so
    // the exact interval would leave this fresh at the next tick and read every
    // six minutes instead of three.
    expect(planIsStale(plan(20, now - PLAN_CACHE_POLL_MS + 5_000), now)).toBe(true);
  });

  test("a stored off setting does not discard Claude usage", () => {
    resetForTests();
    const storage = testStorage();
    try {
      const claude: UsagePlan = {
        id: "claude",
        name: "Claude",
        via: "Claude Code",
        harness: "claude-code",
        status: "ok",
        plan: "max",
        windows: [{ label: "5-hour", usedPercent: 18, minutes: 300 }],
      };
      const codex: UsagePlan = {
        id: "codex",
        name: "ChatGPT",
        via: "Codex CLI",
        status: "ok",
        windows: [{ label: "Weekly", usedPercent: 46, minutes: 10_080 }],
      };
      commitPlansAnswer([claude, codex]);
      expect(useApp.getState().planCache["claude-code"]).toBeDefined();
      // A legacy stored off value must not prevent a later answer being kept.
      localStorage.setItem("rustcode.claudeUsage", "0");
      commitPlansAnswer([
        claude,
        { ...claude, id: "claude:work", account: "work", status: "signedOut" },
        codex,
      ]);
      expect(useApp.getState().planCache["claude-code"]).toBeDefined();
      expect(useApp.getState().planCache["claude-code#work"]).toBeUndefined();
      expect(useApp.getState().signedOutLogins["claude-code"]).toBeUndefined();
      // Every other subscription is committed as usual.
      expect(useApp.getState().planCache.codex?.windows[0]?.usedPercent).toBe(46);
    } finally {
      storage.done();
    }
  });

  test("the plans answer maps a subscription to its harness, dropping ones with no windows", () => {
    const cache = plansToCache(
      parsePlans({
        plans: [
          {
            id: "claude",
            name: "Claude",
            via: "Claude Code",
            status: "ok",
            plan: "max",
            windows: [{ label: "5-hour", usedPercent: 18, windowMinutes: 300, resetsAt: 1000 }],
          },
          { id: "codex", name: "ChatGPT", status: "signedOut", windows: [] },
        ],
      }),
      5000,
    );
    expect(Object.keys(cache)).toEqual(["claude-code"]);
    // The window keeps the name the provider gave it, so `list_harnesses` can
    // report it to an agent (state/harnessUsage.ts).
    expect(cache["claude-code"]).toEqual({
      source: "claude-code",
      windows: [{ label: "5-hour", usedPercent: 18, minutes: 300, resetsAt: 1_000_000 }],
      fetchedAt: 5000,
      name: "Claude",
      plan: "max",
      harness: "claude-code",
      account: null,
    });
  });

  test("each account is its own cache entry", () => {
    const cache = plansToCache(
      parsePlans({
        plans: [
          {
            id: "claude",
            name: "Claude",
            harness: "claude-code",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 18, windowMinutes: 300 }],
          },
          {
            id: "claude:paral-2",
            name: "Paral 2",
            harness: "claude-code",
            account: "paral-2",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 71, windowMinutes: 300 }],
          },
        ],
      }),
    );
    expect(Object.keys(cache).sort()).toEqual(["claude-code", "claude-code#paral-2"]);
    expect(cache["claude-code#paral-2"]?.account).toEqual("paral-2");
    expect(cache["claude-code"]?.windows[0]?.usedPercent).toBe(18);
    expect(harnessPlanKey("claude-code", "paral-2")).toBe("claude-code#paral-2");
    expect(harnessPlanKey("claude-code", null)).toBe("claude-code");
  });

  test("a thread's own limits always win over the cache", () => {
    const own: CachedPlan = {
      source: "codex",
      windows: [{ usedPercent: 42, minutes: 300 }],
      fetchedAt: 2000,
    };
    expect(shownPlan(own, undefined).plan).toBe(own);
    expect(shownPlan(own, undefined).cached).toBe(false);
    // A thread on Claude Code has no limits of its own: the cache shows through.
    const cached = plan(18, 1000);
    expect(shownPlan(null, cached)).toEqual({ plan: cached, cached: true });
    // A newer cache is another account's numbers (`rustcode` runs several
    // providers): the thread's own still win, and are not marked cached.
    expect(shownPlan(own, plan(30, 3000))).toEqual({ plan: own, cached: false });
    // An answer without windows is not a plan.
    expect(shownPlan({ source: "codex", windows: [], fetchedAt: 9000 }, cached)).toEqual({
      plan: cached,
      cached: true,
    });
    // A thread whose harness the engine did not name uses the one plan there is.
    expect(onlyCachedPlan({})).toBeUndefined();
    expect(onlyCachedPlan({ "claude-code": cached })).toBe(cached);
    expect(onlyCachedPlan({ "claude-code": cached, codex: plan(40, 2000) })).toBeUndefined();
  });

  test("a harness that is signed out loses its entry, and a day-old cache is dropped", () => {
    const cache = { "claude-code": plan(20, 1000), codex: plan(40, 2000) };
    expect(forgetPlans(cache, ["codex"])).toEqual({ "claude-code": plan(20, 1000) });
    // Nothing to forget: the same map comes back, so nothing re-renders.
    expect(forgetPlans(cache, ["nope"])).toBe(cache);
    expect(cache.codex).toBeDefined();
    // Launch after a day: numbers older than that are not shown at all.
    const now = 20 * 24 * 60 * 60_000;
    const old = plan(20, now - PLAN_CACHE_MAX_AGE_MS - 1);
    const fresh = plan(40, now);
    expect(prunePlans({ old, fresh }, now)).toEqual({ fresh });
    expect(prunePlans(cache, 2000)).toBe(cache);
  });

  test("a window whose reset has passed reads as reset, not as its old numbers", () => {
    const now = 10_000;
    const passed: CachedPlan = {
      source: "claude-code",
      windows: [{ usedPercent: 92, minutes: 300, resetsAt: now - 1000 }],
      fetchedAt: now - 7000,
    };
    expect(shownPlan(null, passed, now)).toEqual({
      plan: { ...passed, windows: [{ usedPercent: 0, minutes: 300 }] },
      cached: true,
    });
    // A window still running keeps its numbers.
    const running: CachedPlan = {
      ...passed,
      windows: [{ usedPercent: 92, minutes: 300, resetsAt: now + 60_000 }],
    };
    expect(shownPlan(null, running, now)).toEqual({ plan: running, cached: true });
    expect(windowNow({ usedPercent: 5, minutes: 60, resetsAt: now }, now)).toEqual({
      usedPercent: 0,
      minutes: 60,
    });
    expect(planNow({ source: "codex", windows: [] }, now)).toBeUndefined();
  });

  test("coming back to the window catches up when any harness is stale", () => {
    const now = 10_000_000;
    expect(planCacheStale({}, now)).toBe(true);
    expect(planCacheStale({ "claude-code": plan(20, now), codex: plan(40, now) }, now)).toBe(false);
    // One stale harness among fresh ones is enough to ask again.
    const mixed = { "claude-code": plan(20, now), codex: plan(40, now - PLAN_CACHE_POLL_MS) };
    expect(planCacheStale(mixed, now)).toBe(true);
  });

  test("a plans answer drops a harness it says is signed out", () => {
    resetForTests();
    useApp.setState({ planCache: { "claude-code": plan(20, 1000), codex: plan(40, 1000) } });
    commitPlansAnswer(
      parsePlans({
        plans: [
          {
            id: "claude",
            name: "Claude",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 30, windowMinutes: 300 }],
          },
          { id: "codex", name: "ChatGPT", status: "signedOut", windows: [] },
        ],
      }),
    );
    const cache = useApp.getState().planCache;
    expect(cache.codex).toBeUndefined();
    expect(cache["claude-code"]?.windows[0]?.usedPercent).toBe(30);
    resetForTests();
  });

  test("a partial answer never drops another account's numbers", () => {
    // The app asks for one login at a time, so an answer often holds one row.
    const before = {
      "claude-code": plan(20, 1000),
      "claude-code#work": plan(40, 1000),
      "claude-code#home": plan(60, 1000),
      codex: plan(80, 1000),
    };
    const answer = plansToCache(
      parsePlans({
        plans: [
          { id: "claude", name: "Claude", harness: "claude-code", status: "notAsked", windows: [] },
          {
            id: "claude:work",
            name: "Work",
            harness: "claude-code",
            account: "work",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 50, windowMinutes: 300 }],
          },
        ],
      }),
      5000,
    );
    const after = mergePlans(forgetPlans(before, []), answer);
    expect(after["claude-code#work"]?.windows[0]?.usedPercent).toBe(50);
    // What the answer did not read keeps its numbers, untouched.
    expect(after["claude-code"]).toBe(before["claude-code"]);
    expect(after["claude-code#home"]).toBe(before["claude-code#home"]);
    expect(after.codex).toBe(before.codex);
  });

  test("a partial answer leaves the other logins' cache and sign-out record alone", () => {
    resetForTests();
    useApp.setState({
      planCache: {
        "claude-code": plan(20, 1000),
        "claude-code#work": plan(40, 1000),
        codex: plan(80, 1000),
      },
      signedOutLogins: { "claude-code": true },
    });
    commitPlansAnswer(
      parsePlans({
        plans: [
          { id: "claude", name: "Claude", harness: "claude-code", status: "notAsked", windows: [] },
          {
            id: "claude:work",
            name: "Work",
            harness: "claude-code",
            account: "work",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 12, windowMinutes: 300 }],
          },
        ],
      }),
    );
    const state = useApp.getState();
    expect(state.planCache["claude-code#work"]?.windows[0]?.usedPercent).toBe(12);
    expect(state.planCache["claude-code"]).toBeDefined();
    expect(state.planCache.codex).toBeDefined();
    // The default login was not read, so what an earlier answer found stands.
    expect(state.signedOutLogins).toEqual({ "claude-code": true });
    resetForTests();
  });

  test("a thread's numbers belong to the account it runs on, known from the list", () => {
    resetForTests();
    useApp.setState({
      sessions: [
        { id: "listed", cwd: "/tmp", harness: "claude-code", account: "work" },
        { id: "default-login", cwd: "/tmp", harness: "claude-code" },
      ],
    });
    const state = useApp.getState();
    // Before the thread is opened, the list's summary names its account.
    expect(planKeyFor(state, "listed")).toBe("claude-code#work");
    expect(planKeyFor(state, "default-login")).toBe("claude-code");
    resetForTests();
  });

  test("the plans answer records which login is signed out, and clears it when it reads again", () => {
    resetForTests();
    commitPlansAnswer(
      parsePlans({
        plans: [
          { id: "claude", name: "Claude", harness: "claude-code", status: "signedOut", windows: [] },
          {
            id: "claude:work",
            name: "Work",
            harness: "claude-code",
            account: "work",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 4, windowMinutes: 300 }],
          },
        ],
      }),
    );
    expect(useApp.getState().signedOutLogins).toEqual({ "claude-code": true });
    commitPlansAnswer(
      parsePlans({
        plans: [
          {
            id: "claude",
            name: "Claude",
            harness: "claude-code",
            status: "ok",
            windows: [{ label: "5-hour", usedPercent: 4, windowMinutes: 300 }],
          },
        ],
      }),
    );
    // Signed back in: nothing is signed out any more.
    expect(useApp.getState().signedOutLogins).toEqual({});
    resetForTests();
  });
});

describe("an error's words", () => {
  test("the engine's reason in data wins over its generic name", () => {
    // Worktree creation: `Error::invalid_params().data({ reason, message })`.
    const error = new RpcError(-32602, "Invalid params", {
      reason: "not_git",
      message: "/Users/me/code is not in a git repository",
    });
    expect(errorText(error)).toBe("/Users/me/code is not in a git repository");
  });

  test("a data string is the reason too", () => {
    expect(errorText(new RpcError(-32602, "Invalid params", "invalid session id"))).toBe(
      "invalid session id",
    );
    expect(errorText(new RpcError(-32602, "Invalid params", { error: { message: "no such thread" } }))).toBe(
      "no such thread",
    );
  });

  test("without a reason it is the message it always was", () => {
    expect(errorText(new RpcError(-32603, "The engine stopped"))).toBe("The engine stopped");
    expect(errorText(new RpcError(-32602, "Invalid params", { reason: "not_git" }))).toBe("Invalid params");
    expect(errorText(new RpcError(-32602, "Invalid params", "   "))).toBe("Invalid params");
    expect(errorText(new Error("plain"))).toBe("plain");
    expect(errorText("text")).toBe("text");
  });
});

describe("continuing after a limit", () => {
  test("dropping a wait and turning the setting off clear it", async () => {
    const { continueDueThreads, dropLimitWait, setContinueAfterLimit } = await import("./store");
    const wait = { until: 1, harness: "claude-code", account: null, attempts: 1 };
    useApp.setState({ limitWaits: { gone: wait } });
    dropLimitWait("gone");
    expect(useApp.getState().limitWaits).toEqual({});
    useApp.setState({ limitWaits: { a: wait } });
    setContinueAfterLimit(false);
    expect(useApp.getState().limitWaits).toEqual({});
    setContinueAfterLimit(true);
    await continueDueThreads();
  });
});

describe("a delegate whose turn failed", () => {
  const ready = async () => {
    const transport = new HeldSessionLoadTransport({ delayMs: 0 });
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const caller = useApp.getState().sessions[0]!.id;
    setPermissionMode(caller, "full");
    const target = useApp
      .getState()
      .sessions.find((x) => x.id !== caller && x.cwd === "/Users/you/code/rustcode")!.id;
    await openThread(target);
    return { caller, target, transport };
  };
  const send = (caller: string, target: string, message: string) =>
    answerOrchestration({
      sessionId: caller,
      tool: "message_session",
      args: { session_id: target, message },
    });

  test("an interrupted delegate runs the next message as a new turn", async () => {
    const { caller, target, transport } = await ready();
    const first = runPrompt(target, "keep working");
    await until(() =>
      useApp
        .getState()
        .threads[target]!.items.some((item) => item.kind === "tool" && item.id.startsWith("live-")),
    );
    const interrupted = await answerOrchestration({
      sessionId: caller,
      tool: "interrupt_session",
      args: { session_id: target },
    });
    expect(interrupted.interrupted).toBe(true);
    await first;
    expect(
      useApp.getState().threads[target]!.items.findLast((item) => item.kind === "turnEnd"),
    ).toMatchObject({
      outcome: "cancelled",
    });
    const answer = await send(caller, target, "continue after interrupt");
    expect(answer.error).toBeUndefined();
    expect(answer.receipt).toBe("started");
    await until(() =>
      useApp
        .getState()
        .threads[target]!.items.some(
          (item) => item.kind === "user" && item.text.includes("continue after interrupt"),
        ),
    );
    await until(
      () =>
        transport.requests.filter(
          (request) =>
            request.method === Method.sessionPrompt &&
            (request.params as JsonObject | undefined)?.sessionId === target,
        ).length >= 2,
    );
  });

  test("can be messaged again: the failure is no reason to refuse it", async () => {
    const { caller, target } = await ready();
    // Its turn failed (and its harness process may since have been stopped for idling): the thread
    // keeps the error on screen, which used to read as "could not be opened".
    updateThread(target, (t) => ({
      ...t,
      running: false,
      error: "Codex rejected the request (HTTP 400 Bad Request).",
    }));
    const answer = await send(caller, target, "try again with less");
    expect(answer.error).toBeUndefined();
    expect(answer.receipt).toBe("started");
    await until(
      () =>
        useApp
          .getState()
          .threads[target]?.items.some((i) => i.kind === "user" && i.text.includes("try again with less")) ??
        false,
    );
  });

  test("a thread whose open failed is opened again before it is refused", async () => {
    const { caller, target } = await ready();
    updateThread(target, (t) => ({
      ...t,
      openFailed: true,
      error: "Could not open this thread: engine restarting",
    }));
    const answer = await send(caller, target, "are you there");
    expect(answer.error).toBeUndefined();
    expect(useApp.getState().threads[target]?.openFailed).toBeUndefined();
  });

  test("a message waits for one in-flight open and is delivered once", async () => {
    const transport = new HeldSessionLoadTransport();
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const caller = useApp.getState().sessions[0]!.id;
    setPermissionMode(caller, "full");
    const target = useApp
      .getState()
      .sessions.find((x) => x.id !== caller && x.cwd === "/Users/you/code/rustcode")!.id;
    transport.target = target;

    const opening = openThread(target);
    await until(() => transport.heldLoad !== undefined);
    expect(openThread(target)).toBe(opening);
    let messageSettled = false;
    const message = send(caller, target, "deliver once after load").then((answer) => {
      messageSettled = true;
      return answer;
    });
    const matching = (method: string) =>
      transport.requests.filter(
        (request) =>
          request.method === method && (request.params as JsonObject | undefined)?.sessionId === target,
      );

    try {
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(messageSettled).toBe(false);
      expect(matching(Method.sessionPrompt)).toHaveLength(0);
      transport.releaseLoad();
      await opening;
      const answer = await message;
      expect(answer.error).toBeUndefined();
      await until(() => matching(Method.sessionPrompt).length === 1);
      expect(matching(Method.sessionLoad)).toHaveLength(1);
      expect(matching(Method.sessionPrompt)).toHaveLength(1);
      expect(
        useApp
          .getState()
          .threads[target]!.items.filter(
            (item) => item.kind === "user" && item.text.includes("deliver once"),
          ),
      ).toHaveLength(1);
    } finally {
      transport.releaseLoad();
    }
  });

  test("a thread archived while it is opening does not receive a delegate message", async () => {
    const transport = new HeldSessionLoadTransport();
    await connect(transport);
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 1);
    const caller = useApp.getState().sessions[0]!.id;
    setPermissionMode(caller, "full");
    const target = useApp
      .getState()
      .sessions.find((x) => x.id !== caller && x.cwd === "/Users/you/code/rustcode")!.id;
    transport.target = target;

    const message = send(caller, target, "do not reopen after archive");
    await until(() => transport.heldLoad !== undefined);
    setArchived(target, true);
    try {
      transport.releaseLoad();
      const answer = await message;
      expect((answer.error as JsonObject).message).toContain("archived");
      expect(
        transport.requests.filter(
          (request) =>
            request.method === Method.sessionPrompt &&
            (request.params as JsonObject | undefined)?.sessionId === target,
        ),
      ).toHaveLength(0);
    } finally {
      transport.releaseLoad();
    }
  });

  test("archived and settled threads return a clear refusal", async () => {
    const { caller, target } = await ready();
    setArchived(target, true);
    const archived = await send(caller, target, "archived");
    expect((archived.error as JsonObject).message).toContain("unarchive it");

    setArchived(target, false);
    await setThreadStatus(target, "done");
    const settled = await send(caller, target, "settled");
    expect((settled.error as JsonObject).message).toContain("settled");
  });

  test("a stopped thread starts a new turn when messaged", async () => {
    const { caller, target } = await ready();
    updateThread(target, (thread) => ({
      ...thread,
      items: [
        ...thread.items,
        { kind: "turnEnd", id: "user-stopped", at: Date.now(), outcome: "cancelled" },
        { kind: "notice", id: "stop-note", text: "Stopped", tone: "info" },
      ],
    }));
    const stopped = await send(caller, target, "stopped");
    expect(stopped.error).toBeUndefined();
    expect(stopped.receipt).toBe("started");
    await until(() =>
      useApp
        .getState()
        .threads[target]!.items.some((item) => item.kind === "user" && item.text.includes("stopped")),
    );
  });

  test("a message queued on it fails with the reason instead of staying Queued", async () => {
    const { caller, target } = await ready();
    updateThread(target, (t) => ({ ...t, running: true }));
    const answer = await send(caller, target, "after that, this");
    expect(answer.receipt).toBe("queued");
    const id = String(answer.link_id);
    // The user's own queued message stays held for Resume.
    updateThread(target, (t) => ({
      ...t,
      running: false,
      queueHeld: true,
      queue: [...t.queue, { id: "user-1", text: "mine", queuedAt: Date.now() }],
    }));
    threadTurnEnded(target, false, "failed", "Codex rejected the request (HTTP 400 Bad Request).", []);
    const link = useApp.getState().links.links.find((l) => l.id === id)!;
    expect(link.state).toBe("failed");
    expect(link.answer).toContain("Not delivered");
    expect(link.answer).toContain("HTTP 400 Bad Request");
    const queue = useApp.getState().threads[target]!.queue;
    expect(queue.map((q) => q.id)).toEqual(["user-1"]);
    expect(useApp.getState().threads[target]!.queueHeld).toBe(true);
    expect(
      useApp
        .getState()
        .threads[target]!.items.filter(
          (item) => item.kind === "user" && item.text.includes("after that, this"),
        ),
    ).toHaveLength(0);
    // Ending the same failure twice does not enqueue or deliver the message a second time.
    threadTurnEnded(target, false, "failed", "Codex rejected the request (HTTP 400 Bad Request).", []);
    expect(useApp.getState().links.links.find((l) => l.id === id)?.state).toBe("failed");
    expect(useApp.getState().threads[target]!.queue.map((q) => q.id)).toEqual(["user-1"]);
  });

  test("settling it takes its failed turn out of the Inbox", async () => {
    const { target } = await ready();
    markFinished(target, Date.now(), true);
    expect(target in useApp.getState().failed).toBe(true);
    await setThreadStatus(target, "done");
    expect(target in useApp.getState().failed).toBe(false);
  });
});

describe("projects and scheduled work", () => {
  /** A ready app with a caller thread, and a call answered as the engine would answer it. */
  async function ready() {
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const session = useApp.getState().sessions[0]!;
    const call = (tool: string, args: JsonObject) =>
      answerOrchestration({ sessionId: session.id, tool, args });
    return { caller: session.id, folder: session.cwd ?? "", call };
  }

  test("a project reads without asking, and every change is asked first", async () => {
    // The project book lives in app storage, as the app's own settings do.
    const storage = testStorage();
    try {
      const { caller, folder, call } = await ready();
      const read = await call("read_project", { path: folder });
      expect(useApp.getState().grant).toBeNull();
      expect(read.project).toMatchObject({
        path: folder,
        is_git: true,
        remote: "https://github.com/LHagfoss/rustcode.git",
        default_branch: "main",
      });

      const path = "/tmp/rc-project-book";
      const creating = call("create_project", { path });
      await until(() => useApp.getState().grant !== null);
      expect(useApp.getState().grant).toMatchObject({ kind: "project", target: path });
      resolveGrant("once");
      const created = await creating;
      expect((created.project as JsonObject).path).toBe(path);
      expect(projectEntry(path)?.registeredAt).toBeGreaterThan(0);
      // A folder registered before its first thread is listed, with none.
      const listed = await call("list_projects", {});
      const entries = listed.projects as { path: string; threads: number }[];
      expect(entries.find((p) => p.path === path)?.threads).toBe(0);

      const updating = call("update_project", { path, name: "Book", harness: "rustcode" });
      await until(() => useApp.getState().grant !== null);
      expect(useApp.getState().grant?.detail).toContain("Book");
      resolveGrant("once");
      const updated = await updating;
      expect(updated.project).toMatchObject({ path, name: "Book", harness: "rustcode" });
      // A harness the app does not have is refused without a card.
      const unknown = await call("update_project", { path, harness: "nope" });
      expect((unknown.error as { code?: string }).code).toBe("not_found");
      expect(useApp.getState().grant).toBeNull();

      // A refusal changes nothing; the same call allowed only forgets the folder.
      const removing = call("remove_project", { path });
      await until(() => useApp.getState().grant !== null);
      expect(useApp.getState().grant?.detail).toContain("stay on disk");
      resolveGrant("deny");
      expect((await removing).error).toMatchObject({ code: "denied" });
      expect(projectEntry(path)).toBeDefined();
      const again = call("remove_project", { path });
      await until(() => useApp.getState().grant !== null);
      resolveGrant("once");
      expect(await again).toMatchObject({ path, removed: true, files_deleted: false });
      expect(projectEntry(path)).toBeUndefined();

      // Full access answers the card itself.
      setPermissionMode(caller, "full");
      expect((await call("create_project", { path: "/tmp/rc-project-full" })).project).toBeDefined();
      expect(useApp.getState().grant).toBeNull();
    } finally {
      storage.done();
    }
  });

  test("a clone asks, lands in the parent folder, and is registered", async () => {
    const storage = testStorage();
    try {
      const { call } = await ready();
      const cloning = call("clone_project", {
        url: "https://github.com/owner/repo.git",
        parent: "/Users/you/code",
      });
      await until(() => useApp.getState().grant !== null);
      expect(useApp.getState().grant?.kind).toBe("project");
      expect(useApp.getState().grant?.detail).toContain("git clone");
      resolveGrant("once");
      const cloned = await cloning;
      expect(cloned.project).toMatchObject({ path: "/Users/you/code/repo", is_git: true });
      expect(projectEntry("/Users/you/code/repo")?.registeredAt).toBeGreaterThan(0);
    } finally {
      storage.done();
    }
  });

  test("a project's default harness and model start its new threads", async () => {
    const storage = testStorage();
    try {
      const { folder } = await ready();
      updateProjectEntry(folder, { harness: "rustcode", model: "chatgpt/gpt-6-luna" });
      expect(projectThreadChoice(useApp.getState(), folder)).toMatchObject({
        harness: "rustcode",
        model: "chatgpt/gpt-6-luna",
      });
      // The new-thread screen reads the same choice, so the picker shows what ⏎ starts.
      setProjectCwd(folder);
      expect(newThreadChoice(useApp.getState())).toMatchObject({
        harness: "rustcode",
        model: "chatgpt/gpt-6-luna",
      });
      // A pick is the draft and wins; the project's default comes back when it is cleared.
      setNewThreadChoice({ harness: "claude-code", model: null, effort: null });
      expect(newThreadChoice(useApp.getState())?.harness).toBe("claude-code");
      setNewThreadChoice(null);
      expect(newThreadChoice(useApp.getState())?.harness).toBe("rustcode");
      // A model the harness does not list falls back to the one it would start on.
      updateProjectEntry(folder, { model: "nope" });
      expect(projectThreadChoice(useApp.getState(), folder)?.model).not.toBe("nope");
      // A new harness with no model drops the old model: the book never holds a pair that cannot
      // start, and the harness applies its own default.
      updateProjectEntry(folder, { harness: "claude-code" });
      expect(projectEntry(folder)).toMatchObject({ harness: "claude-code" });
      expect(projectEntry(folder)?.model).toBeUndefined();
      updateProjectEntry(folder, { harness: undefined, model: undefined });
      // A harness that cannot run a thread starts one the usual way instead.
      updateProjectEntry("/tmp/rc-project-off", { harness: "claude-code" });
      expect(projectThreadChoice(useApp.getState(), "/tmp/rc-project-off")).toBeNull();
      // A folder with no project default is the app's own default, as before.
      setProjectCwd("/tmp/rc-no-project");
      expect(newThreadChoice(useApp.getState())?.harness).not.toBe("claude-code");
    } finally {
      storage.done();
    }
  });

  test("scheduled work reads freely and asks before it is added, run or deleted", async () => {
    const { call } = await ready();
    const listed = await call("list_scheduled_tasks", {});
    expect(useApp.getState().grant).toBeNull();
    expect((listed.tasks as { id: string }[])[0]?.id).toBe("task-morning");

    const creating = call("schedule_task", {
      prompt: "Check the build",
      schedule: { kind: "cron", expression: "0 9 * * 1-5" },
    });
    await until(() => useApp.getState().grant !== null);
    expect(useApp.getState().grant?.kind).toBe("schedule");
    expect(useApp.getState().grant?.detail).toContain("0 9 * * 1-5");
    resolveGrant("once");
    const created = await creating;
    const task = created.task as { id: string; schedule: { timezone?: string } };
    expect(task.id).toBeTruthy();
    // A cron with no zone is read in the user's own, which the app fills in.
    expect(task.schedule.timezone).toBeTruthy();

    // A card names the task the list reports, not the opaque id the call carried.
    const denied = call("delete_scheduled_task", { id: "task-morning" });
    await until(() => useApp.getState().grant !== null);
    expect(useApp.getState().grant?.detail).toContain("Morning build check");
    resolveGrant("deny");
    expect((await denied).error).toMatchObject({ code: "denied" });

    const running = call("run_scheduled_task_now", { id: "task-morning" });
    await until(() => useApp.getState().grant !== null);
    expect(useApp.getState().grant?.detail).toContain("Morning build check");
    resolveGrant("once");
    expect(await running).toMatchObject({ id: "task-morning", state: "claimed" });
  });
});
