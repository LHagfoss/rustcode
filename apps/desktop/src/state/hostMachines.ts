// The machines an orchestration call can act on: resolving one, listing its projects, harnesses and threads, and starting threads or fan-outs on it. The store and host.ts import this module; nothing here touches either at load time.
import { type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { type Harness, type HarnessSnapshot, Method, plainPath, projectName } from "../acp/types";
import { currentPlatform } from "../lib/platform";
import { titleFrom } from "../lib/title";
import { capacityRow, useFleet } from "./fleet";
import { unavailableReason } from "./harness";
import { book, delegateLimits, permitted, ruleThread, setBook } from "./host";
import { addLink, deliver, type FanOutCall, type StartCall } from "./hostManage";
import { FILTERED_MODELS, LISTED_MODELS, listProjects, MODEL_MISS_HINT } from "./hostReads";
import { accessOf, childMode, choiceLabel, deliverOwn, narrowed, ownMode, ranWith } from "./hostStart";
import { accountSignedOut } from "./logins";
import {
  LOCAL_MACHINE_ID,
  localAppVersion,
  localIdentity,
  type Machine,
  machineName,
  offlineSinceLabel,
  probeMachineRoutes,
  refreshMachines,
  retryMachines,
  selfName,
  useMachines,
} from "./machines";
import {
  checkStart,
  delegateAccountKey,
  delegated,
  type Failure,
  type FanOut,
  failure,
  failureJson,
  isFailure,
  type Link,
  linksLeft,
  listSessions,
  MAX_PROJECTS,
  projectOf,
  type RuleThread,
  sameProject,
} from "./orchestration";
import {
  isDataFolder,
  isRootFolder,
  type Project,
  projectByRepository,
  projectsOn,
  remoteRepository,
} from "./projects";
import {
  connectionFor,
  createSession,
  ensureHarnessModels,
  errorText,
  harnessesOn,
  loadMachineHarnesses,
  setPermissionMode,
  useApp,
} from "./store";
import { isRunning, localId } from "./thread";
import { LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";

// ---------------------------------------------------------------- machines

/**
 * The computer a call acts on: this one, or a machine the app is paired with
 * (`state/machines.ts`). `machine` is an id or name from `list_machines`; a
 * call without one is this computer, never a "main" machine.
 */
export interface MachineRef {
  /** `""` for this computer, else the machine's id (`state/threadKeys.ts`). */
  host: string;
  /** What `list_machines` calls it: `local`, or the machine's id. */
  id: string;
  name: string;
  /** `macos`, `windows`, `linux`, or `unknown`. */
  platform: string;
}

/** This computer, as a call names it (`list_machines` `current: true`). */
export function localMachine(): MachineRef {
  const platform = currentPlatform();
  return {
    host: LOCAL_HOST,
    id: LOCAL_MACHINE_ID,
    name: selfName(),
    platform: platform === "mac" ? "macos" : platform,
  };
}

/** A paired machine, as a call names it: the name this app shows, never the reported one. */
function machineRef(machine: Machine): MachineRef {
  return { host: machine.id, id: machine.id, name: machineName(machine), platform: machine.platform };
}

/**
 * The machine a `machine` argument names: `defaultHost` (the caller's own
 * computer) when it is absent, else this computer (`local` or its name) or a
 * paired machine by id or name, any case. A name is what the user says
 * ("Studio"); an id is what a previous answer gave. Several machines with one
 * name are refused rather than guessed between.
 */
export function resolveMachine(
  asked: string | undefined,
  defaultHost: string = LOCAL_HOST,
): MachineRef | Failure {
  const wanted = asked?.trim().toLowerCase();
  if (!wanted) return machineOfHost(defaultHost);
  const local = localMachine();
  if (wanted === LOCAL_MACHINE_ID || wanted === local.name.toLowerCase()) return local;
  const machines = useMachines.getState().machines;
  const byId = machines.filter((machine) => machine.id.toLowerCase() === wanted);
  // The name shown is a local display name; the reported one still names it,
  // so a pairing saved before a rename answers to either.
  const found = byId.length
    ? byId
    : machines.filter(
        (machine) => machineName(machine).toLowerCase() === wanted || machine.name.toLowerCase() === wanted,
      );
  if (!found.length) {
    const names = [local.name, ...machines.map(machineName)];
    return failure("not_found", `no machine “${asked}”; list_machines has ${names.join(", ")}`);
  }
  if (found.length > 1) {
    return failure(
      "invalid",
      `“${asked}” names ${found.length} machines; pass the id: ${found.map((machine) => machine.id).join(", ")}`,
    );
  }
  return machineRef(found[0]!);
}

/**
 * Whether a call can act on `machine` now. An offline one says so by name and
 * when it went away, never silently falls back to this computer.
 */
export function machineUnavailable(machine: MachineRef): Failure | null {
  if (!machine.host) {
    return connectionFor(LOCAL_HOST)
      ? null
      : failure("unavailable", "this computer's RustCode server is not connected; nothing was sent");
  }
  if (connectionFor(machine.host)) return null;
  const saved = useMachines.getState().machines.find((m) => m.id === machine.host);
  if (saved?.state === "needsPairing") {
    return failure(
      "offline",
      `${machine.name} is no longer paired with this computer; pair it again in Settings › Machines. Nothing was sent`,
    );
  }
  const since = offlineSinceLabel(useMachines.getState().offlineSince[machine.host]);
  // A machine that answered and refused says what it said: "offline" would send the agent to check
  // a network that works.
  if (saved?.state === "unavailable") {
    return failure(
      "offline",
      `${machine.name} answers but is not serving this computer${saved.detail ? `: ${saved.detail}` : ""}; nothing was sent`,
    );
  }
  return failure("offline", `${machine.name} is offline${since ? ` since ${since}` : ""}; nothing was sent`);
}

/**
 * Where a machine stands, as `list_machines` says it: `connected`, `connecting`, `reconnecting`
 * (no route answered; the app is on its retry clock), `unavailable` (a route answered and the
 * machine would not serve: `cause` says why), `revoked` (it refused this computer's pairing), or
 * `offline` (not connected and not trying).
 */
function machineState(saved: Machine | undefined, connected: boolean): string {
  if (connected) return "connected";
  switch (saved?.state) {
    case "needsPairing":
      return "revoked";
    case "connecting":
      return "connecting";
    case "unavailable":
      return "unavailable";
    case "offline":
      return saved.retryInSecs ? "reconnecting" : "offline";
    default:
      return "offline";
  }
}

/** What every machine-aware answer carries, so an agent can tell where a thread runs. */
export function machineFields(machine: MachineRef): JsonObject {
  return { machine: machine.name, machine_id: machine.id };
}

/** A host key ("" is this computer), as an answer about a thread there names it. */
export function machineOfHost(host: string): MachineRef {
  if (!host) return localMachine();
  const saved = useMachines.getState().machines.find((m) => m.id === host);
  return {
    host,
    id: host,
    name: useApp.getState().machineNames[host] ?? saved?.name ?? host,
    platform: saved?.platform ?? "unknown",
  };
}

/** The machine a thread key names: an answer about a keyed thread says where it runs. */
export const machineOfThread = (id: string) => machineOfHost(splitThreadKey(id).host);

/** The one machine a wait's links act on, when they all do. */
export function waitMachine(links: Link[]): MachineRef | undefined {
  const hosts = new Set(links.map((link) => splitThreadKey(link.target).host));
  return hosts.size === 1 ? machineOfHost([...hosts][0]!) : undefined;
}

/**
 * A thread id as the named machine knows it: an id that already carries a
 * machine is kept (and must be that machine's), and a bare one is prefixed
 * with the machine the call named.
 */
export function scopedTarget(machine: MachineRef, target: string): string | Failure {
  const { host } = splitThreadKey(target);
  if (host) {
    if (machine.host && machine.host !== host) {
      const other = useApp.getState().machineNames[host] ?? host;
      return failure("invalid", `“${target}” runs on ${other}, not ${machine.name}`);
    }
    return target;
  }
  return threadKey(machine.host, target);
}

/** A machine and the connection a call on it needs, or the failure to answer with. */
export function checkedMachine(
  asked: string | undefined,
  defaultHost: string = LOCAL_HOST,
): MachineRef | Failure {
  const machine = resolveMachine(asked, defaultHost);
  if (isFailure(machine)) return machine;
  return machineUnavailable(machine) ?? machine;
}

/**
 * The machine and thread key a call on one thread acts on: the machine the
 * call names, else the one the id carries, else `defaultHost` (the caller's
 * machine). A connected machine only, so a call never lands on the wrong
 * computer.
 */
export function machineThread(
  asked: string | undefined,
  target: string,
  defaultHost: string = LOCAL_HOST,
): { machine: MachineRef; target: string } | Failure {
  const targetHost = splitThreadKey(target).host;
  const carried = targetHost ? machineOfHost(targetHost) : machineOfHost(defaultHost);
  const machine = asked?.trim()
    ? checkedMachine(asked, defaultHost)
    : (machineUnavailable(carried) ?? carried);
  if (isFailure(machine)) return machine;
  const key = scopedTarget(machine, target);
  if (isFailure(key)) return key;
  return { machine, target: key };
}

/**
 * The same for a call that only touches this app's own record of the thread
 * (its queue): the machine may be away, and the thread is still named by it.
 */
export function threadNamedOn(
  asked: string | undefined,
  target: string,
  defaultHost: string = LOCAL_HOST,
): { machine: MachineRef; target: string } | Failure {
  const targetHost = splitThreadKey(target).host;
  const named = asked?.trim()
    ? resolveMachine(asked, defaultHost)
    : targetHost
      ? machineOfHost(targetHost)
      : machineOfHost(defaultHost);
  if (isFailure(named)) return named;
  const key = scopedTarget(named, target);
  if (isFailure(key)) return key;
  return { machine: named, target: key };
}

/**
 * The thread a management call acts on, on the machine the call names or its
 * id carries: `undefined` is the caller itself, on `defaultHost`.
 */
export function managedOn(
  asked: string | undefined,
  target: string | undefined,
  defaultHost: string = LOCAL_HOST,
): { machine: MachineRef; target: string | undefined } | Failure {
  if (!target) {
    const machine = checkedMachine(asked, defaultHost);
    if (isFailure(machine)) return machine;
    if (machine.host !== defaultHost) {
      return failure("invalid", `session_id is required for a thread on ${machine.name}`);
    }
    return { machine, target };
  }
  return machineThread(asked, target, defaultHost);
}

/** One request to the engine of `host`: this computer's, or a paired machine's over its connection. */
export async function requestOn<T = Json>(
  host: string,
  method: string,
  params: JsonObject,
  timeoutMs: number,
): Promise<T> {
  const conn = connectionFor(host);
  if (!conn) throw new Error(host ? "that machine is not connected" : "the engine is not connected");
  return conn.request<T>(method, params, timeoutMs);
}

/**
 * Each folder's repository (`remoteRepository` of its `origin`), per machine:
 * `null` for a folder with none. Only an answer is kept, so a read that failed
 * is tried again by the next call.
 */
export const repositories = new Map<string, string | null>();
/** How long one folder's git read may take while a call matches projects. */
const REPOSITORY_READ_MS = 10_000;

/** The repository a folder on `host` is a checkout of, or `null`; `undefined` when it could not be read. */
async function repositoryOf(host: string, path: string): Promise<string | null | undefined> {
  const key = `${host}\n${path}`;
  if (repositories.has(key)) return repositories.get(key);
  try {
    const facts = await requestOn<JsonObject>(host, Method.projectRead, { path }, REPOSITORY_READ_MS);
    const repository = remoteRepository(str(facts?.remote));
    repositories.set(key, repository);
    return repository;
  } catch {
    return undefined;
  }
}

/**
 * Folders cloned on a paired machine by `clone_project` here: its thread list
 * names a project only once a thread runs there, and a clone is where the
 * next thread starts.
 */
export const clonedOn = new Map<string, string[]>();

/** For tests: forget every folder's repository and every clone made on a machine. */
export function forgetMachineProjectsForTests(): void {
  repositories.clear();
  clonedOn.clear();
}

/** A machine's projects: what its threads name, newest first, then what was cloned there from here. */
export function machineProjects(host: string): Project[] {
  const listed = projectsOn(useApp.getState().sessions, host);
  const cloned = (clonedOn.get(host) ?? [])
    .filter((path) => !listed.some((project) => project.path === path))
    .map((path) => ({ path, name: projectName(path) }));
  return [...listed, ...cloned];
}

/** Models `list_machines` names per harness: enough to choose a machine by, not the catalogue. */
const MACHINE_LISTED_MODELS = 8;

/** The machine's last capacity report from the fleet sync, as a `capacity` field. */
function withCapacity(machine: MachineRef): JsonObject {
  const fleet = useFleet.getState();
  const report = machine.host
    ? fleet.entries.find((entry) => entry.hostId === machine.id)?.capacity
    : fleet.host?.capacity;
  const row = capacityRow(report);
  return row ? { capacity: row } : {};
}

/**
 * A machine as `list_machines` describes it: what it is, where it stands and
 * why, how busy it is, and what it has to run a thread with and in. An agent
 * choosing where to delegate reads this once instead of calling
 * `list_harnesses` and `list_projects` on every machine.
 */
async function machineEntry(machine: MachineRef, callerHost: string = LOCAL_HOST): Promise<JsonObject> {
  const saved = useMachines.getState().machines.find((m) => m.id === machine.id);
  const connected = !!connectionFor(machine.host);
  const state = machineState(saved, connected);
  const version = machine.host ? (saved?.version ?? null) : await localAppVersion();
  const st = useApp.getState();
  const threads = st.sessions.filter((x) => (x.host ?? LOCAL_HOST) === machine.host);
  const running = threads.filter((x) => isRunning(st.threads[x.id]) || x.status === "running").length;
  const waiting = threads.reduce((n, x) => n + (st.threads[x.id]?.permissions.length ?? 0), 0);
  return {
    id: machine.id,
    // The reported name stays here, so a client that never heard of the local
    // one still says what the machine calls itself; display_name is what the
    // app shows everywhere, and `name` above is that too for a call.
    name: saved?.name ?? machine.name,
    display_name: saved?.displayName ?? (machine.host ? null : localIdentity().displayName),
    icon: saved?.icon ?? (machine.host ? null : localIdentity().icon),
    icon_tint: saved?.iconTint ?? (machine.host ? null : localIdentity().tint),
    ...(saved?.paused ? { paused: true } : {}),
    platform: machine.platform,
    ...(version ? { version } : {}),
    ...withCapacity(machine),
    ...(machine.host === callerHost ? { current: true } : {}),
    state,
    // Why it is not connected, in the words Settings › Machines shows, as a word to branch on
    // (`cause`), and when it tries again.
    ...(!connected && machine.host
      ? {
          reason:
            [saved?.detail, saved?.hint].filter(Boolean).join(" ") ||
            (saved?.paused ? "paused" : "not connected"),
          ...(saved?.reason ? { cause: saved.reason } : {}),
          ...(saved?.retryInSecs != null ? { retry_in_secs: saved.retryInSecs } : {}),
        }
      : {}),
    ...(saved?.route
      ? { route: { kind: saved.route.kind, address: saved.route.address, encrypted: saved.route.encrypted } }
      : {}),
    ...(saved?.lastConnectedAt ? { last_connected: saved.lastConnectedAt } : {}),
    // Each route with what its last test found: reachable, refused or timeout, with the latency.
    ...(saved?.routes.length
      ? {
          routes: saved.routes.map((route) => ({
            kind: route.kind,
            address: route.address,
            encrypted: route.encrypted,
            probe: route.probe
              ? {
                  result: route.probe.outcome,
                  ...(route.probe.latencyMs === null ? {} : { latency_ms: route.probe.latencyMs }),
                  tested_at: route.probe.at,
                }
              : null,
          })),
        }
      : {}),
    running_threads: running,
    pending_requests: waiting,
    harnesses: await machineHarnesses(machine),
    projects: await machineProjectRows(machine, connected),
  };
}

/** How long `list_machines` with `probe` waits for the machines it woke to connect or say why not. */
const RECONNECT_WAIT_MS = 10_000;
const RECONNECT_POLL_MS = 500;

/**
 * Try every paired machine that is not connected again now, and wait (at most
 * `RECONNECT_WAIT_MS`) until each is connected or its attempt ended with a
 * reason. A probe whose route answered has already woken its machine in the
 * shell; this also wakes one whose routes all timed out, since a probe is only
 * a TCP connect.
 */
async function reconnectMachines(): Promise<void> {
  const woken = new Set(await retryMachines());
  // The ones the probe woke are trying already.
  for (const m of useMachines.getState().machines) {
    if (m.state === "connecting" && !connectionFor(m.id)) woken.add(m.id);
  }
  if (!woken.size) return;
  const tried = new Set<string>();
  const deadline = Date.now() + RECONNECT_WAIT_MS;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, RECONNECT_POLL_MS));
    await refreshMachines();
    const machines = useMachines.getState().machines;
    let pending = false;
    for (const id of woken) {
      const m = machines.find((entry) => entry.id === id);
      if (!m || connectionFor(id) || m.state === "needsPairing" || m.state === "stopped") continue;
      if (m.state === "connecting" || m.state === "connected") {
        tried.add(id);
        pending = true;
      } else if (!tried.has(id)) {
        // Still on the backoff it had before the wake reached it.
        pending = true;
      }
    }
    if (!pending) return;
  }
}

/**
 * A machine's projects for `list_machines`: path, name and the repository each
 * is a checkout of. A machine that is not connected says what was read last.
 */
async function machineProjectRows(machine: MachineRef, connected: boolean): Promise<JsonObject[]> {
  const local = machine.host ? [] : listProjects(useApp.getState()).projects;
  const projects = machine.host
    ? machineProjects(machine.host)
    : (Array.isArray(local) ? local : []).flatMap((row) => {
        const project = obj(row);
        return project ? [{ path: String(project.path ?? ""), name: String(project.name ?? "") }] : [];
      });
  const listed = projects.slice(0, MAX_PROJECTS);
  const remotes = await Promise.all(
    listed.map((project) =>
      connected
        ? repositoryOf(machine.host, project.path)
        : repositories.get(`${machine.host}\n${project.path}`),
    ),
  );
  return listed.map((project, index) => ({
    path: project.path,
    name: project.name,
    ...(remotes[index] === undefined ? {} : { remote: remotes[index] }),
  }));
}

/**
 * `list_machines`: this computer and each paired machine, one call. With `probe`
 * every route of every paired machine is tested first, so the answer carries
 * what each found now rather than the last test, and every machine that is not
 * connected is tried again at once; the answer then says whether it came up,
 * or why not.
 */
export async function listMachines(probe = false, callerHost: string = LOCAL_HOST): Promise<JsonObject> {
  const probed = probe ? await probeMachineRoutes() : undefined;
  if (probe) await reconnectMachines();
  const local = localMachine();
  const machines = [local, ...useMachines.getState().machines.map(machineRef)];
  return {
    current: callerHost || local.id,
    machines: await Promise.all(machines.map((entry) => machineEntry(entry, callerHost))),
    ...(probed === undefined ? {} : { probed }),
  };
}

/**
 * `run_machine_command`: run one shell command on any connected machine in the
 * fleet through its engine's `_rustcode/machine/exec` method. Respects the
 * caller's access level (Full runs without asking; Ask/Auto-edit prompts) and
 * the target machine's `.rustcode/command-policy.toml` command guard.
 */
export async function runMachineCommand(
  machine: MachineRef,
  caller: RuleThread,
  call: { command: string; cwd?: string; timeoutSecs?: number },
): Promise<JsonObject> {
  const callerHost = splitThreadKey(caller.id).host;
  const cwd = call.cwd ?? (machine.host === callerHost ? projectOf(caller) || undefined : undefined);
  const crossMachine = machine.host !== callerHost;
  const no = await permitted(caller, `exec@${machine.id}`, {
    kind: "exec",
    caller: caller.id,
    callerTitle: caller.title,
    target: machine.name,
    detail: cwd ? `in ${cwd}\n${call.command}` : call.command,
    ...(crossMachine ? { machine: machine.name } : {}),
  });
  if (no) return failureJson(no);
  const timeoutSecs = call.timeoutSecs ?? 60;
  try {
    const result = await requestOn<JsonObject>(
      machine.host,
      Method.machineExec,
      {
        command: call.command,
        ...(cwd ? { cwd } : {}),
        timeoutSecs,
      },
      (timeoutSecs + 15) * 1000,
    );
    return {
      ...machineFields(machine),
      ...(typeof result.cwd === "string" ? { cwd: result.cwd } : {}),
      success: result.success === true,
      output: typeof result.output === "string" ? result.output : "",
    };
  } catch (error) {
    return failureJson(
      failure("failed", `the command on ${machine.name} could not run: ${errorText(error)}`),
    );
  }
}

/**
 * The harnesses one machine can run, as `list_machines` lists them: this app's
 * own snapshot, or that machine's. Each says whether it can run a thread and
 * why not, its first models and its named logins; `list_harnesses` with
 * `machine` has the rest.
 */
async function machineHarnesses(machine: MachineRef): Promise<JsonObject[] | null> {
  const snapshot = machine.host
    ? await loadMachineHarnesses(machine.host)
    : harnessesOn(useApp.getState(), LOCAL_HOST);
  if (!snapshot) return null;
  return snapshot.harnesses.map((h): JsonObject => {
    const reason = unavailableReason(h);
    if (reason) return { id: h.id, name: h.name, available: false, reason };
    const models = h.models.slice(0, MACHINE_LISTED_MODELS).map((m) => m.id);
    return {
      id: h.id,
      name: h.name,
      available: true,
      ...(h.id === snapshot.default.harness ? { default: true } : {}),
      models,
      ...(models.length < h.models.length ? { omitted_models: h.models.length - models.length } : {}),
      ...(h.accounts?.length
        ? {
            accounts: h.accounts.map((a) => ({
              id: a.id,
              name: a.name,
              ...(accountSignedOut(h.id, a.id, useApp.getState().signedOutLogins)
                ? { signed_out: true }
                : {}),
            })),
          }
        : {}),
    };
  });
}

/** Another machine's harnesses, read over its own connection (kept per machine). */
async function remoteSnapshot(machine: MachineRef): Promise<HarnessSnapshot | Failure> {
  const snapshot = await loadMachineHarnesses(machine.host);
  if (snapshot) {
    // A harness that never ran there has learned no models: ask the machine, as its picker does.
    const empty = snapshot.harnesses.filter((h) => !unavailableReason(h) && !h.builtin && !h.models.length);
    if (!empty.length) return snapshot;
    await Promise.all(empty.map((h) => ensureHarnessModels(h.id, machine.host)));
    return useApp.getState().remoteHarnesses[machine.host] ?? snapshot;
  }
  return machineUnavailable(machine) ?? failure("failed", `could not read ${machine.name}'s harnesses`);
}

/** One harness of a named machine, by id else name, refused with what can run there. */
function harnessOn(
  snapshot: HarnessSnapshot,
  asked: string | undefined,
  machine: MachineRef,
): Harness | Failure {
  const wanted = asked?.trim().toLowerCase();
  const all = snapshot.harnesses;
  const usable = (h: Harness) => !unavailableReason(h);
  if (!wanted) {
    const fallback = snapshot.default.harness;
    const h =
      all.find((x) => x.id === fallback && usable(x)) ??
      all.find(usable) ??
      all.find((x) => x.id === fallback);
    if (!h) return failure("not_allowed", `${machine.name} has no harness that can run a thread`);
    const reason = unavailableReason(h);
    if (reason) {
      return failure("not_allowed", `${h.name} cannot run a thread on ${machine.name}: ${reason}`);
    }
    return h;
  }
  const byId = all.filter((h) => h.id.toLowerCase() === wanted);
  const named = byId.length ? byId : all.filter((h) => h.name.toLowerCase() === wanted);
  if (!named.length) {
    return failure(
      "invalid",
      `no harness “${asked}” on ${machine.name}; it has ${all.map((h) => h.id).join(", ")}`,
    );
  }
  if (named.length > 1) {
    return failure(
      "invalid",
      `“${asked}” names ${named.length} harnesses on ${machine.name}; pass the id: ${named.map((h) => h.id).join(", ")}`,
    );
  }
  const h = named[0]!;
  const reason = unavailableReason(h);
  if (reason) return failure("not_allowed", `${h.name} cannot run a thread on ${machine.name}: ${reason}`);
  return h;
}

/**
 * `list_projects` on a machine: the projects it has threads in. This
 * computer's answer also counts a project's unopened threads; another
 * machine's is what its listed threads say, which is what it knows here, plus
 * what `clone_project` cloned there, each with the repository it is a
 * checkout of (`remote`, `host/owner/repo`), which is how `start_session`
 * finds the caller's project there.
 */
export async function projectsOnMachine(machine: MachineRef): Promise<JsonObject> {
  if (!machine.host) return { ...machineFields(machine), ...listProjects(useApp.getState()) };
  const rows = new Map<
    string,
    { path: string; name: string; threads: number; working: boolean; updatedAt: number }
  >();
  for (const x of useApp.getState().sessions) {
    if ((x.host ?? LOCAL_HOST) !== machine.host) continue;
    // A thread with no folder of its own is not a project: its machine lists a stand-in, and a root
    // is never a project either (issue 3425). Neither is the app's own data folder (issue 3524).
    if (x.workspaceUnknown) continue;
    const path = plainPath(x.worktree?.repo || x.repo || x.cwd);
    if (!path || isRootFolder(path) || isDataFolder(path)) continue;
    const entry = rows.get(path) ?? {
      path,
      name: projectName(path),
      threads: 0,
      working: false,
      updatedAt: 0,
    };
    entry.threads += 1;
    entry.working ||= isRunning(useApp.getState().threads[x.id]) || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    rows.set(path, entry);
  }
  for (const path of clonedOn.get(machine.host) ?? []) {
    if (!rows.has(path))
      rows.set(path, { path, name: projectName(path), threads: 0, working: false, updatedAt: 0 });
  }
  const listed = [...rows.values()].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, MAX_PROJECTS);
  const remotes = await Promise.all(listed.map((p) => repositoryOf(machine.host, p.path)));
  const projects = listed.map((p, index) => ({
    path: p.path,
    name: p.name,
    // null: no `origin`; absent: the folder could not be read there.
    ...(remotes[index] === undefined ? {} : { remote: remotes[index] }),
    threads: p.threads,
    working: p.working,
  }));
  return { ...machineFields(machine), projects };
}

/**
 * `list_harnesses` on a machine: its snapshot, trimmed to what picks a thread
 * (id, name, availability, models and named accounts), never another
 * machine's plan numbers or this app's remembered choices.
 */
export async function listHarnessesOn(
  machine: MachineRef,
  caller: RuleThread,
  asked: { harness?: string; model?: string },
): Promise<JsonObject> {
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  let listed = snapshot.harnesses;
  if (asked.harness) {
    const harness = harnessOn(snapshot, asked.harness, machine);
    if (isFailure(harness)) return failureJson(harness);
    listed = [harness];
  }
  const filter = asked.model?.toLowerCase();
  return {
    ...machineFields(machine),
    current: { harness: caller.harness ?? "rustcode", ...ranWith(caller.id) },
    harnesses: listed.map((h): JsonObject => {
      const reason = unavailableReason(h);
      if (reason) return { id: h.id, name: h.name, available: false, reason };
      const matches = filter
        ? h.models.filter((m) => `${m.id} ${m.name}`.toLowerCase().includes(filter))
        : h.models;
      const models = matches
        .slice(0, filter ? FILTERED_MODELS : LISTED_MODELS)
        .map((m) => ({ id: m.id, name: m.name, ...(m.efforts.length ? { efforts: m.efforts } : {}) }));
      return {
        id: h.id,
        name: h.name,
        available: true,
        ...(h.accounts?.length
          ? {
              accounts: h.accounts.map((a) => ({
                id: a.id,
                name: a.name,
                auth: a.auth,
                ...(accountSignedOut(h.id, a.id, useApp.getState().signedOutLogins)
                  ? { signed_out: true }
                  : {}),
              })),
            }
          : {}),
        models,
        ...(models.length < matches.length ? { omitted_models: matches.length - models.length } : {}),
        ...(filter && !models.length ? { filter: MODEL_MISS_HINT } : {}),
      };
    }),
  };
}

/**
 * `list_sessions` on a machine: its threads, newest first, with the machine
 * each one is on. Another machine's threads are not narrowed to this
 * project: its folders are not this one's, and the user named the machine.
 */
export function sessionsOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  query: string,
  allProjects: boolean,
): JsonObject {
  const s = useApp.getState();
  const among = { ...s, sessions: s.sessions.filter((x) => (x.host ?? LOCAL_HOST) === machine.host) };
  const threads = among.sessions.flatMap((x) => ruleThread(among, x.id) ?? []);
  const listed = listSessions(threads, caller, query, true);
  const sessions = (Array.isArray(listed.sessions) ? listed.sessions : []).map((entry) => {
    const item = obj(entry) ?? {};
    return {
      ...item,
      ...machineFields(machine),
      ...(allProjects || machine.host ? { project: item.project ?? "" } : {}),
    };
  });
  return { ...machineFields(machine), sessions };
}

/**
 * The project a thread starts in on a machine: the agent's own path, else the
 * caller's path when that machine has it, else the project there that is a
 * checkout of the caller's repository (its `origin`, however each URL is
 * spelled), else, for a caller with no repository, a folder of the same name.
 * Nothing is guessed: a machine whose projects match none of those refuses
 * with each one's path and repository, and never starts in whichever it used
 * last (B6). The app's own data folder is never a project, however a
 * machine's thread list names it (issue 3524).
 */
export async function projectOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  asked: string | undefined,
): Promise<string | Failure> {
  // A path the agent names is its own choice, and the machine's engine refuses
  // one that is not a folder there: this app only knows what it has listed.
  if (asked?.trim()) return asked.trim().replace(/\/+$/, "");
  const projects = machineProjects(machine.host);
  const own = projectOf(caller);
  const there = projects.find((project) => sameProject(project.path, own));
  if (there) return there.path;
  const ours = own ? await repositoryOf(LOCAL_HOST, own) : null;
  const theirs = await Promise.all(projects.map((project) => repositoryOf(machine.host, project.path)));
  const found = projectByRepository(
    { name: own ? projectName(own) : "", repository: ours },
    projects.map((project, index) => ({ ...project, repository: theirs[index] })),
  );
  if (found) return found.path;
  const wanted = ours ?? (own ? projectName(own) : "this thread's project");
  if (!projects.length) {
    return failure(
      "invalid",
      `${machine.name} has no project listed yet, so none is ${wanted}; pass project with its absolute path there, or clone_project with machine`,
    );
  }
  const listed = projects
    .slice(0, MAX_PROJECTS)
    .map((project, index) => `${project.path}${theirs[index] ? ` (${theirs[index]})` : ""}`);
  return failure(
    "invalid",
    `${machine.name} has no project that is ${wanted}; pass project as one of: ${listed.join(", ")}, or clone_project with machine`,
  );
}

/** `session/new` on a machine, with the worktree fallback a folder outside git needs. */
async function threadOnMachine(
  machine: MachineRef,
  project: string,
  harness: string,
  options: {
    model?: string;
    effort?: string;
    account?: string | null;
    title: string;
    worktree?: boolean;
    parent?: string;
  },
): Promise<string> {
  const wantWorktree = (options.worktree ?? true) && !!useApp.getState().features.worktrees;
  const ask = (worktree: boolean) =>
    createSession({
      host: machine.host,
      cwd: project,
      harness,
      account: options.account ?? undefined,
      model: options.model,
      effort: options.effort,
      title: options.title,
      parent: options.parent,
      worktree: worktree ? { repo: project, title: options.title } : undefined,
    });
  try {
    return await ask(wantWorktree);
  } catch (error) {
    if (wantWorktree && options.worktree === undefined && /git|repositor/i.test(errorText(error))) {
      return ask(false);
    }
    throw error;
  }
}

/** `start_session` on a paired machine: its harness, its project, its thread, this thread's link. */
export async function startOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  call: StartCall,
): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), 1, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const project = await projectOnMachine(machine, caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  const harness = harnessOn(snapshot, call.harness, machine);
  if (isFailure(harness)) return failureJson(harness);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode(project) : childMode(caller, project), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    `start@${machine.id}${other ? `:${project}` : ""}`,
    {
      kind: "start",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: [choiceLabel(harness.name, call.account ?? undefined, call.model, call.effort)],
      access,
      worktree: call.worktree ?? true,
      machine: machine.name,
      standalone: standalone || undefined,
    },
    undefined,
    harness.accounts?.length ? [delegateAccountKey(machine.id, harness.id, call.account ?? null)] : [],
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), 1, delegateLimits());
    if (again) return failureJson(again);
  }
  let id: string;
  try {
    id = await threadOnMachine(machine, project, harness.id, {
      model: call.model,
      effort: call.effort,
      // The login is that machine's own: never resolve it against this app's.
      account: call.account ?? null,
      title,
      worktree: call.worktree,
      parent: standalone ? undefined : caller.id,
    });
    // Before its first turn, so nothing it does runs broader than the thread that started it.
    setPermissionMode(id, access);
  } catch (error) {
    return failureJson(
      failure("failed", `the thread could not start on ${machine.name}: ${errorText(error)}`),
    );
  }
  if (standalone) {
    deliverOwn(id, call.prompt);
    return {
      session_id: id,
      receipt: "started",
      title,
      harness: harness.id,
      access: accessOf(id),
      standalone: true,
      ...machineFields(machine),
    };
  }
  const link = addLink({
    orchestrator: caller.id,
    target: id,
    orchestratorTitle: caller.title,
    targetTitle: title,
    message: call.prompt,
    state: "working",
    started: true,
    harness: harness.id,
    harnessName: harness.name,
    model: call.model,
    account: call.account,
    machine: machine.name,
  });
  deliver(id, delegated(call.prompt, caller.title), link, caller.id);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    access: accessOf(id),
    ...machineFields(machine),
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
}

/** `fan_out` on a paired machine: the same task on its harnesses, one link each. */
export async function fanOutOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  call: FanOutCall,
): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), call.harnesses.length, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  const project = await projectOnMachine(machine, caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const chosen: { harness: Harness; model?: string; effort?: string; account?: string | null }[] = [];
  const targets = new Set<string>();
  for (const entry of call.harnesses) {
    const h = harnessOn(snapshot, entry.harness, machine);
    if (isFailure(h)) return failureJson(h);
    const key = [h.id, entry.model ?? "", entry.effort ?? "", entry.account ?? ""].join("\u0000");
    if (targets.has(key)) {
      return failureJson(
        failure(
          "invalid",
          `harnesses resolves to the same target twice on ${machine.name}: ${choiceLabel(h.name, entry.account, entry.model, entry.effort)}; give each entry a different harness, model, effort or account`,
        ),
      );
    }
    targets.add(key);
    chosen.push({ harness: h, model: entry.model, effort: entry.effort, account: entry.account ?? null });
  }
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode(project) : childMode(caller, project), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    `start@${machine.id}${other ? `:${project}` : ""}`,
    {
      kind: "fan_out",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: chosen.map((h) => choiceLabel(h.harness.name, h.account ?? undefined, h.model, h.effort)),
      access,
      worktree: true,
      machine: machine.name,
      standalone: standalone || undefined,
    },
    undefined,
    chosen
      .filter((h) => h.harness.accounts?.length)
      .map((h) => delegateAccountKey(machine.id, h.harness.id, h.account ?? null)),
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), chosen.length, delegateLimits());
    if (again) return failureJson(again);
  }
  const fanId = localId("fan");
  const out: JsonObject[] = [];
  const linkIds: string[] = [];
  for (const h of chosen) {
    const threadTitle = [title, h.harness.name, h.account].filter(Boolean).join(" · ");
    try {
      const id = await threadOnMachine(machine, project, h.harness.id, {
        model: h.model,
        effort: h.effort,
        account: h.account,
        title: threadTitle,
        parent: standalone ? undefined : caller.id,
      });
      setPermissionMode(id, access);
      if (standalone) {
        deliverOwn(id, call.prompt);
        out.push({
          session_id: id,
          harness: h.harness.id,
          access: accessOf(id),
          ...machineFields(machine),
        });
        continue;
      }
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.harness.id,
        harnessName: h.harness.name,
        model: h.model,
        account: h.account ?? undefined,
        fanOut: fanId,
        machine: machine.name,
      });
      linkIds.push(link);
      out.push({
        session_id: id,
        link_id: link,
        harness: h.harness.id,
        access: accessOf(id),
        ...machineFields(machine),
      });
      deliver(id, delegated(call.prompt, caller.title), link, caller.id);
    } catch (error) {
      out.push({ harness: h.harness.id, ...failureJson(failure("failed", errorText(error))) });
    }
  }
  if (linkIds.length) {
    const group: FanOut = {
      id: fanId,
      orchestrator: caller.id,
      title,
      prompt: call.prompt,
      links: linkIds,
      createdAt: Date.now(),
    };
    setBook((b) => ({ ...b, fanOuts: [...b.fanOuts, group] }));
  }
  if (standalone) return { ...machineFields(machine), links: out, standalone: true };
  return {
    ...machineFields(machine),
    links: out,
    fan_out_id: fanId,
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
}
