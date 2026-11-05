import { beforeEach, describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import { MockTransport } from "../acp/mock";
import { harnessPlanKey, parsePlans } from "../acp/rustcode";
import { emptyExtras } from "./compose";
import { answerOrchestration, resolveGrant } from "./host";
import {
  agentHistory,
  answerPermission,
  answerQuestionOptions,
  type CachedPlan,
  cancel,
  cancelQueuedEdit,
  closeReport,
  commandsFor,
  commitPlansAnswer,
  completeOnboarding,
  connect,
  editMessage,
  ensureHarnessModels,
  errorText,
  fetchHarnessModels,
  forgetPlans,
  mergePlans,
  moveThread,
  onlyCachedPlan,
  PLAN_CACHE_MAX_AGE_MS,
  PLAN_CACHE_POLL_MS,
  permissionModeOf,
  pickNewThreadChoice,
  planCacheStale,
  planIsStale,
  planNow,
  plansToCache,
  prunePlans,
  refreshAgents,
  refreshHarnesses,
  refreshSessions,
  removeQueued,
  renameThread,
  resetForTests,
  resumeQueue,
  saveQueuedEdit,
  selectThread,
  sendQueuedNow,
  setActiveTheme,
  setArchived,
  setDefaultCwd,
  setDefaultHarness,
  setDefaultPermissionMode,
  setDraft,
  setHarnessEnabled,
  setNewThreadChoice,
  setOrchestration,
  setPermissionMode,
  setProjectCwd,
  setThreadStatus,
  shownPlan,
  startNewThread,
  startQueuedEdit,
  statusOf,
  steerOrSubmit,
  submit,
  switchNewThreadHarness,
  togglePin,
  trashThread,
  updateQueued,
  updateThread,
  useApp,
  windowNow,
} from "./store";
import { turnCompletion } from "./thread";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

const selected = () => {
  const s = useApp.getState();
  return s.selectedId ? s.threads[s.selectedId] : undefined;
};

beforeEach(() => resetForTests());

describe("engine controller with the mock engine", () => {
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
    // ⌘⏎ with images: a steer takes text only, so the whole prompt queues, in order.
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
    // Sending one with images now is refused, and it stays queued with them.
    await sendQueuedNow(id, queue[0]!.id);
    expect(useApp.getState().threads[id]!.queue.length).toBe(3);
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
  });

  test("a thread moves to another folder, and the default folder is separate", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0);
    const id = useApp.getState().sessions[0]!.id;
    selectThread(id);
    await until(() => selected()?.loaded === true);
    expect(await moveThread(id, "/tmp/elsewhere")).toBe(true);
    expect(useApp.getState().sessions.find((x) => x.id === id)?.cwd).toBe("/tmp/elsewhere");
    await until(() => selected()?.loaded === true);
    expect(selected()!.cwd).toBe("/tmp/elsewhere");

    setDefaultCwd("/tmp/default");
    startNewThread();
    // A new thread from an open one starts in that thread's folder; picking another keeps the default.
    expect(useApp.getState().projectCwd).toBe("/tmp/elsewhere");
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
    await until(() => useApp.getState().sessions.length === 4);
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
    expect(names).toEqual(["rustcode", "light", "graphite"]);
    await setActiveTheme("light");
    expect(useApp.getState().themeActive).toBe("light");
    expect(useApp.getState().theme).toBe("light");
    // The mock announces `_rustcode/themes/changed` after a set, as the engine does for a CLI change.
    await setActiveTheme("graphite");
    await until(() => useApp.getState().themes?.active === "graphite");
    expect(useApp.getState().theme).toBe("dark");
  });

  test("pins live in the engine when it has them; Settled is threads marked Done", async () => {
    await connect(new MockTransport({ delayMs: 0 }));
    await until(() => useApp.getState().features.pins === true && useApp.getState().sessions.length === 4);
    const id = useApp.getState().sessions[2]!.id;
    togglePin(id);
    await new Promise((r) => setTimeout(r, 10));
    await refreshSessions();
    expect(useApp.getState().sessions.find((x) => x.id === id)?.pinned).toBe(true);
    await setThreadStatus(id, "done");
    expect(statusOf(useApp.getState(), id)).toBe("done");
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

describe("toasts", () => {
  test("errors time out too, a repeat restarts the shown one, an id replaces in place", async () => {
    const { toast, dismissToast } = await import("./store");
    toast("Copied", "success");
    toast("Copied", "success");
    expect(useApp.getState().toasts.length).toBe(1);
    expect(useApp.getState().toasts[0]!.persist).toBe(false);
    toast("Could not save", "error");
    expect(useApp.getState().toasts.at(-1)!.persist).toBe(false);
    // A burst keeps only the newest five.
    for (let i = 0; i < 50; i++) toast(`Failed ${i}`, "error");
    expect(useApp.getState().toasts.length).toBe(5);
    expect(useApp.getState().toasts.at(-1)!.text).toBe("Failed 49");
    toast("Starting engine…", "info", { id: "engine", persist: true, progress: 0.2 });
    toast("The engine stopped.", "error", { id: "engine", action: { label: "Restart", run: () => {} } });
    const engine = useApp.getState().toasts.filter((t) => t.id === "engine");
    expect(engine.length).toBe(1);
    expect(engine[0]!.action?.label).toBe("Restart");
    // Replacing a toast that is fading out keeps the new one.
    dismissToast("engine");
    toast("The engine restarted", "success", { id: "engine" });
    await new Promise((r) => setTimeout(r, 160));
    expect(useApp.getState().toasts.find((t) => t.id === "engine")?.text).toBe("The engine restarted");
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

describe("plan usage cache", () => {
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

  test("staleness follows the three-minute poll", () => {
    const now = 10_000_000;
    expect(PLAN_CACHE_POLL_MS).toBe(180_000);
    expect(planIsStale(undefined, now)).toBe(true);
    expect(planIsStale(plan(20, now - PLAN_CACHE_POLL_MS + 1), now)).toBe(false);
    expect(planIsStale(plan(20, now - PLAN_CACHE_POLL_MS), now)).toBe(true);
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
    expect(cache["claude-code"]).toEqual({
      source: "claude-code",
      windows: [{ usedPercent: 18, minutes: 300, resetsAt: 1_000_000 }],
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
