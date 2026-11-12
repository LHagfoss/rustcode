// Settings › Machines without a browser: the labels a row shows, the
// eight-digit code field, and the states `machines_list`, `machines_ask` and
// `machines_pair` put the page in. The shell is a stand-in that answers the
// way `src-tauri/src/machines.rs` and the mock do.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import {
  askMachine,
  cancelPairing,
  cleanMachineName,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  DEV_APP_VERSION,
  defaultSelfName,
  discoverMachines,
  forgetLocalAppVersion,
  type GatewayService,
  gatewayLine,
  installServer,
  lastSeen,
  linkCliToApp,
  localAppVersion,
  localIdentity,
  type Machine,
  machineGlyph,
  machineLine,
  machineName,
  machineTint,
  mergeDiscovered,
  PAIR_DEADLINE_MS,
  pairMachine,
  platformIcon,
  platformName,
  probeLabel,
  probeTone,
  readUntilAnswering,
  refreshMachines,
  refreshServerService,
  resetMachinesForTests,
  routeLabel,
  routeName,
  START_RETRY_WAITS_MS,
  selfName,
  serverLine,
  setLocalIdentity,
  stateLabel,
  stateTone,
  threadCount,
  threadLabel,
  updateAdvice,
  useMachines,
  versionSkew,
} from "./machines";

const MACHINE: Machine = {
  id: "gw-7c1d9e4f2a8b6d30",
  name: "Lars's Linux box",
  platform: "linux",
  version: "1.28.0",
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
  /** What `machines_discover` finds. */
  discovered: {
    name: string;
    source: "bonjour" | "overlay";
    address: string | null;
    addresses: string[];
    gatewayId: string | null;
    platform?: string | null;
    version?: string | null;
    routeKind?: string;
    hint?: string | null;
    saved: boolean;
  }[] = [];
  /** What the local search says when it failed; the overlay rows are still listed. */
  discoverNotice: string | null = null;
  /** Whether a terminal's `rustcode` reads as an older version. */
  pathStale = false;
  /** Whether the background server reads as installed. */
  serverInstalled = true;
  /** What `server_service_install` refuses with, when it does. */
  installError: Error | null = null;
  /** Whether the install reports back as not installed (the CLI's partial answer). */
  installAnswersInstalled = true;
  /** What the shell answers for `app_version` (a dev build in a browser). */
  appVersion: string | null = "1.28.0";
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
      case "machines_discover":
        return { rows: this.discovered, notice: this.discoverNotice };
      case "server_service_status":
        return this.serverService();
      case "server_service_install":
        if (this.installError) throw this.installError;
        this.serverInstalled = this.installAnswersInstalled;
        return this.serverService();
      case "server_link_path":
        if (this.installError) throw this.installError;
        this.pathStale = false;
        return this.serverService();
      case "app_version":
        return this.appVersion;
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

  /** `server_service_status`, as `src-tauri/src/service.rs` answers it. */
  private serverService() {
    const installed = this.serverInstalled;
    return {
      installed,
      running: installed,
      version: installed ? "1.28.0" : null,
      cliVersion: "1.28.0",
      problems: this.pathStale
        ? [
            {
              code: "path-version-mismatch",
              message: "`rustcode` on PATH is 1.25.0, but this engine is 1.28.0.",
            },
          ]
        : [],
      // The shell that installs and still reads "not installed" keeps the
      // command's own words here.
      error:
        installed || this.installAnswersInstalled
          ? null
          : "The RustCode server was installed but does not read as installed yet.",
    };
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
    server: null,
    serverInstalling: false,
    serverLinking: false,
  });
  forgetLocalAppVersion();
});

afterEach(() => {
  // The machines and their channels are the module's, not this file's: another file's engine must
  // not inherit them (`resetForTests` clears the same state when a file connects through the store).
  resetMachinesForTests();
  setShellTransport(null);
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

describe("machines at app start", () => {
  test("a first read that failed is tried again, and stops at the first answer", async () => {
    const waits: number[] = [];
    let reads = 0;
    await readUntilAnswering(
      async () => ++reads >= 3,
      async (ms) => {
        waits.push(ms);
      },
    );
    expect(reads).toBe(3);
    // The two failures waited the first two waits, in order.
    expect(waits).toEqual(START_RETRY_WAITS_MS.slice(0, 2));

    // A shell that answers at once costs no wait at all.
    const immediate: number[] = [];
    await readUntilAnswering(
      async () => true,
      async (ms) => {
        immediate.push(ms);
      },
    );
    expect(immediate).toEqual([]);
  });

  test("a read that never answers stops after the last wait", async () => {
    const waits: number[] = [];
    let reads = 0;
    await readUntilAnswering(
      async () => {
        reads += 1;
        return false;
      },
      async (ms) => {
        waits.push(ms);
      },
    );
    expect(waits).toEqual(START_RETRY_WAITS_MS);
    // One read before the first wait, and one after each of them.
    expect(reads).toBe(START_RETRY_WAITS_MS.length + 1);
    // 1 + 2 + 4 + 8 + 16 + 30 s, then the page is left to read when it opens.
    expect(START_RETRY_WAITS_MS.reduce((sum, ms) => sum + ms, 0)).toBe(61_000);
  });
});

describe("machines rows", () => {
  test("a state says where the machine stands, in the words the row uses", () => {
    expect(stateLabel({ state: "connected", retryInSecs: null })).toBe("Connected");
    expect(stateLabel({ state: "connecting", retryInSecs: null })).toBe("Connecting");
    expect(stateLabel({ state: "offline", retryInSecs: 4 })).toBe("Reconnecting…");
    expect(stateLabel({ state: "offline", retryInSecs: null })).toBe("Offline");
    expect(stateLabel({ state: "needsPairing", retryInSecs: null })).toBe("Needs pairing");
    expect(stateLabel({ state: "stopped", retryInSecs: null })).toBe("Not connected");
    expect(stateTone({ state: "connected", retryInSecs: null })).toBe("ok");
    expect(stateTone({ state: "needsPairing", retryInSecs: null })).toBe("bad");
    expect(stateTone({ state: "offline", retryInSecs: null })).toBe("warn");
    // Between attempts the app is working on it: the calm tone, as `Connecting` has.
    expect(stateTone({ state: "offline", retryInSecs: 4 })).toBe("idle");
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
    // The shell's hint is the row's own line, not part of this one (#3433).
    const hinted = machineLine({
      ...MACHINE,
      hint: "Not reachable over NetBird (100.90.16.237:17879). On that machine, check that NetBird allows inbound connections.",
    });
    expect(hinted).toBe("LAN · Unencrypted");
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

describe("the machines a browse and a handshake find", () => {
  const studio = {
    name: "Studio",
    source: "bonjour" as const,
    address: "192.168.1.20:17879",
    addresses: ["192.168.1.20"],
    gatewayId: "gw-8e1f4b7a5c2d4e6f",
    platform: "macos",
    version: "0.96.0",
    routeKind: "lan",
    hint: null,
    saved: true,
  };
  const studioOverlay = {
    ...studio,
    source: "overlay" as const,
    address: "100.90.16.237:17879",
    addresses: ["100.90.16.237:17879"],
    routeKind: "netbird",
    hint: "Not reachable over NetBird (100.90.16.237:17879).",
  };

  test("one machine found both ways is one row, with what either source knows", () => {
    const rows = mergeDiscovered([studio, studioOverlay]);
    expect(rows).toHaveLength(1);
    // The LAN row keeps the route it had; the overlay address is not lost.
    expect(rows[0]?.address).toBe("192.168.1.20:17879");
    expect(rows[0]?.addresses).toEqual(["192.168.1.20", "100.90.16.237:17879"]);
    expect(rows[0]?.version).toBe("0.96.0");
    expect(rows[0]?.hint).toContain("NetBird");
  });

  test("a machine with no host id yet is one row by name, and different names stay apart", () => {
    const unnamed = { ...studio, gatewayId: null, saved: false };
    const rows = mergeDiscovered([
      unnamed,
      { ...unnamed, source: "bonjour" },
      { ...studioOverlay, name: "Lars's Linux box", gatewayId: "gw-7c1d9e4f2a8b6d30" },
    ]);
    expect(rows.map((row) => row.name)).toEqual(["Studio", "Lars's Linux box"]);
  });

  test("the browse is what fills the sheet", async () => {
    shell.discovered = [studio, studioOverlay];
    await discoverMachines();
    expect(useMachines.getState().discovered).toHaveLength(1);
    expect(useMachines.getState().discovering).toBe(false);
  });

  test("a browse that fails still lists the overlay rows, and says why", async () => {
    // The shell answers with the saved machine's overlay row and the reason its
    // local search found nothing (mDNS cannot bind in a container, say).
    shell.discovered = [studioOverlay];
    shell.discoverNotice = "Could not look for machines on this network: no multicast interface";
    await discoverMachines();
    expect(useMachines.getState().discovered?.map((row) => row.name)).toEqual(["Studio"]);
    expect(useMachines.getState().discoverError).toContain("no multicast interface");
    expect(useMachines.getState().discovering).toBe(false);
    // Looking again clears the notice once the browse answers.
    shell.discoverNotice = null;
    await discoverMachines();
    expect(useMachines.getState().discovered).toHaveLength(1);
    expect(useMachines.getState().discoverError).toBeNull();
  });

  test("an overlay address says which product made it", () => {
    expect(routeName(null, "netbird")).toBe("NetBird");
    expect(routeName(null, "tailscale")).toBe("Tailscale");
    expect(routeName(null, "zerotier")).toBe("ZeroTier");
    expect(routeName(null, "lan")).toBe("LAN");
    expect(routeName(null, "vpn")).toBe("VPN");
    expect(
      routeName({ kind: "vpn", address: "100.90.16.237:17879", provider: "netbird", encrypted: true }),
    ).toBe("NetBird");
    expect(routeName(null)).toBe("No route saved");
  });
});

describe("a machine on another RustCode version", () => {
  test("an older machine says what to update, and how on its platform", () => {
    expect(versionSkew("0.96.0", "1.28.0", "macos")).toBe(
      "RustCode 0.96.0 on that machine — update it there: Settings › About › Check for updates, or `brew upgrade rustcode`",
    );
    expect(versionSkew("0.96.0", "1.28.0", "linux")).toContain("install the new AppImage");
    expect(versionSkew("0.96.0", "1.28.0", "windows")).toContain("run the RustCode installer");
    expect(updateAdvice("")).toBe("install the newer RustCode");
    // The app is the one to update when the machine is ahead.
    expect(versionSkew("1.30.0", "1.28.0", "macos")).toBe(
      "RustCode 1.30.0 on that machine — update RustCode here",
    );
  });

  test("the same version, or one nothing reported, says nothing", () => {
    expect(versionSkew("1.28.0", "1.28.0")).toBeNull();
    expect(versionSkew("1.28.0", null)).toBeNull();
    expect(versionSkew(null, "1.28.0")).toBeNull();
    // A beta of the same release is older, and says so.
    expect(versionSkew("1.28.0-beta.2", "1.28.0")).toContain("update it there");
  });

  test("a development build does not call every released machine newer", () => {
    // `tauri dev` reports tauri.conf.json's placeholder; the mock reports 1.28.0.
    expect(versionSkew("1.28.0", DEV_APP_VERSION)).toBeNull();
    expect(versionSkew(DEV_APP_VERSION, "1.28.0")).toContain("update it there");
  });

  test("this app's version is read once, from the shell when it is not the app", async () => {
    forgetLocalAppVersion();
    shell.appVersion = "1.28.0";
    expect(await localAppVersion()).toBe("1.28.0");
    shell.appVersion = "1.29.0";
    expect(await localAppVersion()).toBe("1.28.0");
    forgetLocalAppVersion();
  });
});

function gatewayOff(): GatewayService {
  return {
    remoteEnabled: false,
    running: false,
    address: null,
    advertised: null,
    owner: null,
    version: null,
    pid: null,
    pairedDevices: 0,
    port: null,
  };
}

describe("the RustCode server in the background", () => {
  test("its state reads as installed and running, or what is missing", () => {
    expect(serverLine(null)).toBe("Not checked yet");
    expect(
      serverLine({
        installed: false,
        running: false,
        version: null,
        cliVersion: "1.28.0",
        gateway: gatewayOff(),
        pathRustcode: null,
        problems: [],
        error: null,
      }),
    ).toBe("Not installed");
    expect(
      serverLine({
        installed: true,
        running: true,
        version: "1.28.0",
        cliVersion: "1.28.0",
        gateway: gatewayOff(),
        pathRustcode: null,
        problems: [],
        error: null,
      }),
    ).toBe("Installed · Running");
    expect(
      serverLine({
        installed: true,
        running: false,
        version: null,
        cliVersion: "1.28.0",
        gateway: gatewayOff(),
        pathRustcode: null,
        problems: [],
        error: null,
      }),
    ).toBe("Installed · Not running");
  });

  test("the gateway says where it runs and who keeps it running", () => {
    const server = {
      remoteEnabled: true,
      running: true,
      address: "100.90.28.23:17879",
      advertised: "100.90.28.23:17879",
      owner: "server" as const,
      version: "1.28.0",
      pid: 97702,
      pairedDevices: 1,
      port: 17879,
    };
    expect(gatewayLine(server, true)).toBe("Running on 100.90.28.23:17879 (started by the RustCode server)");
    // Started by the app: reachable only while the app runs, and the line says so.
    expect(gatewayLine({ ...server, owner: "app" }, true)).toBe(
      "Running on 100.90.28.23:17879 (started by the app)",
    );
    expect(gatewayLine({ ...server, running: false, address: null }, true)).toBe(
      "Not running · the server starts it on port 17879",
    );
    // No server installed: nothing restarts it, so the line must not say one does.
    expect(gatewayLine({ ...server, running: false, address: null }, false)).toBe(
      "Not running · `rustcode remote start` starts it on port 17879",
    );
    expect(gatewayLine({ ...gatewayOff(), port: null })).toBe("Off");
    expect(gatewayLine(undefined)).toBe("Not checked yet");
  });

  test("reading it, then installing it, leaves the page knowing it runs", async () => {
    await refreshServerService();
    expect(useMachines.getState().server?.installed).toBe(true);
    shell.serverInstalled = false;
    await refreshServerService();
    expect(useMachines.getState().server?.installed).toBe(false);
    expect(await installServer()).toBe(true);
    expect(useMachines.getState().server?.running).toBe(true);
    expect(useMachines.getState().serverInstalling).toBe(false);
    expect(updateAdvice("macos")).toContain("brew upgrade rustcode");
  });

  test("pointing a terminal's rustcode at the app clears the version warning", async () => {
    shell.pathStale = true;
    await refreshServerService();
    expect(useMachines.getState().server?.problems.map((problem) => problem.code)).toEqual([
      "path-version-mismatch",
    ]);
    expect(await linkCliToApp()).toBe(true);
    expect(useMachines.getState().server?.problems).toEqual([]);
    expect(useMachines.getState().serverLinking).toBe(false);
    // A copy the CLI refuses to touch (Homebrew's) keeps its reason on screen.
    shell.installError = new Error("Homebrew's copy; run `brew upgrade lhagfoss/tap/rustcode`");
    expect(await linkCliToApp()).toBe(false);
    expect(useMachines.getState().serverLinking).toBe(false);
  });

  test("an install that fails says why, and leaves the page without a server", async () => {
    shell.serverInstalled = false;
    shell.installError = new Error("a development build; run `rustcode server install` from a release");
    expect(await installServer()).toBe(false);
    expect(useMachines.getState().serverInstalling).toBe(false);
    // Nothing is claimed installed that the shell never confirmed.
    expect(useMachines.getState().server?.installed ?? false).toBe(false);
  });

  test("an install that answers not-installed keeps the CLI's reason on the page", async () => {
    shell.serverInstalled = false;
    shell.installAnswersInstalled = false;
    expect(await installServer()).toBe(false);
    expect(useMachines.getState().server?.error).toContain("installed but");
    expect(useMachines.getState().serverInstalling).toBe(false);
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

describe("a machine's local name and icon", () => {
  test("the display name wins, and a blank one falls back to the reported name", () => {
    expect(machineName(MACHINE)).toBe("Lars's Linux box");
    expect(machineName({ ...MACHINE, displayName: "Linux box" })).toBe("Linux box");
    expect(machineName({ ...MACHINE, displayName: "   " })).toBe("Lars's Linux box");
    expect(machineName({ ...MACHINE, displayName: null })).toBe("Lars's Linux box");
    expect(cleanMachineName("  Studio  ")).toBe("Studio");
    expect(cleanMachineName("  ")).toBeNull();
  });

  test("a chosen icon wins over the platform's, and an unknown one falls back", () => {
    expect(platformIcon("macos")).toBe("laptop");
    expect(platformIcon("darwin")).toBe("laptop");
    expect(platformIcon("windows")).toBe("tower");
    expect(platformIcon("linux")).toBe("linux");
    expect(platformIcon(null)).toBe("unknown");
    expect(machineGlyph(MACHINE)).toBe("linux");
    expect(machineGlyph({ ...MACHINE, icon: "studio" })).toBe("studio");
    expect(machineGlyph({ ...MACHINE, icon: "not-an-icon" })).toBe("linux");
    expect(machineTint({ iconTint: "4" })).toBe("4");
    expect(machineTint({ iconTint: "12" })).toBeNull();
    expect(machineTint({ iconTint: null })).toBeNull();
  });

  test("this computer's name and icon persist", () => {
    const scope = globalThis as typeof globalThis & { localStorage: Storage };
    const previous = scope.localStorage;
    const data = new Map<string, string>();
    scope.localStorage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
      clear: () => data.clear(),
      key: (index: number) => [...data.keys()][index] ?? null,
      get length() {
        return data.size;
      },
    } as Storage;
    try {
      setLocalIdentity({ displayName: "  Desk Mac  ", icon: "studio", tint: "2" });
      expect(selfName()).toBe("Desk Mac");
      expect(localIdentity()).toEqual({ displayName: "Desk Mac", icon: "studio", tint: "2" });
      expect(JSON.parse(data.get("rustcode.localMachineIdentity") ?? "null")).toMatchObject({
        displayName: "Desk Mac",
        icon: "studio",
        tint: "2",
      });
      // A blank name returns to the platform's own name, and an unknown icon is dropped.
      setLocalIdentity({ displayName: "   ", icon: "not-an-icon" as never });
      expect(selfName()).toBe(defaultSelfName());
      expect(localIdentity()).toEqual({ displayName: null, icon: null, tint: "2" });
    } finally {
      if (previous) scope.localStorage = previous;
      else Reflect.deleteProperty(scope, "localStorage");
      setLocalIdentity({ displayName: null, icon: null, tint: null });
    }
  });
});

describe("a route's last test", () => {
  const at = "2026-11-12T10:00:00Z";
  test("reads as reachable, refused or timed out, with the latency when there is one", () => {
    expect(probeLabel(null)).toBe("Not tested");
    expect(probeLabel({ outcome: "reachable", latencyMs: 4, detail: null, at })).toBe("Reachable · 4 ms");
    expect(probeLabel({ outcome: "refused", latencyMs: 2, detail: null, at })).toBe("Refused · 2 ms");
    expect(probeLabel({ outcome: "timeout", latencyMs: null, detail: null, at })).toBe("Timed out");
    expect(probeLabel({ outcome: "unreachable", latencyMs: null, detail: "no route", at })).toBe(
      "Unreachable",
    );
  });

  test("is green only when the address answered", () => {
    expect(probeTone(null)).toBe("idle");
    expect(probeTone({ outcome: "reachable", latencyMs: 1, detail: null, at })).toBe("ok");
    expect(probeTone({ outcome: "refused", latencyMs: 1, detail: null, at })).toBe("bad");
  });
});
