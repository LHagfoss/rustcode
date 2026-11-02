import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import {
  agentHistory,
  answerPermission,
  answerQuestionOptions,
  cancel,
  completeOnboarding,
  connect,
  editMessage,
  fetchHarnessModels,
  refreshAgents,
  renameThread,
  resetForTests,
  selectThread,
  setArchived,
  setDefaultHarness,
  setHarnessEnabled,
  setNewThreadChoice,
  setProjectCwd,
  submit,
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
    expect(kinds).toEqual(["user", "thought", "tool", "tool", "assistant", "turnEnd"]);
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
    await until(() => useApp.getState().sessions.length === 3);
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
});
