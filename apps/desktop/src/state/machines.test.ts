// Settings › Machines without a browser: the labels a row shows, the
// eight-digit code field, and the states `machines_list`, `machines_ask` and
// `machines_pair` put the page in. The shell is a stand-in that answers the
// way `src-tauri/src/machines.rs` and the mock do.

import { beforeEach, describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import {
  askMachine,
  cancelPairing,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  lastSeen,
  type Machine,
  machineLine,
  PAIR_DEADLINE_MS,
  pairMachine,
  platformName,
  refreshMachines,
  routeLabel,
  stateLabel,
  stateTone,
  threadCount,
  threadLabel,
  useMachines,
} from "./machines";

const MACHINE: Machine = {
  id: "gw-7c1d9e4f2a8b6d30",
  name: "Lars's Linux box",
  platform: "linux",
  main: false,
  routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
  route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
  state: "connected",
  detail: null,
  hint: null,
  retryInSecs: null,
  addedAt: "2026-11-02T09:00:00+00:00",
  lastConnectedAt: "2026-11-08T09:00:00+00:00",
};

/** A shell that answers what the app's own shell answers, without a machine. */
class FakeShell implements Transport {
  readonly name = "mock";
  calls: { command: string; args: Record<string, unknown> }[] = [];
  machines: Machine[] = [{ ...MACHINE }];
  /** What `machines_ask` answers with, or an error. */
  ask: { requestId: string; codeLength: number; expiresInSecs: number } | Error = {
    requestId: "8f1c2d3e4a5b6c7d",
    codeLength: 8,
    expiresInSecs: 120,
  };
  pairError: Error | null = null;
  /** `machines_ask` waits for `finishAsk`, as the shell does while the owner decides. */
  askHangs = false;
  private pendingAsk: ((value: unknown) => void) | null = null;

  finishAsk(value: unknown): void {
    this.pendingAsk?.(value);
    this.pendingAsk = null;
  }

  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}

  async call(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
    this.calls.push({ command, args });
    switch (command) {
      case "machines_list":
        return this.machines;
      case "machine_connect":
        return { connected: true };
      case "machines_ask":
        if (this.ask instanceof Error) throw new Error(this.ask.message);
        if (this.askHangs) return await new Promise((resolve) => (this.pendingAsk = resolve));
        return this.ask;
      case "machines_pair": {
        if (this.pairError) throw new Error(this.pairError.message);
        const paired: Machine = { ...MACHINE, state: "connecting", main: false };
        this.machines = [paired];
        return paired;
      }
      case "machines_remove":
        this.machines = this.machines.filter((machine) => machine.id !== args.id);
        return null;
      case "machines_set_main":
        this.machines = this.machines.map((machine) => ({ ...machine, main: machine.id === args.id }));
        return null;
      default:
        throw new Error(`The fake shell does not run ${command}.`);
    }
  }

  command(name: string): Record<string, unknown> | undefined {
    return this.calls.find((call) => call.command === name)?.args;
  }
}

/** A transport with nowhere to send commands, as `dev:web` is for machines. */
class NoShell implements Transport {
  readonly name = "websocket";
  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}
  async call(): Promise<unknown> {
    throw new Error("Only the app can do this.");
  }
}

let shell: FakeShell;

beforeEach(() => {
  shell = new FakeShell();
  setShellTransport(shell);
  useMachines.setState({
    ready: false,
    error: null,
    machines: [],
    counts: {},
    discovered: null,
    discovering: false,
    discoverError: null,
    pairing: null,
    pairingError: null,
  });
});

/** Wait for a state the page only learns after a round trip. */
async function waitFor(check: () => boolean, ms = 1500): Promise<void> {
  const until = Date.now() + ms;
  while (!check() && Date.now() < until) await Bun.sleep(5);
  if (!check()) throw new Error("the state never arrived");
}

describe("machines page state", () => {
  test("the list arrives, and a connected machine reports its own thread count", async () => {
    await refreshMachines();
    expect(useMachines.getState().machines.map((machine) => machine.name)).toEqual(["Lars's Linux box"]);
    expect(useMachines.getState().ready).toBe(true);
    // Its ACP is asked over the channel the shell opened: `session/list`, three threads.
    await waitFor(() => useMachines.getState().counts[MACHINE.id] === 3);
    expect(threadCount(useMachines.getState().machines[0]!, useMachines.getState().counts)).toBe(3);
    expect(shell.command("machine_connect")).toEqual({ id: MACHINE.id });
  });

  test("asking a machine waits for its owner, then asks for the code", async () => {
    expect(useMachines.getState().pairing).toBeNull();
    await askMachine("192.168.1.20:17879", "Studio");
    const asked = shell.command("machines_ask") ?? {};
    expect(asked).toMatchObject({ address: "192.168.1.20:17879", name: "Studio" });
    // The id is the page's, made before the shell was asked.
    expect(String(asked.askId)).toMatch(/^ask-[a-z0-9]+-\d+$/);
    const pairing = useMachines.getState().pairing;
    expect(pairing?.stage).toBe("code");
    if (pairing?.stage !== "code") throw new Error("the code stage never arrived");
    expect(pairing.askId).toBe(asked.askId as string);
    expect(pairing.codeLength).toBe(8);
    expect(pairing.expiresAt).toBeGreaterThan(Date.now());
  });

  test("a machine that refuses the request says why, and leaves nothing waiting", async () => {
    shell.ask = new Error("Studio did not answer the request. Ask again.");
    await askMachine("192.168.1.20:17879", "Studio");
    expect(useMachines.getState().pairing).toBeNull();
    expect(useMachines.getState().pairingError).toContain("did not answer");
    cancelPairing();
    expect(useMachines.getState().pairingError).toBeNull();
  });

  test("the code pairs the machine, and it is listed at once", async () => {
    await askMachine("192.168.1.31:17879", "Lars's Linux box");
    const pairing = useMachines.getState().pairing;
    if (pairing?.stage !== "code") throw new Error("the code stage never arrived");
    await pairMachine("48219034");
    // One id from the first press to the code: the page's own.
    expect(shell.command("machines_pair")).toEqual({ askId: pairing.askId, code: "48219034" });
    expect(useMachines.getState().pairing).toBeNull();
    expect(useMachines.getState().machines[0]?.name).toBe("Lars's Linux box");
  });

  test("a second ask cancels the first one's request", async () => {
    shell.askHangs = true;
    void askMachine("192.168.1.31:17879", "Lars's Linux box");
    const first = useMachines.getState().pairing?.askId;
    shell.finishAsk({ askId: first, codeLength: 8, expiresInSecs: 120 });
    shell.askHangs = false;
    await askMachine("192.168.1.20:17879", "Studio");
    expect(shell.command("machines_cancel")).toEqual({ askId: first });
    expect(useMachines.getState().pairing?.askId).not.toBe(first);
  });

  test("cancelling before the machine answers drops the ask by its own id", async () => {
    shell.askHangs = true;
    const asking = askMachine("192.168.1.31:17879", "Lars's Linux box");
    // The id exists before anything is dialled, so Cancel always has one.
    const askId = useMachines.getState().pairing?.askId;
    expect(askId).toBeString();
    cancelPairing();
    expect(shell.command("machines_cancel")).toEqual({ askId });
    // A late accept does not put the page back into the code stage.
    shell.finishAsk({ askId, codeLength: 8, expiresInSecs: 120 });
    await asking;
    expect(useMachines.getState().pairing).toBeNull();
  });

  test("a spent code is over: the page asks for a new request instead of retyping", async () => {
    shell.pairError = new Error("That connection request is over. Ask the machine again.");
    await askMachine("192.168.1.31:17879", "Lars's Linux box");
    await pairMachine("48219034");
    expect(useMachines.getState().pairing).toBeNull();
    expect(useMachines.getState().pairingError).toContain("request is over");
  });

  test("a failed read is an error on the page, not an empty list", async () => {
    setShellTransport(new NoShell());
    await refreshMachines();
    expect(useMachines.getState().ready).toBe(true);
    expect(useMachines.getState().error).toContain("Only the app can do this.");
  });

  test("checking a code never waits for ever: past the deadline the page says so", async () => {
    const asked = Date.now();
    useMachines.setState({
      pairing: {
        stage: "pairing",
        address: "192.168.1.31:17879",
        host: "Lars's MacBook Pro",
        askId: "ask-test-1",
        since: asked,
      },
      pairingError: null,
    });
    const pairing = useMachines.getState().pairing;
    expect(codeCheckOverdue(pairing, asked + 1000)).toBe(false);
    expect(codeCheckOverdue(pairing, asked + PAIR_DEADLINE_MS - 1)).toBe(false);
    expect(codeCheckOverdue(pairing, asked + PAIR_DEADLINE_MS + 1)).toBe(true);
    // The other stages wait on something else: the owner's answer, or a code yet
    // to be typed, and are bounded by the shell and the code's own lifetime.
    expect(
      codeCheckOverdue(
        { stage: "waiting", address: "a", host: "b", askId: "c", since: asked },
        asked + 10 * PAIR_DEADLINE_MS,
      ),
    ).toBe(false);
    expect(codeCheckOverdue(null, asked)).toBe(false);
  });

  test("the pairing stage is cancelled by its own ask id", async () => {
    await askMachine("192.168.1.31:17879", "Lars's Linux box");
    const pairing = useMachines.getState().pairing;
    if (pairing?.stage !== "code") throw new Error("the code stage never arrived");
    const pairing_ = pairMachine("48219034");
    expect(useMachines.getState().pairing?.stage).toBe("pairing");
    cancelPairing();
    expect(shell.command("machines_cancel")).toEqual({ askId: pairing.askId });
    expect(useMachines.getState().pairing).toBeNull();
    await pairing_;
    // The shell still paired it: the machine is listed, and the page only
    // stopped waiting for the answer.
    expect(useMachines.getState().machines[0]?.name).toBe("Lars's Linux box");
  });
});

describe("machines rows", () => {
  test("a state says where the machine stands, in the words the row uses", () => {
    expect(stateLabel({ state: "connected", retryInSecs: null })).toBe("Connected");
    expect(stateLabel({ state: "connecting", retryInSecs: null })).toBe("Connecting");
    expect(stateLabel({ state: "offline", retryInSecs: 4 })).toBe("Offline · retrying in 4 s");
    expect(stateLabel({ state: "offline", retryInSecs: null })).toBe("Offline");
    expect(stateLabel({ state: "needsPairing", retryInSecs: null })).toBe("Needs pairing");
    expect(stateLabel({ state: "stopped", retryInSecs: null })).toBe("Not connected");
    expect(stateTone({ state: "connected" })).toBe("ok");
    expect(stateTone({ state: "needsPairing" })).toBe("bad");
    expect(stateTone({ state: "offline" })).toBe("warn");
  });

  test("a route says whether the network encrypts it", () => {
    expect(routeLabel({ kind: "lan", address: "192.168.1.31:17879", encrypted: false })).toBe(
      "LAN · Unencrypted",
    );
    expect(routeLabel({ kind: "tailscale", address: "mac.tail1234.ts.net:17879", encrypted: true })).toBe(
      "Tailscale · Encrypted",
    );
    expect(routeLabel({ kind: "vpn", address: "100.92.13.44:17879", encrypted: true })).toBe(
      "VPN · Encrypted",
    );
    expect(routeLabel(null)).toBe("No route saved");
  });

  test("a row under the name says what the connection is doing", () => {
    // The state is the row's chip; the line under the name is the route.
    expect(machineLine(MACHINE)).toBe("LAN · Unencrypted");
    expect(machineLine({ ...MACHINE, state: "connecting" })).toContain("Connecting");
    expect(
      machineLine({
        ...MACHINE,
        state: "offline",
        detail: "Not connected. Retrying in 4 s.",
        retryInSecs: 4,
      }),
    ).toBe("Not connected. Retrying in 4 s.");
    expect(machineLine({ ...MACHINE, state: "needsPairing", detail: null })).toContain("Pair it again");
  });

  test("platforms read as people say them, and unknown ones say nothing", () => {
    expect(platformName("macos")).toBe("Mac");
    expect(platformName("windows")).toBe("Windows");
    expect(platformName("linux")).toBe("Linux");
    expect(platformName("unknown")).toBeUndefined();
    expect(platformName("")).toBeUndefined();
  });

  test("last seen stays vague until it is a date", () => {
    const now = Date.parse("2026-11-08T12:00:00Z");
    expect(lastSeen(null, now)).toBe("Never");
    expect(lastSeen("2026-11-08T11:59:30Z", now)).toBe("just now");
    expect(lastSeen("2026-11-08T11:57:00Z", now)).toBe("3 min ago");
    expect(lastSeen("2026-11-08T10:00:00Z", now)).toBe("2 hours ago");
    expect(lastSeen("2026-11-06T10:00:00Z", now)).toBe("2 days ago");
    expect(lastSeen("2026-01-02T10:00:00Z", now)).not.toContain("ago");
  });

  test("a thread count is only shown while the machine answers", () => {
    expect(threadCount(MACHINE, { [MACHINE.id]: 3 })).toBe(3);
    expect(threadCount({ ...MACHINE, state: "offline" }, { [MACHINE.id]: 3 })).toBeUndefined();
    expect(threadLabel(3)).toBe("3 threads");
    expect(threadLabel(1)).toBe("1 thread");
    expect(threadLabel(undefined)).toBeUndefined();
  });
});

describe("the eight-digit code", () => {
  test("only digits are kept, and never more than the machine's code", () => {
    expect(codeDigits("48219034", 8)).toBe("48219034");
    expect(codeDigits("4821-9034", 8)).toBe("48219034");
    expect(codeDigits(" 4821 9034 ", 8)).toBe("48219034");
    expect(codeDigits("4821903412", 8)).toBe("48219034");
    expect(codeDigits("abc", 8)).toBe("");
  });

  test("a code is complete only when every digit is in", () => {
    expect(codeComplete("48219034", 8)).toBe(true);
    expect(codeComplete("4821903", 8)).toBe(false);
    expect(codeComplete("482190341", 8)).toBe(false);
    expect(codeComplete("", 8)).toBe(false);
  });
});
