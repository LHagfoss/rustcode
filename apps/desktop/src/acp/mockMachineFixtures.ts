// design-lint: allow-file (fake link colours in canned data)
// The machines the mock is paired with, and the network it discovers (acp/mock.ts).

import { type JsonObject, str } from "./jsonrpc";
import type { MockOptions } from "./mock";

/**
 * The git remote of the mock's projects, as a machine's `session/list` reports it
 * (`_meta.rustcode.remote`): one repository checked out in two places, which is what **Run on**
 * finds again (state/projects.ts).
 */
export function mockRemote(cwd: string): string | undefined {
  if (cwd.startsWith("/Users/you/code/rustcode") || cwd.startsWith("/Users/lars/code/rustcode"))
    return "git@github.com:LHagfoss/rustcode.git";
  if (cwd.startsWith("/Users/you/code/website")) return "git@github.com:LHagfoss/rustcode-website.git";
  return undefined;
}

/**
 * The machine this app is paired with in the mock: a Linux box with three
 * threads, which is what `Lars's Linux box` is on the machine network plan.
 */
export const MOCK_MACHINE = {
  id: "gw-7c1d9e4f2a8b6d30",
  name: "Lars's Linux box",
  platform: "linux",
  host: "ubuntu-1.local",
  route: "192.168.1.31:17879",
  overlay: "100.90.16.237:17879",
  threads: 3,
};

/** The NetBird address the saved machine's handshake reported (#3402). */
export const MOCK_OVERLAY_ROUTE = {
  kind: "vpn",
  address: MOCK_MACHINE.overlay,
  provider: "netbird",
  encrypted: true,
};

/**
 * One saved machine as `machines_list` reports it (`src-tauri/src/machines.rs`).
 * `?machines=connect` asks for the overlay route and an older version, the
 * state the Add sheet has to explain.
 */
export function mockMachine(
  state: "connected" | "offline" | "needsPairing",
  addedAt: number,
  extra: { overlay?: boolean; version?: string; hint?: string | null } = {},
): JsonObject {
  const route = { kind: "lan", address: MOCK_MACHINE.route, encrypted: false };
  const routes = extra.overlay ? [route, MOCK_OVERLAY_ROUTE] : [route];
  return {
    id: MOCK_MACHINE.id,
    name: MOCK_MACHINE.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_MACHINE.platform,
    version: extra.version ?? "1.28.0",
    routes,
    route,
    state,
    detail:
      state === "offline"
        ? "failed to connect to 192.168.1.31:17879: Connection timed out (os error 60)"
        : state === "needsPairing"
          ? "Lars's Linux box ended this computer's pairing. Pair it again to see its threads."
          : null,
    hint: extra.hint ?? null,
    retryInSecs: state === "offline" ? 4 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: state === "connected" ? new Date().toISOString() : new Date(addedAt).toISOString(),
  };
}

/** A second computer on the same network, with threads of its own (`?machines=paired`). */
export const MOCK_STUDIO = {
  id: "gw-8e1f4b7a5c2d4e6f",
  name: "Studio",
  platform: "macos",
  route: "192.168.1.20:17879",
  version: "1.28.0",
  // Two threads in one project and one in another, so **Run on** shows a choice of folders.
  threads: 3,
};

/** A Windows computer on the same network, one RustCode behind (`?machines=paired`). */
export const MOCK_WINDOWS = {
  id: "gw-4a2c8e5f1b7d9c30",
  name: "Lars's Windows box",
  platform: "windows",
  route: "192.168.1.44:17879",
  version: "1.27.0",
  threads: 2,
};

/** The Studio row: a Mac on the LAN, connected (or not) like the Linux box. */
export function mockStudio(state: "connected" | "offline", addedAt: number): JsonObject {
  return {
    id: MOCK_STUDIO.id,
    name: MOCK_STUDIO.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_STUDIO.platform,
    version: MOCK_STUDIO.version,
    routes: [{ kind: "lan", address: MOCK_STUDIO.route, encrypted: false }],
    route: { kind: "lan", address: MOCK_STUDIO.route, encrypted: false },
    state,
    detail: state === "offline" ? "Not connected. Retrying in 8 s." : null,
    retryInSecs: state === "offline" ? 8 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: new Date(addedAt).toISOString(),
  };
}

/** The Windows row: a PC on the LAN, one RustCode behind this app (the version sentence). */
export function mockWindows(state: "connected" | "offline", addedAt: number): JsonObject {
  return {
    id: MOCK_WINDOWS.id,
    name: MOCK_WINDOWS.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_WINDOWS.platform,
    version: MOCK_WINDOWS.version,
    routes: [{ kind: "lan", address: MOCK_WINDOWS.route, encrypted: false }],
    route: { kind: "lan", address: MOCK_WINDOWS.route, encrypted: false },
    state,
    detail: state === "offline" ? "Not connected. Retrying in 8 s." : null,
    retryInSecs: state === "offline" ? 8 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: new Date(addedAt).toISOString(),
  };
}

/** The forwarded port `?machines=forward` reaches its VPS on, and the advice the shell gives for it. */
export const MOCK_FORWARD = {
  address: "127.0.0.1:17880",
  host: "LagosVPS",
  advice:
    "127.0.0.1:17880 takes connections but nothing answers behind it: the tunnel that forwards it, started outside RustCode, lost its link (often after this computer slept). Restart it, keeping it alive with `ssh -N -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -L 17880:127.0.0.1:17879 LagosVPS`, so it ends itself within 45 s of losing the link. Or let RustCode manage this tunnel: it then opens it itself and reopens it after a wake.",
};

/**
 * A VPS reached only through a user-run `ssh -L` whose link died while this computer slept
 * (`?machines=forward`): Connecting, with what to run and the offer to let RustCode manage the
 * tunnel (`stalled_forward` in `src-tauri/src/machines.rs`).
 */
function mockForwardedVps(addedAt: number): JsonObject {
  const route = { kind: "lan", address: MOCK_FORWARD.address, encrypted: false };
  return {
    id: "gw-3a9c51d0e7b24f18",
    name: "LagosVPS",
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: "linux",
    version: "1.28.0",
    routes: [route],
    route,
    state: "connecting",
    reason: "unreachable",
    detail: MOCK_FORWARD.advice,
    hint: null,
    retryInSecs: null,
    forward: {
      address: MOCK_FORWARD.address,
      ssh: MOCK_FORWARD.host,
      sshPort: null,
      identity: null,
      gatewayPort: 17879,
    },
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: new Date(Date.now() - 1000 * 60 * 7).toISOString(),
  };
}

/** The saved machines `?machines=` starts with. */
export function mockMachineList(option: MockOptions["machines"]): JsonObject[] {
  const savedAt = Date.now() - 1000 * 60 * 60 * 24 * 6;
  switch (option) {
    case "connect":
      // Saved once, on an older RustCode, and reached over NetBird: the Add
      // sheet lists it, says to update it, and the server is not installed.
      return [
        mockMachine("offline", savedAt, {
          overlay: true,
          version: "0.96.0",
          hint: "Not reachable over NetBird (100.90.16.237:17879). On that machine, check that NetBird allows inbound connections and that an access policy allows TCP 17879 between the two machines.",
        }),
      ];
    case "offline":
      return [mockMachine("offline", savedAt)];
    case "revoked":
      return [mockMachine("needsPairing", savedAt)];
    case "forward":
      return [mockForwardedVps(savedAt)];
    case "paired":
      // The sidebar's own happy path: this Mac, a Windows PC with threads, and a machine
      // that is away, saved during this session so the Mac and the PC read as just saved.
      return [
        mockStudio("connected", Date.now()),
        mockWindows("connected", savedAt),
        mockMachine("offline", savedAt),
      ];
    default:
      return [mockMachine("connected", savedAt)];
  }
}

/**
 * Whether an address is on the public internet, by the same rule
 * `rustcode-client` applies (`registry::classify_address`): a private, loopback
 * or link-local address is LAN, CGNAT and `.ts.net` are an overlay, a bare name
 * or `.local` is LAN, and anything else is public. The client refuses a public
 * address because the gateway has no TLS.
 */
export function mockIsPublic(address: string): boolean {
  const withoutScheme = address.trim().replace(/^[a-z][a-z0-9+.-]*:\/\//i, "");
  const endpoint = withoutScheme.split("/")[0] ?? withoutScheme;
  const host = (endpoint.split(":")[0] ?? endpoint).replace(/^\[|\]$/g, "").toLowerCase();
  if (!host) return false;
  if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {
    const [a, b] = host.split(".").map(Number) as [number, number];
    if (a === 10 || a === 127) return false;
    if (a === 192 && b === 168) return false;
    if (a === 172 && b >= 16 && b <= 31) return false;
    if (a === 169 && b === 254) return false;
    // 100.64.0.0/10 is CGNAT, which Tailscale and other overlays hand out.
    if (a === 100 && b >= 64 && b <= 127) return false;
    return true;
  }
  // IPv6: loopback, unique-local and link-local are private; the rest are not.
  if (host.includes(":")) return !(host === "::1" || host.startsWith("fd") || host.startsWith("fe80"));
  if (host.endsWith(".local") || host.endsWith(".ts.net")) return false;
  return host.includes(".");
}

/**
 * The machines `machines_discover` lists in the mock, as the shell answers:
 * Bonjour on this network, plus the saved machine's own NetBird address, which
 * the page merges by host id into one row per machine. The local search
 * always works here, so the notice is null.
 */
export function mockDiscovery(saved: JsonObject[]): JsonObject {
  return { rows: mockDiscovered(saved), notice: null };
}

export function mockDiscovered(saved: JsonObject[]): JsonObject[] {
  const machine = saved.find((row) => row.id === MOCK_MACHINE.id);
  const version = machine ? String(machine.version ?? "") || null : null;
  return [
    {
      name: MOCK_MACHINE.name,
      source: "bonjour",
      address: MOCK_MACHINE.route,
      addresses: ["192.168.1.31"],
      gatewayId: MOCK_MACHINE.id,
      platform: MOCK_MACHINE.platform,
      version,
      routeKind: "lan",
      hint: null,
      saved: Boolean(machine),
    },
    {
      name: "Studio",
      source: "bonjour",
      address: "192.168.1.20:17879",
      addresses: ["192.168.1.20"],
      gatewayId: "gw-8e1f4b7a5c2d4e6f",
      platform: "macos",
      version: "1.28.0",
      routeKind: "lan",
      hint: null,
      saved: saved.some((row) => row.id === "gw-8e1f4b7a5c2d4e6f"),
    },
    ...(machine
      ? [
          {
            name: MOCK_MACHINE.name,
            source: "overlay",
            address: MOCK_MACHINE.overlay,
            addresses: [MOCK_MACHINE.overlay],
            gatewayId: MOCK_MACHINE.id,
            platform: MOCK_MACHINE.platform,
            version,
            routeKind: "netbird",
            hint: machine.hint ?? null,
            saved: true,
          },
        ]
      : []),
  ];
}

/** The RustCode server as a background service, as `src-tauri/src/service.rs` answers. */
export function mockServerService(installed: boolean): JsonObject {
  return {
    installed,
    running: installed,
    version: installed ? "1.28.0" : null,
    cliVersion: "1.28.0",
    gateway: {
      remoteEnabled: installed,
      running: installed,
      address: installed ? "192.168.1.20:17879" : null,
      advertised: installed ? "192.168.1.20:17879" : null,
      owner: installed ? "server" : null,
      version: installed ? "1.28.0" : null,
      pid: installed ? 97702 : null,
      pairedDevices: 1,
      port: 17879,
    },
    pathRustcode: null,
    problems: installed
      ? []
      : [{ code: "not-installed", message: "The RustCode server service is not installed." }],
    error: null,
  };
}

/**
 * One saved machine's ACP, answered in process for `?machines=`: the app's own
 * shell dials the real machine over its gateway, so this exists only where
 * there is no shell to dial with. It answers `initialize`, `session/list`,
 * `_rustcode/harnesses` and `session/new`, which is what the app reads and does
 * of a connected machine (docs/desktop.md › Several machines).
 */
/** This computer's id in the mock fleet. */
export const MOCK_LOCAL_HOST = {
  id: "gw-0a1b2c3d4e5f6071",
  name: "Lars's MacBook",
  platform: "macos",
  version: "1.28.0",
};

/** Whether a report's name replaces the known one: the later time, then the greater name. */
export function mockNameWins(incoming: JsonObject, known: JsonObject | undefined): boolean {
  if (!known) return true;
  const [leftAt, leftName] = [str(incoming.nameUpdatedAt) ?? "", str(incoming.name) ?? ""];
  const [rightAt, rightName] = [str(known.nameUpdatedAt) ?? "", str(known.name) ?? ""];
  return leftAt === rightAt ? leftName > rightName : leftAt > rightAt;
}

/** What a computer in the mock fleet says about itself. */
export function mockHostEntry(host: {
  id: string;
  name: string;
  platform: string;
  version: string;
}): JsonObject {
  return {
    hostId: host.id,
    name: host.name,
    platform: host.platform,
    version: host.version,
    lastSeenAt: new Date().toISOString(),
  };
}

/** A computer the Studio knows and this one is not paired with, for the "known via" row. */
export const MOCK_FLEET_PI = {
  hostId: "gw-5d1e9a3c7b2f4086",
  name: "Raspberry Pi",
  platform: "linux",
  version: "1.26.0",
};
