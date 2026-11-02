import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { resolveGrant } from "./host";
import {
  agentHistory,
  answerPermission,
  answerQuestionOptions,
  cancel,
  completeOnboarding,
  connect,
  editMessage,
  ensureHarnessModels,
  fetchHarnessModels,
  pickNewThreadChoice,
  refreshAgents,
  refreshHarnesses,
  refreshSessions,
  renameThread,
  resetForTests,
  selectThread,
  setActiveTheme,
  setArchived,
  setDefaultHarness,
  setHarnessEnabled,
  setNewThreadChoice,
  setOrchestration,
  setProjectCwd,
  setThreadStatus,
  statusOf,
  submit,
  switchNewThreadHarness,
  togglePin,
  trashThread,
  useApp,
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
  test("errors persist, a repeat restarts the shown one, an id replaces in place", async () => {
    const { toast, dismissToast } = await import("./store");
    toast("Copied", "success");
    toast("Copied", "success");
    expect(useApp.getState().toasts.length).toBe(1);
    expect(useApp.getState().toasts[0]!.persist).toBe(false);
    toast("Could not save", "error");
    expect(useApp.getState().toasts.at(-1)!.persist).toBe(true);
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
