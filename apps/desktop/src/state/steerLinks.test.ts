// A message steered into a delegate's running turn (`message_session` with `steer: true`) has no
// turn of its own to settle its link, so the running turn does when it ends (issue 3532): the
// sidebar and the parent's delegate row stop saying "working" without the thread being opened.

import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { answerOrchestration } from "./host";
import {
  connect,
  openThread,
  resetForTests,
  selectThread,
  setOrchestration,
  setPermissionMode,
  submit,
  useApp,
} from "./store";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

beforeEach(() => resetForTests());

describe("a delegate steered mid-turn", () => {
  test("two steers, then the turn ends: both links are done with the delegate closed", async () => {
    // A turn that lasts long enough for two steers to land in it and for the poll to see it run.
    setOrchestration(true);
    await connect(new MockTransport({ delayMs: 15 }));
    await until(() => useApp.getState().status === "ready" && useApp.getState().sessions.length > 0, 10_000);
    const caller = useApp.getState().sessions[0]!.id;
    setPermissionMode(caller, "full");
    const target = useApp.getState().sessions.find((x) => x.cwd === "/Users/you/code/website")!.id;
    await openThread(target);
    selectThread(target);
    const turning = submit("do the thing");
    await until(() => useApp.getState().threads[target]?.running === true);
    const steers: string[] = [];
    for (const message of ["first change", "second change"]) {
      const result = await answerOrchestration({
        sessionId: caller,
        tool: "message_session",
        args: { session_id: target, message, steer: true },
      });
      expect(result.delivered).toBe("steered");
      steers.push(result.link_id as string);
    }
    // The delegate is not the open thread when its turn ends.
    selectThread(caller);
    const linkStates = () =>
      steers.map((id) => useApp.getState().links.links.find((l) => l.id === id)?.state);
    expect(linkStates()).toEqual(["working", "working"]);
    await turning;
    // Settled by the turn that ended, whatever the thread does after it.
    await until(() => linkStates().every((state) => state === "done"), 10_000);
    expect(linkStates()).toEqual(["done", "done"]);
    // The parent has read both answers: no wake turn starts after the test, in the next one's threads.
    useApp.setState((st) => ({
      links: {
        ...st.links,
        links: st.links.links.map((l) => (steers.includes(l.id) ? { ...l, notifiedAt: Date.now() } : l)),
      },
    }));
  });
});
