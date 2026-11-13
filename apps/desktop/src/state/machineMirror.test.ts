import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import {
  loadMirroredList,
  loadMirroredTranscript,
  mirrorThreadList,
  mirrorTranscript,
  readMirrorStatus,
  resetMirrorForTests,
  setListDelayForTests,
  setMirrorRequest,
  setMirrorSettings,
  transcriptToMirror,
  withMirroredRows,
} from "./machineMirror";
import { emptyThread } from "./thread";

const row = (id: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  id,
  cwd: "/work/app",
  updatedAt: 1,
  ...extra,
});

let calls: Record<string, unknown>[] = [];

function engine(answer: (params: Record<string, unknown>) => unknown) {
  calls = [];
  setMirrorRequest(async (params) => {
    calls.push(params);
    return answer(params);
  });
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  resetMirrorForTests();
  setListDelayForTests(30);
});
afterEach(() => resetMirrorForTests());

describe("mirroring what a machine listed", () => {
  test("waits out the throttle and sends the newest list once", async () => {
    engine(() => ({ stored: 1 }));
    mirrorThreadList("laptop", [row("a")]);
    mirrorThreadList("laptop", [row("a"), row("b")]);
    expect(calls).toEqual([]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(calls.length).toBe(1);
    expect(calls[0]).toMatchObject({ action: "putList", host: "laptop" });
    expect((calls[0]!.sessions as SessionSummary[]).map((s) => s.id)).toEqual(["a", "b"]);
  });

  test("leaves out what is only true while the machine answers", async () => {
    engine(() => ({}));
    mirrorThreadList("laptop", [
      row("a", { status: "running" as never, host: "laptop" } as never),
      row("b", { status: "done" as never }),
    ]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    const sent = calls[0]!.sessions as Record<string, unknown>[];
    expect(sent[0]).not.toHaveProperty("status");
    expect(sent[0]).not.toHaveProperty("host");
    expect(sent[1]!.status).toBe("done");
  });

  test("sends nothing for this computer, an empty list or without an engine", () => {
    engine(() => ({}));
    mirrorThreadList("", [row("a")]);
    mirrorThreadList("laptop", []);
    setMirrorRequest(null);
    mirrorThreadList("laptop", [row("a")]);
    expect(calls).toEqual([]);
  });

  test("stops asking once the engine says the switch is off", async () => {
    engine(() => {
      throw new Error("thread mirroring is off");
    });
    const thread = {
      ...emptyThread("laptop/a", "/work/app"),
      items: [{ kind: "agent", id: "1", text: "hi" }],
    };
    mirrorTranscript(thread as never, "laptop/a");
    await flush();
    expect(calls.length).toBe(1);
    mirrorTranscript(thread as never, "laptop/a");
    await flush();
    expect(calls.length).toBe(1);
  });

  test("a transcript goes with its machine, folder and read time, without attachment bytes", async () => {
    engine(() => ({}));
    const thread = {
      ...emptyThread("laptop/a", "/work/app", "Fix it"),
      items: [{ kind: "user", id: "u", text: "hi", attachments: [{ name: "x.png" }] }],
    };
    mirrorTranscript(thread as never, "laptop/a", 123);
    await flush();
    expect(calls[0]).toMatchObject({
      action: "putTranscript",
      host: "laptop",
      id: "a",
      cwd: "/work/app",
      revision: 123,
    });
    const items = (calls[0]!.transcript as { items: Record<string, unknown>[] }).items;
    expect(items[0]).not.toHaveProperty("attachments");
  });

  test("an empty thread and a local thread are not mirrored", async () => {
    engine(() => ({}));
    mirrorTranscript(emptyThread("laptop/a", "/p") as never, "laptop/a");
    mirrorTranscript(
      { ...emptyThread("a", "/p"), items: [{ kind: "agent", id: "1", text: "x" }] } as never,
      "a",
    );
    await flush();
    expect(calls).toEqual([]);
    expect(transcriptToMirror(emptyThread("laptop/a", "/p") as never)).toBeNull();
  });
});

describe("reading the mirror", () => {
  test("lists a machine's rows, newest first as the engine sent them", async () => {
    engine(() => ({
      enabled: true,
      hosts: { laptop: { threads: [{ session: row("b") }, { session: row("a") }, { session: 3 }] } },
    }));
    const rows = await loadMirroredList("laptop");
    expect(rows.map((r) => r.id)).toEqual(["b", "a"]);
    expect(calls[0]).toEqual({ action: "list", host: "laptop" });
  });

  test("answers an empty list when the engine cannot", async () => {
    engine(() => {
      throw new Error("unknown method");
    });
    expect(await loadMirroredList("laptop")).toEqual([]);
    expect(await loadMirroredTranscript("laptop/a")).toBeNull();
  });

  test("turns a mirrored transcript back into what the store keeps", async () => {
    engine(() => ({
      found: true,
      savedAt: 99,
      transcript: { cwd: "/work/app", title: "Fix it", items: [{ kind: "agent", id: "1", text: "hi" }] },
    }));
    const cached = await loadMirroredTranscript("laptop/a");
    expect(cached).toMatchObject({ key: "laptop/a", cwd: "/work/app", title: "Fix it", savedAt: 99 });
    expect(cached?.items.length).toBe(1);
    expect(calls[0]).toEqual({ action: "transcript", host: "laptop", id: "a" });
  });

  test("never reads a local thread from the mirror", async () => {
    engine(() => ({}));
    expect(await loadMirroredTranscript("a")).toBeNull();
    expect(calls).toEqual([]);
  });
});

describe("what a machine that is away draws", () => {
  test("keeps its own rows and adds only the mirrored ones it lacks", () => {
    const merged = withMirroredRows([row("a", { title: "live" })], [row("a", { title: "old" }), row("b")]);
    expect(merged.map((r) => [r.id, r.title])).toEqual([
      ["a", "live"],
      ["b", undefined],
    ]);
  });

  test("returns the same rows when the mirror has nothing new", () => {
    const current = [row("a")];
    expect(withMirroredRows(current, [row("a")])).toEqual(current);
  });
});

describe("the switch", () => {
  test("reads the status and applies a change", async () => {
    engine((params) =>
      params.action === "status"
        ? {
            enabled: true,
            excludedProjects: [],
            hosts: { laptop: { threads: 3, transcripts: 1, bytes: 10 } },
          }
        : { enabled: false, excludedProjects: ["/p"], hosts: {} },
    );
    expect((await readMirrorStatus())?.hosts.laptop).toEqual({ threads: 3, transcripts: 1, bytes: 10 });
    const next = await setMirrorSettings({ enabled: false, excludedProjects: ["/p"] });
    expect(next).toEqual({ enabled: false, excludedProjects: ["/p"], hosts: {} });
    expect(calls[1]).toEqual({ action: "setSettings", enabled: false, excludedProjects: ["/p"] });
    // Off means the throttled list is not even sent.
    mirrorThreadList("laptop", [row("a")]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(calls.length).toBe(2);
  });

  test("a status the engine cannot give is null", async () => {
    engine(() => "nope");
    expect(await readMirrorStatus()).toBeNull();
  });
});
