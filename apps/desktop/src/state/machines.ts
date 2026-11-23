// The other computers this app is paired with (Settings › Machines). One
// saved machine is one entry, reached over an ordered list of routes; the
// app's Rust shell runs a supervisor per machine and relays its ACP here, and
// this module holds what the page shows: the machines, where each connection
// stands, and one ACP client per machine (the same class the local engine
// uses), so a connected machine's `session/list` gives its thread count.
//
// The store routes every thread operation over the same connection this module
// opens (`state/store.ts` `connectionFor`), and the sidebar lists each machine's
// threads in one group (docs/desktop.md › Several machines).
//
// A machine is a computer, a route is one way to reach it, and the server is
// what runs on it (docs/terminology.md).

import {
  type MachineIdentity,
  machineName,
  mergeDiscovered,
  readLocalIdentity,
  routeChangeNotices,
  toldRouteChanges,
} from "./machineLabels";

export {
  cleanMachineName,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  DEV_APP_VERSION,
  defaultSelfName,
  forgetLocalAppVersion,
  isLocalForward,
  LOCAL_MACHINE_ID,
  lastSeen,
  localAppVersion,
  localIdentity,
  MACHINE_ICONS,
  MACHINE_TINTS,
  type MachineIcon,
  type MachineIdentity,
  type MachineTint,
  machineGlyph,
  machineIconOf,
  machineLine,
  machineName,
  machineReconnecting,
  machineTint,
  machineTintOf,
  mergeDiscovered,
  offlineSinceLabel,
  platformIcon,
  platformName,
  type RouteChangeNotice,
  reasonLabel,
  routeChangeNotices,
  routeLabel,
  routeName,
  selfName,
  setLocalIdentity,
  setMachineDisplayName,
  setMachineIcon,
  stateLabel,
  stateTone,
  threadCount,
  threadLabel,
  updateAdvice,
  useLocalIdentity,
  useMachineReconnecting,
  versionSkew,
} from "./machineLabels";

import type { ConnectOffer, SshStep } from "./machineConnect";

export {
  type ConnectOffer,
  cancelConnectCode,
  connectOverSsh,
  connectWithCode,
  createConnectCode,
  dismissSsh,
  followConnectCode,
  pairBackNotice,
  parseConnectOffer,
  SSH_DEADLINE_MS,
  type SshRun,
  type SshStep,
} from "./machineConnect";

import { create } from "zustand";
import { AcpConnection, type JsonObject, obj, str } from "../acp/jsonrpc";
import { isTauri, MachineTransport } from "../acp/transport";
import { Method, parseSessionList, type SessionSummary } from "../acp/types";
import type { Op } from "../lib/op";
import { hasShell, shellCall, shellCallWithin } from "../lib/shell";
import { forgetMachineCache, forgetTranscript, loadThreadList, saveThreadList } from "./machineCache";
import { type LinkState, LinkTable, linkConnection, linkMachineState } from "./machineLink";
import { loadMirroredList, mirrorThreadList, withMirroredRows } from "./machineMirror";
import { forgetMachineOutbox, forgetThreadOutbox } from "./machineOutbox";
import {
  askConfirm,
  deferAnnouncement,
  errorText,
  flushMachineOutbox,
  isTombstoned,
  keptUnlisted,
  machineInitializeParams,
  noteThreadList,
  settleKeptUnlisted,
  toast,
  tombstoneThread,
} from "./store";
import { splitThreadKey, threadKey } from "./threadKeys";

/**
 * Where a machine stands, as the shell reports it. `unavailable` is a machine a route reached
 * that would not serve this computer (its server is not running, another protocol version, this
 * device not allowed): `reason` says which, and the shell keeps trying.
 */
export type MachineState =
  | "connecting"
  | "connected"
  | "offline"
  | "unavailable"
  | "needsPairing"
  | "stopped";

/**
 * Why a machine is not connected, as the shell names it: `unreachable` (no route answered),
 * `rejected_token`, `version_mismatch`, `not_allowed`, `server_not_running`, `busy`, `rejected`,
 * or `dropped` (it was up and the connection ended).
 */
export type MachineReason =
  | "unreachable"
  | "rejected_token"
  | "version_mismatch"
  | "not_allowed"
  | "server_not_running"
  | "busy"
  | "rejected"
  | "dropped";

/** One way to reach a machine: a LAN address, Tailscale, a VPN, SSH, or public. */
export interface MachineRoute {
  /** `lan`, `netbird`, `tailscale`, `zerotier`, `vpn`, `ssh` or `public`. */
  kind: string;
  address: string;
  /** The mesh product the address is on (`netbird`, `tailscale`, `zerotier`), when the machine named it. */
  provider?: string | null;
  /** The network encrypts the traffic (Tailscale, a VPN); a LAN route does not. */
  encrypted: boolean;
  /** What the last test of this route found, or null if it was never tested. */
  probe?: RouteProbe | null;
  /** Turned off in its settings: shown, never dialled. */
  off?: boolean;
  /** Added by hand, so removing it is final; one the machine reports comes back. */
  manual?: boolean;
  /** An SSH route's `user@host` or alias, and RustCode's port there. */
  sshDestination?: string | null;
  sshGatewayPort?: number | null;
}

/** The last plain connect to one route: whether it answered and how fast. */
export interface RouteProbe {
  outcome: "reachable" | "refused" | "timeout" | "unreachable";
  /** Milliseconds to connect, or to be turned away; null for a timeout. */
  latencyMs: number | null;
  detail: string | null;
  at: string;
}

/** A machine that moved to another kind of network while this app held it. */
export interface MachineRouteChange {
  seq: number;
  from: MachineRoute;
  to: MachineRoute;
  /** From a tunnel to a network that does not encrypt. */
  downgrade: boolean;
}

/** A saved machine, as `machines_list` reports it. */
export interface Machine {
  id: string;
  /** The name the machine reports. */
  name: string;
  /** The name this app shows, or null while the reported one is shown. */
  displayName?: string | null;
  /** The glyph this app draws, or null for the platform's own. */
  icon?: string | null;
  /** The series tint of that glyph, `1`..`8`. */
  iconTint?: string | null;
  /** `macos`, `windows`, `linux`, or `unknown`. */
  platform: string;
  /** The RustCode version it reported when it last answered, or null. */
  version: string | null;
  /** New threads are not started here from this app; its connection stays. */
  paused?: boolean;
  routes: MachineRoute[];
  /** The route in use, or the one it will try first. */
  route: MachineRoute | null;
  /**
   * The latest time this app reached the machine over another kind of network than before (the
   * LAN where it was a tunnel). `seq` counts them, so each is told once.
   */
  routeChange?: MachineRouteChange | null;
  state: MachineState;
  /** Why it is not connected, or null while it is (or has not tried yet). */
  reason?: MachineReason | null;
  detail: string | null;
  /**
   * What to check when the machine is reachable one way and not another, e.g.
   * the LAN works while the NetBird address does not. Set by the shell; the
   * page may show it next to `detail`.
   */
  hint: string | null;
  /** Seconds until the next attempt, while it waits. */
  retryInSecs: number | null;
  /**
   * A forwarded port whose tunnel, run outside RustCode (`ssh -L`), is why the machine is away:
   * the row offers to let RustCode manage that tunnel. Null when there is nothing to offer.
   */
  forward?: MachineForward | null;
  addedAt: string;
  lastConnectedAt: string | null;
}

/** The forwarded-port route of a machine whose tunnel is down (`ForwardView` in the shell). */
export interface MachineForward {
  /** The forwarded route, e.g. `127.0.0.1:17880`. */
  address: string;
  /** The host that `ssh -L` signs in to, when the shell found it running; null asks for it. */
  ssh: string | null;
  sshPort: number | null;
  identity: string | null;
  /** The gateway's port on that host, where the forward pointed. */
  gatewayPort: number;
}

/**
 * A machine the Add sheet can list without an address typed, as
 * `machines_discover` reports it: one Bonjour found on this network, or one
 * whose own handshake reported an overlay address (NetBird, Tailscale) that
 * this app saved for it.
 */
export interface DiscoveredMachine {
  name: string;
  /** `bonjour` for a machine announcing itself here, `overlay` for a saved one reached over a tunnel. */
  source?: "bonjour" | "overlay";
  address: string | null;
  addresses: string[];
  gatewayId: string | null;
  /** The machine's platform, when it is saved and has said. */
  platform?: string | null;
  /** The RustCode version it runs, when it is saved and has answered. */
  version?: string | null;
  /** `lan`, `netbird`, `tailscale`, `zerotier`, `vpn` or `public`. */
  routeKind?: string;
  /** What to check when one kind of route does not answer (PR 3402). */
  hint?: string | null;
  /** This app already has it saved. */
  saved: boolean;
}

/**
 * What `machines_discover` answers: the rows, and what to say when the local
 * search failed — the overlay rows come from this app's registry and are in
 * `rows` either way.
 */
export interface Discovery {
  rows: DiscoveredMachine[];
  notice: string | null;
}

/** This computer's remote gateway, as `server status` reports it (`docs/remote.md`). */
export interface GatewayService {
  /** Remote access is on for this computer. */
  remoteEnabled: boolean;
  /** A gateway answers now, so paired devices can reach this computer. */
  running: boolean;
  /** What devices dial, while it runs. */
  address: string | null;
  advertised: string | null;
  /** Who started it: the server (it outlives the app), the app, or a terminal. */
  owner: "server" | "app" | "terminal" | null;
  version: string | null;
  pid: number | null;
  pairedDevices: number;
  /** The port it starts on from its recorded settings, while it runs not. */
  port: number | null;
}

/** The `rustcode` a shell finds first on `PATH`; an older copy is issue 3411. */
export interface PathRustcode {
  path: string;
  version: string | null;
}

/** The RustCode server as this computer runs it in the background (`server status`). */
export interface ServerService {
  /** A service is in place, so the server starts when you log in. */
  installed: boolean;
  /** A server answers on its socket now. */
  running: boolean;
  /** The running server's version. */
  version: string | null;
  /** The version of the `rustcode` that answered. */
  cliVersion: string | null;
  /** The remote gateway: whether this computer stays reachable with the app closed. */
  gateway: GatewayService;
  /** The `rustcode` on `PATH`, when it is another engine than this app's. */
  pathRustcode: PathRustcode | null;
  /** `linger-disabled`, `version-mismatch`, `path-version-mismatch` and the rest, as `server status` words them. */
  problems: { code: string; message: string }[];
  /** Why the status could not be read, when it could not. */
  error: string | null;
}

/** An accepted connection request, waiting for the code the machine shows. */
export interface AskView {
  askId: string;
  codeLength: number;
  expiresInSecs: number;
}

/**
 * Where the Add machine flow stands. `askId` is this page's own name for the
 * request, made before anything is dialled, so Cancel can drop it at any
 * point — even while the other machine's owner is still deciding. `since` is
 * when the stage began, so the page can say when nothing is coming back.
 */
export type Pairing =
  | { stage: "waiting"; address: string; host: string; askId: string; since: number }
  | {
      stage: "code";
      address: string;
      host: string;
      askId: string;
      codeLength: number;
      expiresAt: number;
    }
  | { stage: "pairing"; address: string; host: string; askId: string; since: number };

/**
 * How long the shell may take to check the code with the machine before the
 * page says it did not hear back. The exchange itself is bounded in the shell
 * (`CONNECT_TIMEOUT`, ten seconds) and so is the token write after it (ten
 * seconds more), so this is longer than the worst case that answers; a command
 * that never returns — a credential store waiting for an unlock, a shell that
 * went away — must not leave the page on a spinner (issue 3325).
 */
export const PAIR_DEADLINE_MS = 30_000;

/** Run a simple operation, preserving its caller's return value and failure policy. */
async function runOp<T>(key: "serverInstall" | "serverLink" | "probe", action: () => Promise<T>): Promise<T> {
  useMachines.setState({ [key]: { status: "running" } });
  try {
    const result = await action();
    useMachines.setState({ [key]: { status: "idle" } });
    return result;
  } catch (error) {
    useMachines.setState({ [key]: { status: "failed", error: errorText(error) } });
    throw error;
  }
}

interface MachinesStore {
  /** The first `machines_list` came back (or failed). */
  ready: boolean;
  error: string | null;
  machines: Machine[];
  /**
   * When each machine was first seen away, in ms: what an offline answer says
   * ("Studio is offline since 09:12"). Seeded from `lastConnectedAt` the first
   * time a machine is seen offline, so it reads as the last time it answered.
   */
  offlineSince: Record<string, number>;
  /** Each connected machine's thread count, from its own `session/list`. */
  counts: Record<string, number>;
  /** Last successfully fetched thread list per machine, retained across disconnects. */
  sessions: Record<string, SessionSummary[]>;
  discovered: DiscoveredMachine[] | null;
  discovery: Op;
  pairing: Op<Pairing>;
  /** The connect code this computer shows for another one, while it is open. */
  connectOffer: ConnectOffer | null;
  /** Creating and redeeming share the busy state used by the Add machine UI. */
  connect: Op;
  /** The connection over SSH in progress, or the last one: its steps as they go. */
  ssh: Op<{ steps: SshStep[]; notice?: string }>;
  /** This computer's RustCode server as a background service, once asked. */
  server: ServerService | null;
  /** This computer's own name and icon, kept here because it is not paired. */
  local: MachineIdentity;
  /** An install is running now. */
  serverInstall: Op;
  /** Pointing the `rustcode` on PATH at this app's engine is running now. */
  serverLink: Op;
  /** A route test is running now. */
  probe: Op;
  /** The Add machine dialog is open (Settings › Machines, or the new-thread selector asked). */
  incomingConnectCode?: string;
  addOpen: boolean;
}

export const useMachines = create<MachinesStore>(() => ({
  ready: false,
  error: null,
  machines: [],
  offlineSince: {},
  counts: {},
  sessions: {},
  discovered: null,
  discovery: { status: "idle" },
  pairing: { status: "idle" },
  connectOffer: null,
  connect: { status: "idle" },
  ssh: { status: "idle" },
  server: null,
  serverInstall: { status: "idle" },
  serverLink: { status: "idle" },
  probe: { status: "idle" },
  addOpen: false,
  local: readLocalIdentity(),
}));

// What a machine last listed is kept on this computer, so its rows survive the machine going
// away and the app restarting (`state/machineCache.ts`). A live read replaces the rows in memory,
// and this keeps the newest of them.
useMachines.subscribe((state, prev) => {
  if (state.sessions === prev.sessions) return;
  for (const [host, rows] of Object.entries(state.sessions)) {
    if (rows === prev.sessions[host]) continue;
    saveThreadList(host, rows);
    // Only a machine that answers is mirrored: the rows of one that is away are this computer's
    // own copy, not something the machine said (`state/machineMirror.ts`).
    if (state.machines.find((machine) => machine.id === host)?.state === "connected") {
      mirrorThreadList(host, rows);
    }
  }
});

// ---------------------------------------------------------------- the shell

/** Each saved machine's link, moved only through `transitionLink` (`state/machineLink.ts`). */
const links = new LinkTable();

/** Hear when a machine's connection opens or drops, so the store can route its threads there. */
const linkListeners = new Set<(id: string, connection: AcpConnection | null) => void>();
let started = false;
/** Counts asks, so an answer that arrives after Cancel is ignored. */
let asks = 0;

/**
 * A name for one ask, made here rather than by the machine: the request has to
 * be cancellable before the machine has answered, and only this page can name
 * it that early. Plain ASCII, because it travels as a command argument.
 */
export function newAskId(): string {
  asks += 1;
  return `ask-${Date.now().toString(36)}-${asks}`;
}

/**
 * The waits between first-read attempts at app start: 1, 2, 4, 8, 16 and 30
 * seconds, then the app stops trying and the Machines page reads again when it
 * is opened. The same doubling idea the supervisors use, bounded to a start.
 */
export const START_RETRY_WAITS_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];

/** The app's own wait; a test passes one that does not sleep. */
async function waitMs(ms: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
}

/**
 * Read until the shell answers: `read` says whether it did, and is called once,
 * then again after each of `START_RETRY_WAITS_MS`. Answers at the first truthy
 * read, so a shell that is up at once costs no wait.
 */
export async function readUntilAnswering(
  read: () => Promise<boolean>,
  wait: (ms: number) => Promise<void> = waitMs,
): Promise<void> {
  if (await read()) return;
  for (const ms of START_RETRY_WAITS_MS) {
    await wait(ms);
    if (await read()) return;
  }
}

/**
 * Read the saved machines once, then let each one's supervisor reach the app.
 * Called when the app starts; a machine saved later is picked up by
 * `refreshMachines`.
 *
 * The first read can fail while the shell is still coming up (the registry
 * being written, a config folder that is not there yet). Giving up there would
 * leave every paired machine without a supervisor until a page asked, which is
 * how a machine stayed away after an app update: try again instead, and stop as
 * soon as one read answers.
 */
export async function startMachines(): Promise<void> {
  if (started || !hasShell()) return;
  started = true;
  await readUntilAnswering(async () => {
    await refreshMachines();
    return !useMachines.getState().error;
  });
}

/**
 * How long the app waits on the shell before a Machines page shows an error
 * instead of a spinner. A machine that is away must never hold the page: the
 * shell reads and writes its own files, so a few seconds is generous.
 */
export const SHELL_READ_MS = 10_000;
/** Bonjour browses for a few seconds; the rest is the overlay rows. */
const DISCOVER_MS = 20_000;
/** `rc server status --json` starts a process. */
const SERVER_STATUS_MS = 15_000;
/** One task per machine runs in parallel, each with its own connect timeout. */
export const PROBE_MS = 45_000;

/** Read the machines and their live state from the shell. */
export async function refreshMachines(): Promise<void> {
  if (!hasShell()) return;
  try {
    const machines = await shellCallWithin<Machine[]>(SHELL_READ_MS, "machines_list");
    const now = Date.now();
    const since = { ...useMachines.getState().offlineSince };
    // A machine that was removed leaves nothing behind.
    for (const id of Object.keys(since)) {
      if (!machines.some((machine) => machine.id === id)) delete since[id];
    }
    for (const machine of machines) {
      if (machine.state === "connected") {
        delete since[machine.id];
        continue;
      }
      // The first time this window sees it away: the last time it answered is
      // the closest the shell gives to when it stopped.
      if (since[machine.id] === undefined) {
        const last = Date.parse(machine.lastConnectedAt ?? "");
        since[machine.id] = Number.isNaN(last) ? now : last;
      }
    }
    // A machine this run has not listed yet draws the threads this computer last read from it, so
    // an offline one keeps its rows (and a reconnect replaces them: `readThreads`).
    const state = useMachines.getState();
    const cached: Record<string, SessionSummary[]> = {};
    for (const machine of machines) {
      if (state.sessions[machine.id]) continue;
      if (machine.state === "connecting") {
        holdCopy(machine.id);
        continue;
      }
      const rows = loadThreadList(machine.id);
      if (rows.length) cached[machine.id] = rows;
    }
    for (const notice of routeChangeNotices(machines, toldRouteChanges)) toast(notice.text, notice.tone);
    useMachines.setState({
      machines,
      offlineSince: since,
      ready: true,
      error: null,
      ...(Object.keys(cached).length ? { sessions: { ...state.sessions, ...cached } } : {}),
    });
    // A machine that is away also draws what this computer's disk mirrored from it, past the 200
    // newest the web view keeps (`state/machineMirror.ts`).
    for (const machine of machines) {
      if (machine.state !== "connected" && machine.state !== "connecting") void drawMirrored(machine.id);
    }
    for (const machine of machines) void attach(machine);
    for (const [id, link] of links.entries()) {
      if (link.phase !== "removed" && !machines.some((machine) => machine.id === id)) detach(id);
    }
  } catch (error) {
    useMachines.setState({ ready: true, error: errorText(error) });
  }
}

let copyGraceMs = 5_000;

export function setCopyGraceForTests(ms: number): void {
  copyGraceMs = ms;
}
const copyHolds = new Map<string, ReturnType<typeof setTimeout>>();

function holdCopy(host: string): void {
  if (copyHolds.has(host)) return;
  copyHolds.set(
    host,
    setTimeout(() => {
      copyHolds.delete(host);
      const state = useMachines.getState();
      const machine = state.machines.find((entry) => entry.id === host);
      if (!machine || machine.state === "connected" || state.sessions[host]) return;
      const rows = loadThreadList(host);
      if (rows.length)
        useMachines.setState((current) => ({ sessions: { ...current.sessions, [host]: rows } }));
      void drawMirrored(host);
    }, copyGraceMs),
  );
}

/** Adds the mirrored rows a machine that is away lacks; a live read still replaces them all. */
async function drawMirrored(host: string): Promise<void> {
  const mirrored = await loadMirroredList(host);
  if (!mirrored.length) return;
  useMachines.setState((state) => {
    // The machine came back while the disk was read: its own list is the truth now.
    if (state.machines.find((machine) => machine.id === host)?.state === "connected") return {};
    const current = state.sessions[host] ?? [];
    const merged = withMirroredRows(current, mirrored);
    return merged.length === current.length ? {} : { sessions: { ...state.sessions, [host]: merged } };
  });
}

/**
 * A thread a client just made on a machine (`_rustcode/threads/created`). The machine's server
 * owns its threads, so its list gains the row before any `session/list`: every window drawing
 * that machine shows it at once (docs/desktop.md › Several machines). The row itself is kept by
 * the store's one keep-unlisted set (`state/store.ts`), which a later read merges too.
 */
function rememberCreated(machine: string, params: JsonObject): void {
  const id = str(params.sessionId);
  const cwd = str(params.cwd);
  if (!id || !cwd) return;
  // While this window has a `session/new` out, the creation says which thread is its own.
  if (deferAnnouncement(threadKey(machine, id), () => rememberCreated(machine, params))) return;
  // A thread trashed a moment ago is not made again by a late announcement.
  if (isTombstoned(threadKey(machine, id))) return;
  const announced = parseSessionList({ sessions: [params] }).sessions[0];
  useMachines.setState((state) => {
    const listed = state.sessions[machine] ?? [];
    if (listed.some((session) => session.id === id)) return {};
    const summary: SessionSummary = {
      id,
      cwd,
      updatedAt: Date.now(),
      title: str(params.title) || undefined,
      harness: str(params.harness) || undefined,
      model: str(params.model) || undefined,
      parent: announced?.parent,
      scheduledJob: announced?.scheduledJob,
    };
    return {
      sessions: { ...state.sessions, [machine]: [summary, ...listed] },
      counts: { ...state.counts, [machine]: listed.length + 1 },
    };
  });
}

/** A thread trashed on a machine (`_rustcode/threads/deleted`): its row leaves this window too. */
function forgetCreated(machine: string, sessionId: string | undefined): void {
  if (!sessionId) return;
  forgetMachineThread(machine, sessionId);
}

/**
 * Take a trashed thread out of a machine's list cache, whether the machine said so or this window
 * trashed it while the link was down, and refuse it to the reads that follow (a `session/list` that
 * was in flight already names it).
 */
export function forgetMachineThread(machine: string, sessionId: string): void {
  tombstoneThread(threadKey(machine, sessionId));
  forgetTranscript(threadKey(machine, sessionId));
  forgetThreadOutbox(threadKey(machine, sessionId));
  useMachines.setState((state) => {
    const listed = state.sessions[machine];
    if (!listed?.some((session) => session.id === sessionId)) return {};
    const sessions = listed.filter((session) => session.id !== sessionId);
    return {
      sessions: { ...state.sessions, [machine]: sessions },
      counts: { ...state.counts, [machine]: sessions.length },
    };
  });
}

/**
 * Give one machine a channel of its own. The supervisor inside the shell keeps
 * retrying on that channel, so it is made once and kept while the machine is
 * saved, whatever state it is in.
 */
async function attach(machine: Machine): Promise<void> {
  const transport = new MachineTransport(machine.id, machine.name);
  // Already attached, or still opening: one channel per machine.
  const attached = links.step(machine.id, { type: "attach", transport });
  if (attached.after === attached.before) return;
  try {
    await transport.start((event) => {
      if (event.type === "open") {
        const connection = new AcpConnection((line) => transport.send(line));
        // Keep the list cache current too: another machine's list refresh must not
        // overwrite a status/deadline just broadcast by this owner, and a thread
        // any client starts there must be listed before the next read.
        connection.onInbound((message) => {
          if (message.type !== "notification") return;
          if (message.method === Method.threadCreated) {
            rememberCreated(machine.id, message.params);
            return;
          }
          if (message.method === Method.threadDeleted) {
            forgetCreated(machine.id, str(message.params.sessionId));
            return;
          }
          if (message.method !== Method.sessionUpdate) return;
          const id = str(message.params.sessionId);
          const update = obj(message.params.update);
          // Session info carries the thread's title and status. Tool calls also
          // have titles, which belong to the transcript.
          if (!id || update?.sessionUpdate !== "session_info_update") return;
          const title = str(update.title);
          const meta = obj(obj(update._meta)?.rustcode);
          if (!title && !meta) return;
          const parsed = meta
            ? parseSessionList({ sessions: [{ sessionId: id, _meta: { rustcode: meta } }] }).sessions[0]
            : undefined;
          useMachines.setState((state) => ({
            sessions: {
              ...state.sessions,
              [machine.id]: (state.sessions[machine.id] ?? []).map((session) => {
                if (session.id !== id) return session;
                return {
                  ...session,
                  ...(title ? { title } : {}),
                  ...(parsed?.status ? { status: parsed.status } : {}),
                  ...(parsed?.snoozedUntil !== undefined ? { snoozedUntil: parsed.snoozedUntil } : {}),
                };
              }),
            },
          }));
        });
        const { before, after } = links.step(machine.id, { type: "opened", transport, connection });
        // A channel of a machine since removed, or one a newer attach replaced: nothing to route.
        if (after === before) {
          connection.close("The machine was removed.");
          return;
        }
        markMachine(machine.id, after, null);
        announce(machine.id, connection);
        void readThreads(machine.id, connection);
      } else if (event.type === "line") {
        links.connectionOf(machine.id, transport)?.receive(event.line);
      } else if (event.type === "lines") {
        const connection = links.connectionOf(machine.id, transport);
        for (const line of event.lines) connection?.receive(line);
      } else if (event.type === "closed") {
        const { before, after } = links.step(machine.id, { type: "closed", transport });
        if (after === before) return;
        linkConnection(before)?.close(event.reason);
        // The sidebar and the chat say the same as the shell's supervisor: it is away, read-only.
        markMachine(machine.id, after, event.reason || null);
        announce(machine.id, null);
        useMachines.setState((state) => {
          const counts = { ...state.counts };
          delete counts[machine.id];
          return { counts };
        });
      }
    });
    links.step(machine.id, { type: "started", transport });
    // Removed while the channel opened: stop the supervisor that start just made.
    if (links.get(machine.id)?.phase === "removed") void transport.stop();
  } catch (error) {
    links.step(machine.id, { type: "failed", transport });
    useMachines.setState({ error: errorText(error) });
  }
}

/**
 * What the supervisor saw: the row, the sidebar group and the chat read it from here. A pairing
 * the shell says was revoked stays revoked, and a supervisor still backing off keeps the state and
 * retry clock `machines_list` gave it: only a machine that was connected turns offline here, and
 * nothing but `machines_list` writes `retryInSecs`.
 */
function markMachine(id: string, link: LinkState | undefined, detail: string | null): void {
  const state = linkMachineState(link);
  if (!state) return;
  useMachines.setState((current) => ({
    machines: current.machines.map((machine) => {
      if (machine.id !== id) return machine;
      if (state === "connected")
        return { ...machine, state, detail: null, lastConnectedAt: new Date().toISOString() };
      if (machine.state !== "connected") return machine;
      return { ...machine, state: "offline", detail: detail ?? machine.detail };
    }),
  }));
}

/**
 * Tests only: drop every channel and the lists they gave, as a freshly started app would. The
 * app's own store calls this through `resetForTests` (`state/store.ts`), so one test file's
 * machines never reach another's.
 */
export function resetMachinesForTests(): void {
  for (const [id, link] of links.entries()) {
    if (link.phase === "removed") continue;
    linkConnection(link)?.close("The app reset its machines.");
    void link.transport.stop();
    announce(id, null);
  }
  links.clear();
  for (const timer of copyHolds.values()) clearTimeout(timer);
  copyHolds.clear();
  linkListeners.clear();
  started = false;
  useMachines.setState({
    ready: false,
    error: null,
    machines: [],
    offlineSince: {},
    counts: {},
    sessions: {},
    discovered: null,
    connect: { status: "idle" },
    ssh: { status: "idle" },
    discovery: { status: "idle" },
    pairing: { status: "idle" },
    server: null,
    serverInstall: { status: "idle" },
    serverLink: { status: "idle" },
    probe: { status: "idle" },
    addOpen: false,
  });
}

/** Stop a machine's channel: the shell stops its supervisor with it. */
function detach(id: string): void {
  // A machine that is removed leaves nothing of its threads on this computer, and nothing waits
  // to be sent to it.
  forgetMachineCache(id);
  forgetMachineOutbox(id);
  // Nothing of it is listed any more, and that is the whole truth: what waited on its threads goes.
  noteThreadList(id, true);
  const { before: link } = links.step(id, { type: "remove" });
  if (!link || link.phase === "removed") return;
  linkConnection(link)?.close("The machine was removed.");
  announce(id, null);
  // A channel still opening is stopped here too, and again by `attach` once its start answers.
  void link.transport.stop();
  useMachines.setState((state) => {
    const counts = { ...state.counts };
    const sessions = { ...state.sessions };
    delete counts[id];
    delete sessions[id];
    return { counts, sessions };
  });
}

/**
 * The machine's threads, from its own `session/list`. The count is a nicety: a
 * machine that does not answer it is still listed as connected. `initialize`
 * is asked once per connection, so re-listing (a focus, a reconnect) is one
 * request.
 */
async function readThreads(id: string, connection: AcpConnection): Promise<void> {
  try {
    const link = links.get(id);
    if (!(link?.phase === "ready" && link.connection === connection)) {
      // A machine's server announces its threads to the clients that list them
      // (`_rustcode/threads/created`, docs/desktop.md › Several machines).
      await connection.request("initialize", machineInitializeParams(), 10_000);
      // Only this connection is marked: an answer from one a reconnect replaced is dropped.
      links.step(id, { type: "initialized", connection });
    }
    const sessions: SessionSummary[] = [];
    let cursor: string | undefined;
    for (let page = 0; page < 10; page++) {
      const answer = await connection.request("session/list", cursor ? { cursor } : {}, 10_000);
      const result = parseSessionList(answer);
      sessions.push(...result.sessions);
      cursor = result.nextCursor;
      if (!cursor) break;
    }
    if (connection.isClosed) return;
    // A cursor left over is a list that stopped at its cap: a thread missing from it is not gone.
    noteThreadList(id, !cursor);
    // Rows the app keeps that this read does not name yet stay: a snapshot from before a thread
    // was announced is not news that it is gone. The ones it names now need no keeping.
    // A thread trashed while this read was out is not news that it came back.
    for (let i = sessions.length - 1; i >= 0; i--) {
      if (isTombstoned(threadKey(id, sessions[i]?.id ?? ""))) sessions.splice(i, 1);
    }
    // What the user changed here while the machine was away goes to it now; a change this list
    // shows the machine overrides is dropped first, so the rows below draw the machine's state.
    void flushMachineOutbox(id, connection, sessions);
    const listed = new Set(sessions.map((session) => session.id));
    settleKeptUnlisted(id, listed);
    const kept = keptUnlisted(id, listed).map((summary) => ({
      ...summary,
      id: splitThreadKey(summary.id).id,
      host: undefined,
    }));
    useMachines.setState((state) => {
      // The kept summary was stored with no status. The cache row has the one a status broadcast
      // wrote (a thread another client announced is running), so keep that, or the row reads idle
      // and its keep expires while the thread still runs.
      const cached = state.sessions[id] ?? [];
      const rows = [
        ...sessions,
        ...kept.map((summary) => {
          const status = cached.find((row) => row.id === summary.id)?.status;
          return status ? { ...summary, status } : summary;
        }),
      ];
      return {
        counts: { ...state.counts, [id]: rows.length },
        sessions: { ...state.sessions, [id]: rows },
      };
    });
  } catch {
    // An older machine, or one whose ACP is not allowed yet: no count.
  }
}

// ---------------------------------------------------------------- live links

/** One saved machine and the ACP connection its supervisor currently holds. */
export interface MachineLink {
  machine: Machine;
  connection: AcpConnection | null;
}

/** The connection to one machine, while it is open. */
export function machineConnection(id: string): AcpConnection | null {
  return linkConnection(links.get(id));
}

/** Every saved machine with its connection, in the sidebar's order. */
export function machineLinks(): MachineLink[] {
  return useMachines.getState().machines.map((machine) => ({
    machine,
    connection: linkConnection(links.get(machine.id)),
  }));
}

/**
 * Hear when a machine's connection opens or drops. The store listens to route
 * that machine's thread operations over its own connection (state/store.ts
 * `connectionFor`), and to keep its threads listed while it is away.
 */
export function onMachineLinks(listener: (id: string, connection: AcpConnection | null) => void): () => void {
  linkListeners.add(listener);
  return () => linkListeners.delete(listener);
}

function announce(id: string, connection: AcpConnection | null): void {
  for (const listener of linkListeners) listener(id, connection);
}

/** The least time between two wakes the app sends on its own (a focus, the network coming back). */
const RETRY_GAP_MS = 5_000;
let lastRetryAt = 0;

/**
 * Ask the shell to try machines again now instead of at the end of their backoff: one machine, or
 * every machine waiting when none is named. Answers the ids woken; a machine that is connected,
 * connecting or needs pairing is left alone.
 */
export async function retryMachines(id?: string): Promise<string[]> {
  if (!hasShell()) return [];
  try {
    const woken = await shellCallWithin<string[]>(SHELL_READ_MS, "machines_retry", id ? { id } : {});
    if (woken.length) await refreshMachines();
    return woken;
  } catch {
    return [];
  }
}

/** Whether any saved machine is waiting out a backoff that a wake would end. */
function anyWaiting(): boolean {
  return useMachines
    .getState()
    .machines.some((machine) => machine.state === "offline" || machine.state === "unavailable");
}

// The window coming to the front, or the network coming back, is when a machine that went away is
// most likely back too, and when the person is looking: try it now, not at the end of the wait.
if (typeof window !== "undefined") {
  const wake = () => {
    const now = Date.now();
    if (!started || !anyWaiting() || now - lastRetryAt < RETRY_GAP_MS) return;
    lastRetryAt = now;
    void retryMachines();
  };
  window.addEventListener("focus", wake);
  window.addEventListener("online", wake);
  if (typeof document !== "undefined")
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") wake();
    });
}

/** Re-read every connected machine's thread list (a window focus, a reconnect). */
export function refreshMachineThreads(): void {
  for (const [id, link] of links.entries()) {
    const connection = linkConnection(link);
    if (connection) void readThreads(id, connection);
  }
}

// ---------------------------------------------------------------- actions

/** Look for machines on this network (Bonjour, a few seconds). */
export async function discoverMachines(): Promise<void> {
  useMachines.setState({ discovery: { status: "running" } });
  try {
    const { rows, notice } = await shellCallWithin<Discovery>(DISCOVER_MS, "machines_discover");
    // A failed browse still lists the overlay rows: the notice says why the
    // local search found nothing, and the saved machines are below it.
    useMachines.setState({
      discovered: mergeDiscovered(rows),
      discovery: notice ? { status: "failed", error: notice } : { status: "idle" },
    });
  } catch (error) {
    useMachines.setState({ discovery: { status: "failed", error: errorText(error) } });
  }
}

/**
 * This computer's RustCode server as a background service. The page asks when
 * it opens and after an install; nothing polls it, because reading it starts
 * a process (`rc server status --json`).
 */
export async function refreshServerService(): Promise<void> {
  if (!hasShell()) return;
  try {
    useMachines.setState({
      server: await shellCallWithin<ServerService>(SERVER_STATUS_MS, "server_service_status"),
    });
  } catch (error) {
    useMachines.setState({
      server: {
        installed: false,
        running: false,
        version: null,
        cliVersion: null,
        gateway: emptyGateway(),
        pathRustcode: null,
        problems: [],
        error: errorText(error),
      },
    });
  }
}

/**
 * Install the server as a background service: it stays up when the app closes
 * and starts when you log in, so threads that live in it keep running. The
 * app's own threads stay in its engine until the app is switched to the server. It is the
 * CLI's own `rc server install`, so a development build is refused there
 * with the reason shown here. Answers whether it ended up installed.
 */
export async function installServer(): Promise<boolean> {
  try {
    const server = await runOp("serverInstall", async () => {
      const server = await shellCall<ServerService>("server_service_install");
      useMachines.setState({ server });
      return server;
    });
    if (server.installed) {
      toast(
        "The RustCode server is installed as a background service and starts when you log in. It runs the remote gateway too, while Remote access is on.",
        "success",
      );
    } else {
      toast(server.error ?? "The RustCode server is installed but is not running yet.", "error");
    }
    return server.installed;
  } catch (error) {
    toast(`Could not install the RustCode server: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Point the `rustcode` a shell runs at this app's engine (issue 3411): the
 * CLI's own `rc server link-path`, run by the app's engine so it is the
 * app's version that the shell then finds. Homebrew's copy is refused with the
 * `brew` command. Answers whether the page now reads no version mismatch.
 */
export async function linkCliToApp(): Promise<boolean> {
  try {
    const server = await runOp("serverLink", async () => {
      const server = await shellCall<ServerService>("server_link_path");
      useMachines.setState({ server });
      return server;
    });
    const stale = server.problems.some((problem) => problem.code.startsWith("path-version-mismatch"));
    if (stale) {
      toast("The `rustcode` on PATH still is not this app's version. See the warning on this page.", "error");
    } else {
      toast(
        "`rustcode` in a terminal now runs this app's version. Open a new terminal to use it.",
        "success",
      );
    }
    return !stale;
  } catch (error) {
    toast(`Could not point \`rustcode\` at this app: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Ask before installing the server, in the words that say what it does: it is
 * where threads can live instead of the app, it stays up when the app closes,
 * it starts when you log in, and it runs the remote gateway. The app's own
 * threads are still its engine's.
 */
export function askInstallServer(): void {
  askConfirm({
    title: "Install the RustCode server?",
    message:
      "The RustCode server is where threads can live instead of the app: as a background service it stays up when the app closes and starts when you log in, so those threads keep running. It also runs the remote gateway while Remote access is on, so paired devices still reach this computer while the app is closed. The threads this app starts itself still run in its own engine and end with it, until RustCode uses the server. Threads, settings and pairings stay as they are.",
    action: "Install",
    run: () => void installServer(),
  });
}

/** A gateway this page has no reading for yet. */
function emptyGateway(): GatewayService {
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

/** Who started the gateway, in the words the page shows. */
export function gatewayOwnerLabel(owner: GatewayService["owner"]): string {
  switch (owner) {
    case "server":
      return "the RustCode server";
    case "app":
      return "the app";
    case "terminal":
      return "a terminal";
    default:
      return "an unknown process";
  }
}

/**
 * The gateway in one line: where paired devices reach this computer, and who
 * keeps it there. With the server installed and remote access on, that is the
 * server, so the computer stays reachable when the app closes. `serverInstalled`
 * is what makes "the server starts it" honest: with no server installed,
 * nothing restarts it but `rc remote start`.
 */
export function gatewayLine(gateway: GatewayService | undefined, serverInstalled = false): string {
  if (!gateway) return "Not checked yet";
  if (gateway.running) {
    const where = gateway.address ?? "an unknown address";
    return `Running on ${where} (started by ${gatewayOwnerLabel(gateway.owner)})`;
  }
  if (gateway.remoteEnabled) {
    const who = serverInstalled ? "the server starts it" : "`rc remote start` starts it";
    return gateway.port ? `Not running · ${who} on port ${gateway.port}` : `Not running · ${who}`;
  }
  return "Off";
}

/** The background server in one line: `Installed · Running`, or what is wrong. */
export function serverLine(server: ServerService | null): string {
  if (!server) return "Not checked yet";
  if (server.error && !server.installed && !server.running) return server.error;
  const installed = server.installed ? "Installed" : "Not installed";
  if (!server.installed) return installed;
  return `${installed} · ${server.running ? "Running" : "Not running"}`;
}

/**
 * Ask `address` to let this app in. The request appears on that machine for
 * its owner to accept; once accepted, the code it shows is what pairs it.
 *
 * The shell holds the socket for the whole exchange, so it streams what is
 * happening there: waiting for the owner, then accepted (the code is next).
 * A denial or a request nobody answered comes back as this call's error, in
 * words the page shows where "Waiting for ‹host›…" was.
 */
export async function askMachine(address: string, name?: string): Promise<void> {
  const target = address.trim();
  if (!target) return;
  // One ask at a time: a second press drops the first request (and its socket)
  // instead of leaving two in front of two machines' owners.
  if (useMachines.getState().pairing.data) cancelPairing();
  const host = name?.trim() || target;
  const askId = newAskId();
  const asked = { address: target, host, askId, since: Date.now() };
  useMachines.setState({ pairing: { status: "running", data: { stage: "waiting", ...asked } } });
  const current = () => useMachines.getState().pairing.data?.askId === askId;
  const waiting = (event: PairingEvent) => {
    if (!current()) return;
    if (event.type === "approved")
      useMachines.setState({
        pairing: {
          status: "running",
          data: {
            stage: "code",
            ...asked,
            codeLength: event.codeLength,
            expiresAt: Date.now() + event.expiresInSecs * 1000,
          },
        },
      });
  };
  try {
    let answer: AskView;
    if (isTauri()) {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<PairingEvent>();
      channel.onmessage = waiting;
      answer = await invoke<AskView>("machines_ask", {
        askId,
        address: target,
        name: name ?? null,
        onEvent: channel,
      });
    } else {
      answer = await shellCall<AskView>("machines_ask", {
        askId,
        address: target,
        name: name ?? null,
      });
    }
    // Without a channel (the mock, `dev:web`) the answer is where the code stage begins.
    waiting({ type: "approved", ...answer });
  } catch (error) {
    // Cancelled: the request was the point, and its refusal is not the user's.
    if (!current()) return;
    useMachines.setState({ pairing: { status: "failed", error: errorText(error) } });
  }
}

/** What the shell streams while a machine is asked. */
type PairingEvent =
  | { type: "waiting"; askId: string; host: string }
  | { type: "approved"; askId: string; codeLength: number; expiresInSecs: number }
  | { type: "refused"; message: string };

/**
 * Give up on a request. The shell is told by its ask id, which it has had since
 * before the machine was dialled: a request the owner has not answered yet
 * loses its socket then and there, and an accepted one loses the one waiting
 * for its code.
 */
export function cancelPairing(): void {
  const pairing = useMachines.getState().pairing.data;
  useMachines.setState({ pairing: { status: "idle" } });
  if (pairing) void shellCall("machines_cancel", { askId: pairing.askId }).catch(() => undefined);
}

/** Send the code the machine shows. */
export async function pairMachine(code: string): Promise<void> {
  const pairing = useMachines.getState().pairing.data;
  if (pairing?.stage !== "code") return;
  useMachines.setState({
    pairing: { status: "running", data: { ...pairing, stage: "pairing", since: Date.now() } },
  });
  try {
    const machine = await shellCall<Machine>("machines_pair", {
      askId: pairing.askId,
      code,
    });
    useMachines.setState((state) => ({
      pairing: { status: "idle" },
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
  } catch (error) {
    // The request is spent either way; asking again is what recovers it.
    useMachines.setState({ pairing: { status: "failed", error: errorText(error) } });
  }
}

/** Forget a machine, after asking: its token and its routes go with it. */
export function removeMachine(machine: Machine, onRemoved?: () => void): void {
  const name = machineName(machine);
  askConfirm({
    title: `Remove ${name}?`,
    message:
      `${name} is forgotten on this computer: its pairing token and routes are deleted, and its threads are no longer listed here. ` +
      "It keeps listing this computer until you revoke it there.",
    action: "Remove",
    destructive: true,
    run: () => {
      void shellCall("machines_remove", { id: machine.id }).then(
        async () => {
          detach(machine.id);
          void import("./settingsFollow").then((follow) => follow.forgetFollower(machine.id));
          toast(`Removed ${name}.`, "success");
          await refreshMachines();
          onRemoved?.();
        },
        (error) => toast(errorText(error), "error"),
      );
    },
  });
}

/** A probe in words: `Reachable · 4 ms`, `Refused · 2 ms`, `Timed out`, `Not tested`. */
export function probeLabel(probe: RouteProbe | null | undefined): string {
  if (!probe) return "Not tested";
  const ms = probe.latencyMs === null ? "" : ` · ${probe.latencyMs} ms`;
  switch (probe.outcome) {
    case "reachable":
      return `Reachable${ms}`;
    case "refused":
      return `Refused${ms}`;
    case "timeout":
      return "Timed out";
    default:
      return "Unreachable";
  }
}

/** The chip tone of a probe: green when it answered, red when it did not, none before a test. */
export function probeTone(probe: RouteProbe | null | undefined): "ok" | "bad" | "idle" {
  if (!probe) return "idle";
  return probe.outcome === "reachable" ? "ok" : "bad";
}

/**
 * Test every route of one machine (every machine when none is named) with a
 * plain connect, and show what each found. Answers whether the test ran.
 */
export async function probeMachineRoutes(machine?: Pick<Machine, "id">): Promise<boolean> {
  try {
    const machines = await runOp("probe", () =>
      shellCallWithin<Machine[]>(PROBE_MS, "machines_probe", machine ? { id: machine.id } : {}),
    );
    useMachines.setState({ machines });
    return true;
  } catch (error) {
    toast(`Could not test the routes: ${errorText(error)}`, "error");
    return false;
  }
}

/** How a machine paired over SSH is reached: its network address with SSH as the fallback, or SSH only. */
export type SshVia = "auto" | "ssh";

/** Pause new threads on a machine, or let them start there again. */
export async function setMachinePaused(machine: Pick<Machine, "id">, paused: boolean): Promise<void> {
  // The row answers at once, and the shell's own read settles the value.
  useMachines.setState((state) => ({
    machines: state.machines.map((entry) => (entry.id === machine.id ? { ...entry, paused } : entry)),
  }));
  try {
    await shellCallWithin(SHELL_READ_MS, "machines_set_paused", { id: machine.id, paused });
    await refreshMachines();
  } catch (error) {
    await refreshMachines();
    toast(errorText(error), "error");
  }
}

/** Open or close the Add machine dialog, which lives on Settings › Machines. */
export function setAddMachineOpen(open: boolean): void {
  useMachines.setState({ addOpen: open });
}

/** Clear what the last attempt to connect with a code said, so the dialog starts clean. */
export function clearConnectError(): void {
  if (useMachines.getState().connect.status === "failed")
    useMachines.setState({ connect: { status: "idle" } });
}

/** Close a machine's connection without forgetting it; Run on can still use it another time. */
export async function disconnectMachine(machine: Pick<Machine, "id">): Promise<void> {
  try {
    await shellCallWithin(SHELL_READ_MS, "machine_disconnect", { id: machine.id });
    await refreshMachines();
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/**
 * Ask a machine to try again now. The shell stops its supervisor and starts a new
 * one, so a backoff that had grown to minutes is a fresh attempt; the app keeps
 * its channel and reopens it as the machine answers. A machine the shell is not
 * keeping (it was saved after the last read) is picked up by reading the list.
 */
export function reconnectMachine(machine: Pick<Machine, "id">): void {
  const link = links.get(machine.id);
  if (!link || link.phase === "removed" || link.phase === "opening") {
    void refreshMachines();
    return;
  }
  // The row says it is trying while it asks, rather than staying offline until an answer.
  useMachines.setState((state) => ({
    machines: state.machines.map((entry) =>
      entry.id === machine.id ? { ...entry, state: "connecting", detail: null, retryInSecs: null } : entry,
    ),
  }));
  linkConnection(link)?.close("Reconnecting.");
  links.step(machine.id, { type: "reconnect" });
  void link.transport
    .restart()
    .then(() => refreshMachines())
    .catch((error) => {
      useMachines.setState((state) => ({
        machines: state.machines.map((entry) =>
          entry.id === machine.id ? { ...entry, state: "offline", detail: errorText(error) } : entry,
        ),
      }));
    });
}
