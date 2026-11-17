// Which machine a Settings page edits, and one machine's server settings over
// its own connection (docs/desktop.md › Settings on one machine or all).
//
// The choice is one value for the whole panel and persists, as T3 Code's does:
// **This machine** (the default), one paired machine, or **All machines**. A
// server setting reads from each connected machine and writes to the chosen
// one(s); an offline machine is listed but never changed, so it keeps its
// value. The local machine's values are kept in `settings.json` on its server
// and applied to the app's own state, so a value set from another machine is
// there when this one starts.
//
// `default-harness` is read through `_rustcode/harnesses` because the engine
// already keeps it in `config.toml`; the other keys use `_rustcode/settings`.

import { useEffect } from "react";
import { create } from "zustand";
import { arr, type Json, type JsonObject, obj } from "../acp/jsonrpc";
import type { Harness, HarnessSnapshot } from "../acp/types";
import { Method, parseHarnessSnapshot } from "../acp/types";
import { readStorage, writeStorage } from "../lib/storage";
import { useMachines } from "./machines";
import { readProjectBook, updateProjectEntry } from "./projectBook";
import { matchProject, type Project, projectsOn } from "./projects";
import { settingsReadCache } from "./settingsReadCache";
import {
  failureSummary,
  missingSettingValues,
  type ProjectSettingKey,
  projectOverridable,
  projectReadOnlyReason,
  projectTargets,
  resolveSetting,
  runOnMachines,
  type ScopeBreakdownEntry,
  type ScopeChoice,
  type ScopedMachine,
  type ScopeValueEntry,
  SERVER_SETTING_KEYS,
  type ServerSettingKey,
  type SettingSource,
  SOURCE_LABEL,
  scopeBreakdown,
  scopeValuesMixed,
  serverSetting,
} from "./settingsRegistry";
import { connectionFor, errorText, type NewThreadChoice, toast, useApp } from "./store";
import { LOCAL_HOST } from "./threadKeys";

// ------------------------------------------------------------ the choice

export type SettingsScope = ScopeChoice;

const SCOPE_STORAGE = "rustcode.settingsScope";

function readScope(): SettingsScope {
  try {
    const raw = JSON.parse(readStorage(SCOPE_STORAGE) ?? "null") as unknown;
    const root = obj(raw as Json);
    const mode = root?.mode;
    if (mode === "all") return { mode: "all" };
    if (mode === "machine" && typeof root?.host === "string") return { mode: "machine", host: root.host };
  } catch {
    // A stored value from an older build reads as the default.
  }
  return { mode: "this" };
}

interface ScopeStore {
  scope: SettingsScope;
  setScope: (scope: SettingsScope) => void;
}

export const useSettingsScope = create<ScopeStore>((set) => ({
  scope: readScope(),
  setScope: (scope) => {
    // A full or disabled store only loses the choice for the next open.
    writeStorage(SCOPE_STORAGE, JSON.stringify(scope));
    set({ scope });
  },
}));

// ------------------------------------------------------------ the project choice

const PROJECT_STORAGE = "rustcode.settingsProject";

function readProjectChoice(): Project | null {
  try {
    const root = obj(JSON.parse(readStorage(PROJECT_STORAGE) ?? "null") as Json);
    if (typeof root?.path === "string" && root.path)
      return {
        path: root.path,
        name: typeof root.name === "string" ? root.name : root.path,
        ...(typeof root.remote === "string" ? { remote: root.remote } : {}),
      };
  } catch {
    // An unreadable choice reads as All projects.
  }
  return null;
}

/** **All projects** (null, the default) or one project, as this machine names it. */
export const useProjectScope = create<{
  project: Project | null;
  setProject: (project: Project | null) => void;
}>((set) => ({
  project: readProjectChoice(),
  setProject: (project) => {
    writeStorage(PROJECT_STORAGE, JSON.stringify(project));
    set({ project });
  },
}));

/** The projects the bar offers: this machine's, most recently used first. */
export function useScopeProjects(): Project[] {
  const sessions = useApp((state) => state.sessions);
  return projectsOn(sessions, LOCAL_HOST);
}

/** The chosen project's folder on one machine: matched as Run on matches it, or nothing. */
export function projectPathOn(project: Project, host: string): string | undefined {
  if (host === LOCAL_HOST) return project.path;
  return matchProject(project, projectsOn(useApp.getState().sessions, host))?.path;
}

/** A machine the panel can show, local or paired. */
export type ScopeMachine = ScopedMachine;

/** This machine first, then each paired machine in the order Machines lists them. */
export function scopeMachines(machines: { id: string; name: string; state: string }[]): ScopeMachine[] {
  return [
    { host: LOCAL_HOST, name: "This machine", online: true, local: true },
    ...machines.map((machine) => ({
      host: machine.id,
      name: machine.name,
      online: machine.state === "connected",
      local: false,
    })),
  ];
}

/** Every machine the current choice shows. */
export function scopeTargets(scope: SettingsScope, machines: ScopeMachine[]): ScopeMachine[] {
  if (scope.mode === "all") return machines;
  if (scope.mode === "machine") return machines.filter((machine) => machine.host === scope.host);
  return machines.filter((machine) => machine.local);
}

/** The machines a write reaches: All machines skips the offline ones. */
export function scopeWriteTargets(scope: SettingsScope, machines: ScopeMachine[]): ScopeMachine[] {
  return scopeTargets(scope, machines).filter((machine) => machine.online);
}

export function scopeLabel(scope: SettingsScope, machines: ScopeMachine[]): string {
  if (scope.mode === "all") return "All machines";
  if (scope.mode === "this") return "This machine";
  return machines.find((machine) => machine.host === scope.host)?.name ?? "That machine";
}

// ------------------------------------------------------------ connections

/** Read the machines from the store; local first. */
export function useScopeMachines(): ScopeMachine[] {
  const machines = useMachines((state) => state.machines);
  return scopeMachines(machines);
}

export function useScope(): {
  scope: SettingsScope;
  setScope: (scope: SettingsScope) => void;
  machines: ScopeMachine[];
  targets: ScopeMachine[];
  writes: ScopeMachine[];
} {
  const scope = useSettingsScope((state) => state.scope);
  const setScope = useSettingsScope((state) => state.setScope);
  const machines = useScopeMachines();
  return {
    scope,
    setScope,
    machines,
    targets: scopeTargets(scope, machines),
    writes: scopeWriteTargets(scope, machines),
  };
}

/** One request on a machine's own connection; a missing machine says so. */
export async function machineRequest<T = Json>(
  host: string,
  method: string,
  params: JsonObject = {},
  timeoutMs = 15_000,
): Promise<T> {
  const connection = connectionFor(host);
  if (!connection) throw new Error("That machine is not connected.");
  return connection.request<T>(method, params, timeoutMs);
}

// ------------------------------------------------------------ setting values

const LOCAL_SEEDED = "rustcode.serverSettingsSeeded";

interface ValuesStore {
  values: Record<string, Json>;
  loading: Record<string, boolean>;
}

/** Keyed by `host\u0000setting`, so one store holds every machine's values. */
export const useSettingValues = create<ValuesStore>(() => ({ values: {}, loading: {} }));

const valueKey = (host: string, key: string) => `${host}\u0000${key}`;

/** The app's current values, for a server that has none yet or does not answer. */
function localValues(): Record<ServerSettingKey, Json> {
  const state = useApp.getState();
  return {
    "default-permission-mode": state.defaultPermissionMode,
    "resume-after-restart": state.resumeAfterRestart,
    "continue-after-limit": state.continueAfterLimit,
    "auto-compact-mode": state.autoCompact.mode,
    "auto-compact-threshold": state.autoCompact.threshold,
    orchestration: state.orchestration,
    "delegates-total": state.totalLimit,
    "delegates-per-thread": state.threadLimit,
    "trust-delegate-accounts": state.trustDelegateAccounts,
    "read-across-projects": state.readAcrossProjects,
    // Only ever kept on the server: there is no app-side copy to migrate.
    "delegate-presets": [],
    "limit-fallback": state.limitFallback,
    "limit-fallback-map": state.limitFallbackMap as unknown as Json,
    "wake-batch-seconds": state.wakeBatchSeconds,
    // Only ever kept on the server, like the presets: nothing app-side to migrate.
    "workflow-concurrency": serverSetting("workflow-concurrency").default,
    "workflow-call-budget": serverSetting("workflow-call-budget").default,
    "workflow-timeout-minutes": serverSetting("workflow-timeout-minutes").default,
  };
}

/**
 * Put the app's local values on its server once. A key the server already
 * holds is left alone: another machine may have set it before this app first
 * started, and this seed is a migration, not an overwrite.
 */
async function seedLocalSettings(host: string): Promise<void> {
  if (host !== LOCAL_HOST || readStorage(LOCAL_SEEDED) === "1") return;
  const answer = await machineRequest(host, "_rustcode/settings/get", {}, 10_000);
  const storedKeys = arr(obj(answer)?.stored).flatMap((key) => (typeof key === "string" ? [key] : []));
  const missing = missingSettingValues(storedKeys, localValues());
  if (Object.keys(missing).length)
    await machineRequest(host, "_rustcode/settings/set", { values: missing }, 10_000);
  writeStorage(LOCAL_SEEDED, "1");
}

/** Copy the server's values for one host into the shared cache. */
function remember(host: string, values: JsonObject): void {
  useSettingValues.setState((state) => {
    const next = { ...state.values };
    for (const key of SERVER_SETTING_KEYS) {
      if (key in values) next[valueKey(host, key)] = values[key]!;
    }
    return { values: next };
  });
}

/** The cached values of every machine, keyed `host\u0000key`; a machine page compares with this. */
export function useServerValues(): Record<string, Json> {
  return useSettingValues((state) => state.values);
}

/** This app's own values, as a server that has not answered would hold them. */
export function localServerSettingValues(): Record<ServerSettingKey, Json> {
  return localValues();
}

/** Whether a machine's cached server value differs from this app's own. */
export function serverValueDiffers(
  values: Record<string, Json>,
  host: string,
  key: ServerSettingKey,
  local: Record<ServerSettingKey, Json>,
): boolean {
  const remote = values[valueKey(host, key)];
  return remote !== undefined && JSON.stringify(remote) !== JSON.stringify(local[key]);
}

/** Read every server setting from one machine, and cache what it answers. */
const settingsReads = settingsReadCache();
const settingsRevision = new Map<string, number>();

export function loadServerSettings(host: string, force = false): Promise<void> {
  return settingsReads.read(host, () => readServerSettings(host), force);
}

async function readServerSettings(host: string): Promise<void> {
  if ([...writesInFlight.keys()].some((key) => key.startsWith(`${host}\u0000`))) return;
  const revision = settingsRevision.get(host);
  const key = valueKey(host, "*");

  useSettingValues.setState((state) => ({ loading: { ...state.loading, [key]: true } }));
  try {
    if (host === LOCAL_HOST) {
      try {
        await seedLocalSettings(host);
        const answer = await machineRequest(host, "_rustcode/settings/get", {}, 10_000);
        const values = obj(obj(answer)?.values);
        if (values && revision === settingsRevision.get(host)) remember(host, values);
        else if (revision === settingsRevision.get(host)) remember(host, localValues());
      } catch {
        // An older server has no `_rustcode/settings`: the app keeps them.
        if (revision === settingsRevision.get(host)) remember(host, localValues());
      }
    } else {
      const answer = await machineRequest(host, "_rustcode/settings/get", {}, 10_000);
      const values = obj(obj(answer)?.values);
      if (values && revision === settingsRevision.get(host)) remember(host, values);
    }
  } finally {
    useSettingValues.setState((state) => {
      const loading = { ...state.loading };
      delete loading[key];
      return { loading };
    });
  }
}

/** Load the settings of every machine the scope shows; one failing read does not stop the rest. */
export async function loadScopeSettings(machines: ScopeMachine[]): Promise<void> {
  await Promise.allSettled(
    machines.filter((machine) => machine.online).map((machine) => loadServerSettings(machine.host)),
  );
}

/** Apply a server's values to the local app, so its behaviour follows them. */
function applyLocalSettings(values: Record<string, Json | undefined>): void {
  void Promise.all([import("./store"), import("./autoCompactRun")]).then(([store, autoCompact]) => {
    const text = (key: ServerSettingKey) => {
      const value = values[key];
      return typeof value === "string" ? value : undefined;
    };
    const bool = (key: ServerSettingKey) => {
      const value = values[key];
      return typeof value === "boolean" ? value : undefined;
    };
    const number = (key: ServerSettingKey) => {
      const value = values[key];
      return typeof value === "number" ? value : undefined;
    };
    const mode = text("default-permission-mode");
    if (mode === "ask" || mode === "edits" || mode === "full") store.setDefaultPermissionMode(mode);
    const resume = bool("resume-after-restart");
    if (resume !== undefined) store.setResumeAfterRestart(resume);
    const limit = bool("continue-after-limit");
    if (limit !== undefined) store.setContinueAfterLimit(limit);
    const compact = text("auto-compact-mode");
    if (compact === "off" || compact === "size" || compact === "cache")
      autoCompact.setAutoCompactMode(compact);
    const threshold = number("auto-compact-threshold");
    if (threshold !== undefined) autoCompact.setAutoCompactThreshold(threshold);
    const orchestration = bool("orchestration");
    if (orchestration !== undefined) store.setOrchestration(orchestration);
    const total = number("delegates-total");
    if (total !== undefined) store.setTotalLimit(total);
    const perThread = number("delegates-per-thread");
    if (perThread !== undefined) store.setThreadLimit(perThread);
    const trust = bool("trust-delegate-accounts");
    if (trust !== undefined) store.setTrustDelegateAccounts(trust);
    const readAcross = bool("read-across-projects");
    if (readAcross !== undefined) store.setReadAcrossProjects(readAcross);
    const fallback = bool("limit-fallback");
    if (fallback !== undefined) store.setLimitFallback(fallback);
    if (Array.isArray(values["limit-fallback-map"])) store.setLimitFallbackMap(values["limit-fallback-map"]);
    const batch = number("wake-batch-seconds");
    if (batch !== undefined) store.setWakeBatchSeconds(batch);
  });
}

/**
 * `_rustcode/settings/changed`: a machine's `settings.json` changed, written by
 * the CLI, an agent's `manage_settings`, another window or a hand edit. The
 * cache follows it, and on this machine the app applies it at once. A write
 * this window still has in flight wins: its own answer settles the value.
 */
export function onServerSettingsChanged(host: string, params: JsonObject): void {
  const values = obj(params.values);
  if (!values) return;
  const pending = new Set(
    [...writesInFlight.keys()]
      .filter((key) => key.startsWith(`${host}\u0000`))
      .map((key) => key.slice(host.length + 1)),
  );
  const settled: JsonObject = {};
  for (const [key, value] of Object.entries(values)) {
    if (!pending.has(key) && value !== undefined) settled[key] = value;
  }
  settingsRevision.set(host, (settingsRevision.get(host) ?? 0) + 1);
  remember(host, settled);
  if (host === LOCAL_HOST) applyLocalSettings(settled);
}

let hydration: Promise<void> | null = null;

/**
 * Once at start: put this machine's stored server settings into the app, and
 * migrate the app's own values into its server the first time. A server
 * without `_rustcode/settings` leaves the app's values in charge.
 */
export function hydrateLocalServerSettings(): Promise<void> {
  hydration ??= (async () => {
    const connection = connectionFor(LOCAL_HOST);
    if (!connection) return;
    try {
      await seedLocalSettings(LOCAL_HOST);
      const answer = await machineRequest(LOCAL_HOST, "_rustcode/settings/get", {}, 10_000);
      const values = obj(obj(answer)?.values);
      if (!values) return;
      remember(LOCAL_HOST, values);
      applyLocalSettings(values);
      await seedLocalProjectOverrides(useApp.getState().projectPermissionModes).catch(() => undefined);
    } catch {
      // An older server: nothing to apply.
    }
  })();
  return hydration;
}

/** Write one setting to one machine; the local app follows it at once. */
export const useSettingsSave = create<{ pending: number; error: string | null; saved: boolean }>(() => ({
  pending: 0,
  error: null,
  saved: false,
}));
const writesInFlight = new Map<string, Promise<void>>();
const confirmedValues = new Map<string, Json | undefined>();
const writeVersions = new Map<string, number>();

export async function writeServerSetting(host: string, key: ServerSettingKey, value: Json): Promise<void> {
  const cacheKey = valueKey(host, key);
  if (!writesInFlight.has(cacheKey))
    confirmedValues.set(cacheKey, useSettingValues.getState().values[cacheKey]);
  const version = (writeVersions.get(cacheKey) ?? 0) + 1;
  writeVersions.set(cacheKey, version);
  settingsRevision.set(host, (settingsRevision.get(host) ?? 0) + 1);
  remember(host, { [key]: value });
  useSettingsSave.setState((s) => ({ pending: s.pending + 1, error: null, saved: false }));
  const prior = writesInFlight.get(cacheKey);
  const task = (async () => {
    await prior?.catch(() => undefined);
    try {
      await machineRequest(host, "_rustcode/settings/set", { values: { [key]: value } }, 10_000);
      // A full response can contain old values for other writes; commit only this key.
      confirmedValues.set(cacheKey, value);
      if (host === LOCAL_HOST) applyLocalSettings({ [key]: value });
      useSettingsSave.setState({ saved: true });
    } catch (error) {
      if (writeVersions.get(cacheKey) === version) {
        const previous = confirmedValues.get(cacheKey);
        useSettingValues.setState((s) => {
          const values = { ...s.values };
          if (previous === undefined) delete values[cacheKey];
          else values[cacheKey] = previous;
          return { values };
        });
      }
      useSettingsSave.setState({ error: errorText(error), saved: false });
      throw error;
    } finally {
      settingsReads.invalidate(host);
      useSettingsSave.setState((s) => ({ pending: s.pending - 1 }));
    }
  })();
  writesInFlight.set(cacheKey, task);
  try {
    await task;
  } finally {
    if (writesInFlight.get(cacheKey) === task) writesInFlight.delete(cacheKey);
  }
}

// ------------------------------------------------------------ project overrides

/** One machine's answer for one project: its own overrides, keyed `host\u0000path`. */
interface ProjectAnswer {
  overrides: Record<string, Json>;
}

const useProjectValues = create<{ answers: Record<string, ProjectAnswer> }>(() => ({ answers: {} }));

const projectKeyOf = (host: string, path: string) => `${host}\u0000${path}`;

function rememberProject(host: string, path: string, answer: Json): void {
  // An older server answers without `project`: it has no overrides, and the app's own stay.
  const project = obj(obj(answer)?.project);
  if (!project) return;
  const overrides = obj(project.overrides) ?? {};
  useProjectValues.setState((state) => ({
    answers: { ...state.answers, [projectKeyOf(host, path)]: { overrides: { ...overrides } } },
  }));
  if (host === LOCAL_HOST) mirrorLocalProject(path, overrides);
}

/**
 * This machine's project overrides drive the app's own per-project defaults, which a new thread
 * and the approval answers read: Settings › Permissions › Projects and the project's harness.
 */
function mirrorLocalProject(path: string, overrides: Record<string, Json>): void {
  void import("./store").then((store) => {
    const mode = overrides["default-permission-mode"];
    const current = store.useApp.getState().projectPermissionModes[path];
    const next = mode === "ask" || mode === "edits" || mode === "full" ? mode : null;
    if ((current ?? null) !== next) store.setProjectPermissionMode(path, next, { fromServer: true });
  });
  const harness = obj(overrides["default-harness"]);
  if (harness && typeof harness.harness === "string")
    updateProjectEntry(path, {
      harness: harness.harness,
      ...(typeof harness.model === "string" ? { model: harness.model } : {}),
    });
}

/** Read one project's overrides from one machine. */
export async function loadProjectSettings(host: string, path: string): Promise<void> {
  try {
    rememberProject(
      host,
      path,
      await machineRequest(host, "_rustcode/settings/get", { project: path }, 10_000),
    );
  } catch {
    // An older server has no project overrides: the row shows the machine's value.
  }
}

/** Write (a value) or remove (`null`) one project override on one machine. */
export async function writeProjectSetting(
  host: string,
  path: string,
  key: ProjectSettingKey,
  value: Json | null,
): Promise<void> {
  const params: JsonObject =
    value === null ? { project: path, clear: [key] } : { project: path, values: { [key]: value } };
  rememberProject(host, path, await machineRequest(host, "_rustcode/settings/set", params, 10_000));
}

const PROJECT_SEEDED = "rustcode.projectOverridesSeeded";

/**
 * Once: the per-project access and harness this app kept on its own move into this machine's
 * server as project overrides, so every reader resolves them the same way. A project the server
 * already has an override for keeps it.
 */
export async function seedLocalProjectOverrides(modes: Record<string, string>): Promise<void> {
  if (readStorage(PROJECT_SEEDED) === "1") return;
  const book = readProjectBook();
  const paths = new Set([...Object.keys(modes), ...Object.keys(book)]);
  for (const path of paths) {
    const values: JsonObject = {};
    const mode = modes[path];
    if (mode) values["default-permission-mode"] = mode;
    const entry = book[path];
    if (entry?.harness)
      values["default-harness"] = { harness: entry.harness, ...(entry.model ? { model: entry.model } : {}) };
    if (!Object.keys(values).length) continue;
    const answer = await machineRequest(LOCAL_HOST, "_rustcode/settings/get", { project: path }, 10_000);
    const existing = obj(obj(obj(answer)?.project)?.overrides) ?? {};
    const missing = missingSettingValues(Object.keys(existing), values);
    if (Object.keys(missing).length)
      await machineRequest(LOCAL_HOST, "_rustcode/settings/set", { project: path, values: missing }, 10_000);
  }
  writeStorage(PROJECT_SEEDED, "1");
}

/** How a row reads with one project chosen. */
export interface ProjectRowState {
  /** The project's name, for the hover card. */
  name: string;
  /** Why the row cannot change per project; null when it can. */
  readOnly: string | null;
  source: SettingSource;
  /** Each machine's layer, for the hover card. */
  breakdown: ScopeBreakdownEntry[];
  /** Machines the chosen project is not on: never written. */
  skipped: ScopeMachine[];
  /** "Use the machine's value": remove the override everywhere the row reaches. */
  clear?: () => Promise<void>;
}

/** Load the chosen project's overrides on every machine it reaches. */
function useLoadProjectSettings(project: Project | null, targets: ScopeMachine[]): void {
  const sessions = useApp((state) => state.sessions.length);
  const key = project
    ? `${project.path}|${targets.map((machine) => machine.host).join("|")}|${sessions}`
    : "";
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` names the project and its machines
  useEffect(() => {
    if (!project) return;
    for (const machine of targets) {
      const path = machine.online ? projectPathOn(project, machine.host) : undefined;
      if (path) void loadProjectSettings(machine.host, path);
    }
  }, [key]);
}

/** A project row's layers on each machine, and the value shown. */
export function projectLayers(
  key: ProjectSettingKey,
  project: Project,
  targets: ScopeMachine[],
  machineValue: (host: string) => Json | undefined,
  answers: Record<string, ProjectAnswer>,
  fallback: Json,
  format: (value: Json | undefined) => string,
): {
  value: Json | undefined;
  source: SettingSource;
  mixed: boolean;
  breakdown: ScopeBreakdownEntry[];
  reached: { machine: ScopeMachine; path: string }[];
  skipped: ScopeMachine[];
} {
  const { reached, skipped } = projectTargets(targets, (machine) => projectPathOn(project, machine.host));
  const resolved = reached.map((target) => {
    const own = answers[projectKeyOf(target.machine.host, target.path!)]?.overrides[key];
    return {
      target,
      ...resolveSetting({ default: fallback, machine: machineValue(target.machine.host), project: own }),
    };
  });
  const entries: ScopeValueEntry[] = resolved.map(({ target, value }) => ({ ...target.machine, value }));
  const shown = resolved.find((entry) => entry.target.machine.online) ?? resolved[0];
  return {
    value: shown?.value,
    source: shown?.source ?? "machine",
    mixed: scopeValuesMixed(entries),
    breakdown: [
      ...resolved.map(({ target, value, source }) => ({
        ...target.machine,
        value,
        label: `${format(value)} · ${SOURCE_LABEL[source]}${target.machine.online ? "" : " — offline"}`,
      })),
      ...skipped.map((machine) => ({ ...machine, label: "Skipped: the project is not on it" })),
    ],
    reached: reached.map((target) => ({ machine: target.machine, path: target.path! })),
    skipped,
  };
}

// ------------------------------------------------------------ the row hook

export interface ScopedSetting {
  value: Json | undefined;
  mixed: boolean;
  breakdown: ScopeBreakdownEntry[];
  targets: ScopeMachine[];
  writes: ScopeMachine[];
  offline: ScopeMachine[];
  loading: boolean;
  set: (value: Json) => Promise<void>;
  /** With one project chosen: where the value comes from, or why it cannot change here. */
  project: ProjectRowState | null;
}

/**
 * One server setting as the scope shows it: the displayed value, whether the
 * connected machines disagree, each machine's value for the hover card, and a
 * write that reaches every connected machine under **All machines**.
 */
export function useScopedSetting(
  key: ServerSettingKey,
  format: (value: Json | undefined) => string,
): ScopedSetting {
  const { scope, machines, writes } = useScope();
  const loadingByHost = useSettingValues((state) => state.loading);
  const stored = useSettingValues((state) => state.values);
  const targets = scopeTargets(scope, machines);
  const loading = targets.some((machine) => loadingByHost[valueKey(machine.host, "*")] === true);
  const entries: ScopeValueEntry[] = targets.map((machine) => ({
    host: machine.host,
    name: machine.name,
    online: machine.online,
    value: stored[valueKey(machine.host, key)],
  }));
  const shown = entries.find((entry) => entry.online && entry.value !== undefined);
  const mixed = scopeValuesMixed(entries);
  const offline = machines.filter((machine) => !machine.online);
  const chosen = useProjectScope((state) => state.project);
  const answers = useProjectValues((state) => state.answers);
  useLoadProjectSettings(chosen, targets);
  if (chosen) {
    const reason = projectReadOnlyReason("server", key);
    if (reason || !projectOverridable(key)) {
      return {
        value: shown?.value,
        mixed,
        breakdown: scopeBreakdown(entries, (value) => format(value)),
        targets,
        writes: [],
        offline,
        loading,
        set: async () => {
          toast(reason ?? "", "info");
        },
        project: { name: chosen.name, readOnly: reason, source: "machine", breakdown: [], skipped: [] },
      };
    }
    const layers = projectLayers(
      key,
      chosen,
      targets,
      (host) => stored[valueKey(host, key)],
      answers,
      serverSetting(key).default,
      format,
    );
    const writable = layers.reached.filter(
      (target) => target.machine.online && writes.some((w) => w.host === target.machine.host),
    );
    const write = async (value: Json | null) => {
      const result = await runOnMachines(
        writable.map((target) => target.machine),
        (machine) =>
          writeProjectSetting(
            machine.host,
            writable.find((t) => t.machine.host === machine.host)!.path,
            key,
            value,
          ),
      );
      if (result.failures.length) toast(failureSummary(result), "error");
    };
    return {
      value: layers.value,
      mixed: layers.mixed,
      breakdown: layers.breakdown,
      targets,
      writes: writable.map((target) => target.machine),
      offline,
      loading,
      set: (value) => write(value),
      project: {
        name: chosen.name,
        readOnly: null,
        source: layers.source,
        breakdown: layers.breakdown,
        skipped: layers.skipped,
        clear: layers.source === "project" || layers.mixed ? () => write(null) : undefined,
      },
    };
  }
  // Optimistic: the row shows the new value and this app follows it at once, then each machine
  // confirms; one that refuses goes back to what it had, with the reason in a toast.
  const set = async (value: Json) => {
    const before = useSettingValues.getState().values;
    for (const machine of writes) remember(machine.host, { [key]: value });
    if (writes.some((machine) => machine.host === LOCAL_HOST)) applyLocalSettings({ [key]: value });
    const result = await runOnMachines(writes, (machine) => writeServerSetting(machine.host, key, value));
    if (!result.failures.length) return;
    for (const failed of result.failures) {
      const previous = before[valueKey(failed.host, key)];
      // A newer pick since this one was sent is the one the row keeps.
      const now = useSettingValues.getState().values[valueKey(failed.host, key)];
      if (previous === undefined || JSON.stringify(now) !== JSON.stringify(value)) continue;
      remember(failed.host, { [key]: previous });
      if (failed.host === LOCAL_HOST) applyLocalSettings({ [key]: previous });
    }
    toast(failureSummary(result), "error");
  };
  return {
    value: shown?.value,
    mixed,
    breakdown: scopeBreakdown(entries, (value) => format(value)),
    targets,
    writes,
    offline,
    loading,
    set,
    project: null,
  };
}

/** Load the values the current scope shows; a page calls this on open and on a scope change. */
export function useLoadScopeSettings(): void {
  const { targets } = useScope();
  const key = `${targets.map((machine) => `${machine.host}:${machine.online ? 1 : 0}`).join("|")}`;
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` names the target set
  useEffect(() => {
    void loadScopeSettings(targets);
  }, [key]);
}

// ------------------------------------------------------------ default harness

export interface MachineHarness {
  snapshot: HarnessSnapshot | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

interface HarnessStore {
  byHost: Record<string, HarnessSnapshot>;
  loading: Record<string, boolean>;
  errors: Record<string, string | null>;
}

const useMachineHarnesses = create<HarnessStore>(() => ({
  byHost: {},
  loading: {},
  errors: {},
}));

/** One machine's harnesses: the local snapshot from the store, a remote one over its connection. */
export function useMachineHarness(host: string): MachineHarness {
  const local = useApp((state) => state.harnesses);
  const remote = useMachineHarnesses((state) => state.byHost[host]);
  const loading = useMachineHarnesses((state) => state.loading[host] === true);
  const error = useMachineHarnesses((state) => state.errors[host] ?? null);
  const reload = () => void loadMachineHarness(host);
  // A failed read is not asked again here: it would end at once for a machine
  // that is away and start the next one, over and over. Retry is `reload`.
  useEffect(() => {
    if (shouldAutoLoadHarness(host, { remote, loading, error })) void loadMachineHarness(host);
  }, [host, remote, loading, error]);
  if (host === LOCAL_HOST) return { snapshot: local, error: null, loading: false, reload };
  return { snapshot: remote ?? null, error, loading, reload };
}

/** Whether a page that shows a machine's harnesses should read them now: once, never again after a failure. */
export function shouldAutoLoadHarness(
  host: string,
  state: { remote: HarnessSnapshot | undefined | null; loading: boolean; error: string | null },
): boolean {
  return host !== LOCAL_HOST && !state.remote && !state.loading && !state.error;
}

/** Short bound for the read of a machine's harness list; an away machine must not hold a page. */
export const HARNESS_READ_MS = 10_000;

export async function loadMachineHarness(host: string): Promise<void> {
  if (host === LOCAL_HOST) return;
  useMachineHarnesses.setState((state) => ({ loading: { ...state.loading, [host]: true } }));
  try {
    const snapshot = parseHarnessSnapshot(await machineRequest(host, Method.harnesses, {}, HARNESS_READ_MS));
    useMachineHarnesses.setState((state) => ({
      byHost: { ...state.byHost, [host]: snapshot },
      errors: { ...state.errors, [host]: null },
    }));
  } catch (error) {
    useMachineHarnesses.setState((state) => ({
      errors: { ...state.errors, [host]: errorText(error) },
    }));
  } finally {
    useMachineHarnesses.setState((state) => {
      const loading = { ...state.loading };
      delete loading[host];
      return { loading };
    });
  }
}

/** Where one harness ended on a machine, in words. */
export function harnessCheckLine(harness: Pick<Harness, "installed" | "auth"> | undefined): string {
  if (!harness) return "That harness is not listed on the machine now.";
  if (!harness.installed) return "Not installed on that machine.";
  if (harness.auth === "needs_login") return "Needs sign-in on that machine.";
  return "Installed and signed in.";
}

/**
 * A Check on every harness of one machine at once: one request re-detects them
 * all (never one request per harness, one after the other, where a slow CLI
 * would stall the rest), then one read of the snapshot. Never throws: a
 * machine that does not answer gives every harness the same error.
 */
export async function checkMachineHarnesses(host: string, ids: string[]): Promise<Record<string, string>> {
  const lines: Record<string, string> = {};
  if (ids.length === 0) return lines;
  try {
    await machineRequest(host, Method.harnessAccountCheck, { redetect: ids }, HARNESS_CHECK_MS);
    await loadMachineHarness(host);
    const snapshot = useMachineHarnesses.getState().byHost[host];
    const readError = useMachineHarnesses.getState().errors[host];
    for (const id of ids) {
      lines[id] = readError
        ? readError
        : harnessCheckLine(snapshot?.harnesses.find((entry) => entry.id === id));
    }
  } catch (error) {
    for (const id of ids) lines[id] = errorText(error);
  }
  return lines;
}

/** A switch or a default is a write the engine answers with its snapshot. */
const HARNESS_WRITE_MS = 15_000;

/** A Check starts each CLI once; it may take a while, but it ends. */
const HARNESS_CHECK_MS = 30_000;

/** Every chosen machine's harness snapshot, loaded as needed (one hook, not one per row). */
export function useMachineHarnessList(hosts: string[]): {
  host: string;
  snapshot: HarnessSnapshot | null;
  loading: boolean;
  error: string | null;
}[] {
  const local = useApp((state) => state.harnesses);
  const byHost = useMachineHarnesses((state) => state.byHost);
  const loading = useMachineHarnesses((state) => state.loading);
  const errors = useMachineHarnesses((state) => state.errors);
  const key = hosts.join("|");
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` names the target set
  useEffect(() => {
    for (const host of hosts) {
      if (host !== LOCAL_HOST && !byHost[host] && !loading[host]) void loadMachineHarness(host);
    }
  }, [key]);
  return hosts.map((host) => ({
    host,
    snapshot: host === LOCAL_HOST ? local : (byHost[host] ?? null),
    loading: loading[host] === true,
    error: errors[host] ?? null,
  }));
}

/** Enable or disable one harness on one machine; answers with the machine's fresh snapshot. */
export async function writeHarnessEnabled(host: string, id: string, enabled: boolean): Promise<void> {
  if (host === LOCAL_HOST) {
    const { setHarnessEnabled } = await import("./store");
    await setHarnessEnabled(id, enabled);
    return;
  }
  const snapshot = parseHarnessSnapshot(
    await machineRequest(host, Method.harnessSet, { id, enabled }, HARNESS_WRITE_MS),
  );
  useMachineHarnesses.setState((state) => ({
    byHost: { ...state.byHost, [host]: snapshot },
  }));
}

/** Set the default harness, model and effort on one machine. */
export async function writeDefaultHarness(host: string, choice: NewThreadChoice): Promise<void> {
  if (host === LOCAL_HOST) {
    const { setDefaultHarness } = await import("./store");
    await setDefaultHarness(choice);
    return;
  }
  const snapshot = parseHarnessSnapshot(
    await machineRequest(
      host,
      Method.harnessDefault,
      { harness: choice.harness, model: choice.model, effort: choice.effort },
      HARNESS_WRITE_MS,
    ),
  );
  useMachineHarnesses.setState((state) => ({
    byHost: { ...state.byHost, [host]: snapshot },
  }));
}

export interface ScopedDefaultHarness {
  /** The snapshot whose form is shown: this machine's under All machines. */
  snapshot: HarnessSnapshot | null;
  value: NewThreadChoice | null;
  mixed: boolean;
  breakdown: ScopeBreakdownEntry[];
  targets: ScopeMachine[];
  writes: ScopeMachine[];
  error: string | null;
  /** Read the shown machine's harnesses again after a failure. */
  reload: () => void;
  set: (choice: NewThreadChoice) => Promise<void>;
  /** With one project chosen: where the default comes from, and removing the override. */
  project: ProjectRowState | null;
}

/** The default harness/model/effort across the chosen machines, and one write for all. */
export function useScopedDefaultHarness(): ScopedDefaultHarness {
  const { scope, machines, writes } = useScope();
  const targets = scopeTargets(scope, machines);
  const local = useApp((state) => state.harnesses);
  const remote = useMachineHarnesses((state) => state.byHost);
  const loading = useMachineHarnesses((state) => state.loading);
  const errors = useMachineHarnesses((state) => state.errors);
  const hosts = targets.map((machine) => machine.host);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `hosts` names the target set
  useEffect(() => {
    for (const host of hosts) {
      if (host !== LOCAL_HOST && !remote[host] && !loading[host]) void loadMachineHarness(host);
    }
  }, [hosts.join("|")]);
  const snapshotFor = (host: string) => (host === LOCAL_HOST ? local : (remote[host] ?? null));
  const entries: ScopeValueEntry[] = targets.map((machine) => ({
    ...machine,
    value: (snapshotFor(machine.host)?.default ?? undefined) as unknown as Json | undefined,
  }));
  const shownHost = scope.mode === "machine" ? targets[0]?.host : LOCAL_HOST;
  const snapshot = shownHost === undefined ? null : snapshotFor(shownHost);
  const mixed = scopeValuesMixed(entries);
  const value = snapshot?.default ?? null;
  const format = (candidate: Json | undefined) => {
    const choice = candidate as unknown as NewThreadChoice | undefined;
    return choice?.harness ? `${choice.harness}${choice.model ? ` · ${choice.model}` : ""}` : "Not read";
  };
  const chosen = useProjectScope((state) => state.project);
  const answers = useProjectValues((state) => state.answers);
  useLoadProjectSettings(chosen, targets);
  if (chosen) {
    const layers = projectLayers(
      "default-harness",
      chosen,
      targets,
      (host) => (snapshotFor(host)?.default ?? undefined) as unknown as Json | undefined,
      answers,
      null,
      format,
    );
    const writable = layers.reached.filter((target) => target.machine.online);
    const write = async (choice: NewThreadChoice | null) => {
      const value = choice
        ? ({
            harness: choice.harness,
            ...(choice.model ? { model: choice.model } : {}),
            ...(choice.effort ? { effort: choice.effort } : {}),
          } as JsonObject)
        : null;
      const result = await runOnMachines(
        writable.map((target) => target.machine),
        (machine) =>
          writeProjectSetting(
            machine.host,
            writable.find((target) => target.machine.host === machine.host)!.path,
            "default-harness",
            value,
          ),
      );
      if (result.failures.length) toast(failureSummary(result), "error");
    };
    return {
      snapshot,
      value: (layers.value as unknown as NewThreadChoice | null) ?? value,
      mixed: layers.mixed,
      breakdown: layers.breakdown,
      targets,
      writes: writable.map((target) => target.machine),
      error: shownHost === undefined ? null : (errors[shownHost] ?? null),
      reload: () => {
        if (shownHost !== undefined) void loadMachineHarness(shownHost);
      },
      set: (choice) => write(choice),
      project: {
        name: chosen.name,
        readOnly: null,
        source: layers.source === "default" ? "machine" : layers.source,
        breakdown: layers.breakdown,
        skipped: layers.skipped,
        clear: layers.source === "project" || layers.mixed ? () => write(null) : undefined,
      },
    };
  }
  const set = async (choice: NewThreadChoice) => {
    const result = await runOnMachines(writes, (machine) => writeDefaultHarness(machine.host, choice));
    if (result.failures.length) toast(failureSummary(result), "error");
  };
  return {
    project: null,
    snapshot,
    value,
    mixed,
    breakdown: scopeBreakdown(entries, format),
    targets,
    writes,
    error: shownHost === undefined ? null : (errors[shownHost] ?? null),
    reload: () => {
      if (shownHost !== undefined) void loadMachineHarness(shownHost);
    },
    set,
  };
}

export { errorText };
