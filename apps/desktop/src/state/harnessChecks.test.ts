import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import type { TransportEvent } from "../acp/transport";
import type { CheckResult } from "./harnessCheck";
import { runAllHarnessChecks, runHarnessCheck, useHarnessChecks } from "./harnessChecks";
import {
  addCustomHarness,
  connect,
  discardCustomHarness,
  enableCustomHarness,
  refreshHarnesses,
  removeHarnessAccount,
  resetForTests,
  useApp,
} from "./store";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

const find = (id: string) => useApp.getState().harnesses?.harnesses.find((h) => h.id === id);

async function ready() {
  await connect(new MockTransport({ delayMs: 0, scenario: "onboarding" }));
  await until(() => useApp.getState().harnesses !== null);
}

/** The mock, with a record of the requests and a way to hold an answer back. */
class SpyTransport extends MockTransport {
  readonly requests: Array<{ method: string; params: Record<string, unknown> }> = [];
  /** Model listings started and not answered yet, and the most at once. */
  private listing = 0;
  maxListing = 0;
  private held: Array<() => void> = [];
  private holdIds = new Set<number>();
  private methodOf = new Map<number, string>();
  holdNextSnapshot = false;

  async start(onEvent: (event: TransportEvent) => void) {
    await super.start((event) => {
      const id = event.type === "line" ? (JSON.parse(event.line) as { id?: number }).id : undefined;
      if (typeof id === "number" && this.methodOf.get(id) === "_rustcode/harnesses/models") this.listing--;
      if (typeof id === "number" && this.holdIds.delete(id)) this.held.push(() => onEvent(event));
      else onEvent(event);
    });
  }

  send(line: string) {
    const frame = JSON.parse(line) as { id?: number; method?: string; params?: Record<string, unknown> };
    if (frame.method && typeof frame.id === "number") {
      this.requests.push({ method: frame.method, params: frame.params ?? {} });
      this.methodOf.set(frame.id, frame.method);
      if (frame.method === "_rustcode/harnesses/models")
        this.maxListing = Math.max(this.maxListing, ++this.listing);
      if (frame.method === "_rustcode/harnesses" && this.holdNextSnapshot) {
        this.holdNextSnapshot = false;
        this.holdIds.add(frame.id);
      }
    }
    super.send(line);
  }

  count(method: string) {
    return this.requests.filter((r) => r.method === method).length;
  }

  releaseHeld() {
    for (const release of this.held.splice(0)) release();
  }
}

beforeEach(() => {
  resetForTests();
  useHarnessChecks.setState({ byHarness: {}, all: null });
});

describe("Check", () => {
  test("a harness that runs ends Ready with its models", async () => {
    await ready();
    const result = await runHarnessCheck("codex");
    expect(result.state).toBe("ready");
    expect(result.text).toMatch(/^Ready/);
    expect(find("codex")?.models.length).toBeGreaterThan(0);
    expect(useHarnessChecks.getState().byHarness.codex).toEqual({ result });
  });

  test("a signed-out harness ends Needs sign-in instead of an error", async () => {
    await ready();
    // Its one signed-in account goes, which leaves the harness's own sign-in: missing.
    await removeHarnessAccount("claude-code", "paral-2");
    expect(await runHarnessCheck("claude-code")).toEqual({ state: "needs_login", text: "Needs sign-in" });
  });

  test("pressing Check twice runs one check", async () => {
    await ready();
    expect(runHarnessCheck("codex")).toBe(runHarnessCheck("codex"));
    await until(() => !("running" in (useHarnessChecks.getState().byHarness.codex ?? {})));
  });

  test("an unknown harness ends in an error with text, not in limbo", async () => {
    await ready();
    const result = await runHarnessCheck("nope");
    expect(result.state).toBe("error");
    expect(result.text).not.toBe("");
    expect("running" in (useHarnessChecks.getState().byHarness.nope ?? {})).toBe(false);
  });
});

describe("a custom ACP harness", () => {
  const add = (command: string[], env: Record<string, string> = {}) =>
    addCustomHarness({ id: "dsh", name: "Dsh", command, env });

  test("is added off, verified by a Check, then turned on", async () => {
    await ready();
    await add(["dsh", "--profile", "acp"], { DSH_PROFILE: "work" });
    expect(find("dsh")).toMatchObject({
      known: false,
      enabled: false,
      command: ["dsh", "--profile", "acp"],
      env_names: ["DSH_PROFILE"],
    });
    expect((await runHarnessCheck("dsh")).state).toBe("ready");
    await enableCustomHarness("dsh");
    expect(find("dsh")?.enabled).toBe(true);
    expect(find("dsh")?.models.length).toBeGreaterThan(0);
  });

  test("a command that is not found ends Not installed, naming the program", async () => {
    await ready();
    await add(["missing"]);
    const result = await runHarnessCheck("dsh");
    expect(result).toEqual({ state: "not_installed", text: "Not installed: missing was not found on PATH" });
  });

  test("a command that never answers ACP ends in the error text and can be taken out again", async () => {
    await ready();
    await add(["broken"]);
    const result = await runHarnessCheck("dsh");
    expect(result.state).toBe("error");
    expect(result.text).toMatch(/handshake/);
    await discardCustomHarness("dsh");
    expect(find("dsh")).toBeUndefined();
  });
});

describe("Check all", () => {
  const nonBuiltin = () => (useApp.getState().harnesses?.harnesses ?? []).filter((h) => !h.builtin);

  test("ends every card as checking each one alone does", async () => {
    await ready();
    const all = await runAllHarnessChecks();
    const together = Object.fromEntries(
      Object.entries(useHarnessChecks.getState().byHarness).map(([id, entry]) => [id, entry]),
    );
    expect(all.length).toBe(nonBuiltin().length);
    expect(useHarnessChecks.getState().all).toBeNull();

    useHarnessChecks.setState({ byHarness: {}, all: null });
    const alone: typeof together = {};
    for (const h of nonBuiltin()) alone[h.id] = { result: await runHarnessCheck(h.id) };
    expect(together).toEqual(alone);
  });

  test("detects all in one request, lists models two at a time", async () => {
    const transport = new SpyTransport({ delayMs: 0, scenario: "onboarding" });
    await connect(transport);
    await until(() => useApp.getState().harnesses !== null);
    const before = transport.count("_rustcode/harnesses/accounts/check");
    await runAllHarnessChecks();
    const checks = transport.requests
      .filter((r) => r.method === "_rustcode/harnesses/accounts/check")
      .slice(before);
    expect(checks.length).toBe(1);
    expect([...((checks[0]?.params.redetect ?? []) as string[])].sort()).toEqual(
      nonBuiltin()
        .map((h) => h.id)
        .sort(),
    );
    expect(transport.maxListing).toBeGreaterThan(0);
    expect(transport.maxListing).toBeLessThanOrEqual(2);
  });

  test("joins a harness that is already being checked", async () => {
    await ready();
    const alone = runHarnessCheck("codex");
    const all = runAllHarnessChecks();
    expect((await all).length).toBe(nonBuiltin().length);
    const results = await all;
    expect(await alone).toEqual(results[nonBuiltin().findIndex((h) => h.id === "codex")] as CheckResult);
  });
});

describe("a late snapshot", () => {
  test("does not replace a newer one", async () => {
    const transport = new SpyTransport({ delayMs: 0, scenario: "onboarding" });
    await connect(transport);
    await until(() => useApp.getState().harnesses !== null);
    // A refresh asked for first, answered last, with the signed-in account still in it.
    transport.holdNextSnapshot = true;
    const stale = refreshHarnesses();
    await removeHarnessAccount("claude-code", "paral-2");
    const fresh = useApp.getState().harnesses;
    expect(find("claude-code")?.accounts?.some((a) => a.id === "paral-2")).toBe(false);
    transport.releaseHeld();
    await stale;
    expect(useApp.getState().harnesses).toBe(fresh);
    expect(find("claude-code")?.accounts?.some((a) => a.id === "paral-2")).toBe(false);
  });
});
