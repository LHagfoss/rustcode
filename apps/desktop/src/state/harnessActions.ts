// Harnesses: listing and configuring them, their accounts and models, and the harness, model and
// effort a new thread starts on or a thread switches to. Split out of state/store.ts; the state these
// write (`harnesses`, `newThread`, `newThreadAccount`, the model caches) still lives in the app store.

import { type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import {
  fallbackHarnessSnapshot,
  type HarnessAccount,
  type HarnessLast,
  type HarnessSnapshot,
  Method,
  parseConfigOptions,
  parseHarnessLast,
  parseHarnessModels,
  parseHarnessSnapshot,
  parseSessionHarness,
} from "../acp/types";
import { type EffortMemory, restoreChoice } from "./harness";
import { newThreadAccountOn } from "./logins";
import { reloadThread } from "./lostTurns";
import { CLAUDE_CODE, markClaudePlansStarted, refreshPlanCache, refreshPlanUsage } from "./planUsage";
import {
  type AppState,
  connectionFor,
  connectionForThread,
  engine,
  errorText,
  harnessesOn,
  lacksModels,
  loadMachineHarnesses,
  type NewThreadChoice,
  newThreadChoice,
  STORAGE,
  store,
  stored,
  toast,
  updateThread,
  useApp,
} from "./store";
import { isRunning } from "./thread";
import { LOCAL_HOST, splitThreadKey } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/**
 * Every answer carries the whole harness snapshot, and answers to requests in
 * flight together arrive in any order. A request takes a number when it is
 * sent, and an answer replaces the snapshot only when no later request's
 * answer has been applied already: a late, older one would bring back a stale
 * detection (a Ready harness read as Not installed).
 */
let harnessRequestSeq = 0;
let harnessSnapshotSeq = 0;

function applyHarnessSnapshot(seq: number, snapshot: HarnessSnapshot) {
  if (seq < harnessSnapshotSeq) return;
  harnessSnapshotSeq = seq;
  set({ harnesses: snapshot });
}

export async function refreshHarnesses(): Promise<HarnessSnapshot | null> {
  const conn = connectionFor("");
  if (!conn) return null;
  const seq = ++harnessRequestSeq;
  try {
    const snapshot = parseHarnessSnapshot(await conn.request(Method.harnesses, {}, 15_000));
    applyHarnessSnapshot(seq, snapshot);
    if (!snapshot.onboarded) set({ onboardingOpen: true });
    return snapshot;
  } catch (error) {
    // An engine without the harness layer: the built-in harness only, no onboarding.
    const snapshot = fallbackHarnessSnapshot();
    if (!(error instanceof RpcError && error.isMethodNotFound)) {
      toast(`Could not read harnesses: ${errorText(error)}`, "error");
    }
    set({ harnesses: snapshot });
    return snapshot;
  }
}

/** `report: false` lets the caller word the failure itself, so no toast says it twice. */
async function harnessCall(
  method: string,
  params: JsonObject,
  report = true,
): Promise<HarnessSnapshot | null> {
  const seq = ++harnessRequestSeq;
  try {
    const snapshot = parseHarnessSnapshot(await engine().request(method, params, 15_000));
    applyHarnessSnapshot(seq, snapshot);
    return snapshot;
  } catch (error) {
    if (!report) throw error;
    toast(errorText(error), "error");
    return null;
  }
}

export async function setHarnessAdvanced(change: JsonObject): Promise<void> {
  await harnessCall(Method.harnessSet, change, false);
  // The set response already invalidates detection and returns a fresh snapshot.
}

export async function setHarnessEnabled(id: string, enabled: boolean): Promise<HarnessSnapshot | null> {
  // The switch in Settings shows the new state; only a failure needs a word.
  return harnessCall(Method.harnessSet, { id, enabled });
}
/** The account id a harness was last given for new threads; null is its default login. */
export function lastAccountFor(harness: string): string | null {
  try {
    const raw = obj(JSON.parse(stored(STORAGE.harnessAccountChoices) ?? "{}")) ?? {};
    return str(raw[harness]) ?? null;
  } catch {
    return null;
  }
}

function storeAccount(harness: string, account: string | null) {
  let all: JsonObject;
  try {
    all = obj(JSON.parse(stored(STORAGE.harnessAccountChoices) ?? "{}")) ?? {};
  } catch {
    all = {};
  }
  if (account) all[harness] = account;
  else delete all[harness];
  store(STORAGE.harnessAccountChoices, JSON.stringify(all));
}

/** The named accounts of `harness`, as the snapshot lists them, on the computer `host` names. */
export function accountsOf(
  s: { harnesses: HarnessSnapshot | null; remoteHarnesses?: Record<string, HarnessSnapshot> },
  harness: string | undefined,
  host?: string,
): HarnessAccount[] {
  return harness ? (harnessesOn(s, host)?.harnesses.find((h) => h.id === harness)?.accounts ?? []) : [];
}

/** The domain part of an account name, as the account's id must read. */
export function accountSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** The user picked an account for new threads on the current harness. */
export function pickNewThreadAccount(account: string | null, host = get().newThreadHost) {
  const s = get();
  const harness = newThreadChoice({ ...s, newThreadHost: host })?.harness;
  if (harness === CLAUDE_CODE) markClaudePlansStarted();
  const newThreadAccounts = { ...s.newThreadAccounts, [host]: account };
  set((st) => ({
    newThreadAccounts,
    ...(st.newThreadHost === host ? { newThreadAccount: account } : {}),
  }));
  store(STORAGE.newThreadAccounts, JSON.stringify(newThreadAccounts));
  if (harness) storeAccount(harness, account);
  // The login picked may never have been read: its own meter numbers come now.
  void refreshPlanCache();
}

/**
 * The user picked a chip in another harness's header: that harness's account is
 * remembered for new threads, and the new thread moves to that harness on the
 * model and effort it was last left on.
 */
export function pickNewThreadAccountOn(harness: string, account: string) {
  const host = get().newThreadHost;
  if (harness === CLAUDE_CODE) markClaudePlansStarted();
  storeAccount(harness, account);
  if (newThreadChoice(get())?.harness !== harness) switchNewThreadHarness(harness);
  const newThreadAccounts = { ...get().newThreadAccounts, [host]: account };
  set({ newThreadAccount: account, newThreadAccounts });
  store(STORAGE.newThreadAccounts, JSON.stringify(newThreadAccounts));
  void refreshPlanCache();
}

export const addHarnessAccount = (harness: string, id: string, name: string, configDir?: string) =>
  harnessCall(Method.harnessAccountAdd, {
    harness,
    id,
    name,
    ...(configDir ? { config_dir: configDir } : {}),
  });
export const removeHarnessAccount = (harness: string, id: string) =>
  harnessCall(Method.harnessAccountRemove, { harness, id });

/**
 * Reads each account's own credentials again (nothing is started, no token is
 * read). `readPlans` also re-reads the Claude plan rows, which a 3-second
 * wait for a sign-in does not need: that would force a Keychain read each time.
 */
export async function checkHarnessAccounts(
  readPlans = true,
  /** Harnesses detected again instead of read from the engine's last-minute detection. */
  redetect: string[] = [],
  /** False throws a failure instead of a toast. */
  report = true,
): Promise<HarnessSnapshot | null> {
  const snapshot = await harnessCall(Method.harnessAccountCheck, { redetect }, report);
  // Claude Code signs in with its own credentials, which that snapshot does not
  // report (`auth_probe: None`), and its plan read pauses once it finds the
  // default login signed out or cannot read it (a denied Keychain prompt), so
  // neither asks again and again. An explicit check is the user asking to look
  // now: read the default login again, or it would stay as it was found after
  // signing in or out.
  if (readPlans) void refreshPlanCache(true);
  return snapshot;
}

export const removeHarness = (id: string) => harnessCall(Method.harnessRemove, { id });

/**
 * Registers a custom ACP harness from a local command (`_rustcode/harnesses/set`): the command is
 * launched directly, one argument per entry, never through a shell. Off until it is verified.
 * Throws the engine's refusal so the form can show it.
 */
export async function addCustomHarness(request: {
  id: string;
  name: string;
  command: string[];
  env: Record<string, string>;
}): Promise<void> {
  await harnessCall(Method.harnessSet, { ...request, kind: "acp", enabled: false }, false);
}

/** Turns a verified custom harness on, or takes an unverified one out again; a failure throws. */
export const enableCustomHarness = (id: string) =>
  harnessCall(Method.harnessSet, { id, enabled: true }, false);
export const discardCustomHarness = (id: string) => harnessCall(Method.harnessRemove, { id }, false);
/**
 * The Settings default: it is also where a new thread starts, so a draft the new-thread screen
 * kept for the session is dropped once the engine took the change, or the new default would never
 * reach that screen. A default the engine refused keeps the draft.
 */
export async function setDefaultHarness(choice: NewThreadChoice): Promise<HarnessSnapshot | null> {
  const snapshot = await harnessCall(Method.harnessDefault, {
    harness: choice.harness,
    model: choice.model,
    effort: choice.effort,
  });
  if (snapshot) clearNewThreadDraft(LOCAL_HOST);
  return snapshot;
}
export const completeOnboarding = async () => {
  const snapshot = await harnessCall(Method.onboardingComplete, {});
  if (snapshot) set({ onboardingOpen: false });
};

/** "Test": ask the harness for its models. Throws the harness's error so the card can show it. */
export async function fetchHarnessModels(id: string, refresh = false): Promise<number> {
  // `refresh` starts the harness again even when its models are known: a Check.
  const result = (await engine().request(
    Method.harnessModels,
    refresh ? { harness: id, refresh } : { harness: id },
    60_000,
  )) as JsonObject | null;
  const models = parseHarnessModels(result?.models as Json);
  set((s) =>
    s.harnesses
      ? {
          harnesses: {
            ...s.harnesses,
            harnesses: s.harnesses.harnesses.map((h) =>
              h.id === id ? { ...h, models, auth: h.auth === "unknown" ? "ok" : h.auth, error: null } : h,
            ),
          },
        }
      : s,
  );
  // The engine may have learned more (auth, version); take its snapshot too.
  if (refresh) await refreshHarnesses();
  else void refreshHarnesses();
  return models.length;
}

/** Harnesses a Refresh models lists at once: each is a process the harness starts. */
const REFRESH_CONCURRENCY = 2;

/** The Refresh models in flight, one per machine: pressing it again joins it. */
const refreshing = new Map<string, Promise<number>>();

/**
 * "Refresh models" in the model picker: start every enabled harness on `host` again and list its
 * models, rather than answer from the engine's model cache (`_rustcode/harnesses/models` with
 * `refresh`), so a model a harness gained since it was last listed shows at once. Then the
 * machine's snapshot is read again. Answers how many harnesses failed to list; a failure keeps
 * the harness's last list.
 */
export function refreshAllHarnessModels(host = LOCAL_HOST): Promise<number> {
  let run = refreshing.get(host);
  if (!run) {
    run = refreshMachineModels(host).finally(() => refreshing.delete(host));
    refreshing.set(host, run);
  }
  return run;
}

async function refreshMachineModels(host: string): Promise<number> {
  const conn = connectionFor(host);
  if (!conn) return 0;
  const listed = host ? get().remoteHarnesses[host] : get().harnesses;
  const ids = (listed?.harnesses ?? []).filter((h) => !h.builtin && h.enabled).map((h) => h.id);
  const queue = [...ids];
  let failed = 0;
  const worker = async () => {
    for (let id = queue.shift(); id; id = queue.shift()) {
      try {
        await conn.request(Method.harnessModels, { harness: id, refresh: true }, 60_000);
      } catch {
        failed++;
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(REFRESH_CONCURRENCY, ids.length) }, worker));
  forgetAskedModels();
  if (host) await loadMachineHarnesses(host, { report: false, refresh: true });
  else await refreshHarnesses();
  return failed;
}

/** Model listings in flight (ensureHarnessModels), one per harness. */
const modelsAsked = new Map<string, Promise<void>>();

/** Forget which harnesses were asked for models, so the next look asks again. */
export function forgetAskedModels(): void {
  modelsAsked.clear();
}

/**
 * An external harness lists its models only when asked (`_rustcode/harnesses/models`;
 * the engine caches them across restarts, but a harness never listed has none
 * yet). The model picker of a new thread on it, and a restored thread on it,
 * ask, so the model list, effort and Fast mode are there before the harness
 * runs. A failure stays quiet (the picker says the harness picks its default)
 * and is not kept: the next ask, such as opening the picker, tries again.
 */
export function ensureHarnessModels(id: string, host = LOCAL_HOST): Promise<void> {
  if (host) return ensureMachineHarnessModels(id, host);
  const harness = get().harnesses?.harnesses.find((h) => h.id === id);
  if (!harness || harness.builtin || harness.models.length || get().engine.phase !== "ready")
    return Promise.resolve();
  let asked = modelsAsked.get(id);
  if (!asked) {
    asked = fetchHarnessModels(id)
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => modelsAsked.delete(id));
    modelsAsked.set(id, asked);
  }
  return asked;
}

/**
 * The same on a paired machine: its own `_rustcode/harnesses/models` lists them (`docs/desktop.md`
 * › Several machines), and the machine's snapshot is read again so the picker has them. A machine
 * set up fresh has learned none until asked, and its picker would otherwise stay empty. An older
 * machine whose gateway refuses the method keeps what its snapshot carries, quietly.
 */
function ensureMachineHarnessModels(id: string, host: string): Promise<void> {
  const harness = get().remoteHarnesses[host]?.harnesses.find((h) => h.id === id);
  if (!harness || harness.builtin || harness.models.length) return Promise.resolve();
  const conn = connectionFor(host);
  if (!conn) return Promise.resolve();
  const key = `${host}\u0000${id}`;
  let asked = modelsAsked.get(key);
  if (!asked) {
    asked = conn
      .request(Method.harnessModels, { harness: id }, 60_000)
      .then(async () => {
        if (connectionFor(host) !== conn) return;
        const snapshot = parseHarnessSnapshot(await conn.request(Method.harnesses, {}, 15_000));
        if (connectionFor(host) !== conn) return;
        set((st) => ({ remoteHarnesses: { ...st.remoteHarnesses, [host]: snapshot } }));
      })
      .catch(() => undefined)
      .finally(() => modelsAsked.delete(key));
    modelsAsked.set(key, asked);
  }
  return asked;
}

export function setNewThreadChoice(choice: NewThreadChoice | null, host = get().newThreadHost) {
  const newThreadDrafts = { ...get().newThreadDrafts };
  if (choice) newThreadDrafts[host] = choice;
  else delete newThreadDrafts[host];
  set((s) => ({
    newThreadDrafts,
    ...(s.newThreadHost === host ? { newThread: choice } : {}),
  }));
  store(STORAGE.newThreadDrafts, JSON.stringify(newThreadDrafts));
}

/** Drops the persisted new-thread draft for `host`, so its default takes over again. */
export function clearNewThreadDraft(host = LOCAL_HOST) {
  setNewThreadChoice(null, host);
}

/**
 * One harness's choice as the app keeps it: the engine's `last` shape, plus the effort each of
 * its models was last left on. Keyed by harness only, so its logins share one memory.
 */
export interface LocalChoice extends HarnessLast {
  efforts?: Record<string, string>;
}

/** The per-model efforts in one stored entry, dropping anything that is not a model and a level. */
function parseEfforts(value: Json | undefined): Record<string, string> {
  const raw = obj(value) ?? {};
  const out: Record<string, string> = {};
  for (const [model, level] of Object.entries(raw)) {
    const effort = str(level);
    if (model && effort) out[model] = effort;
  }
  return out;
}

/**
 * What the app keeps per harness: the choice an engine that does not remember starts from, and the
 * effort each model was last left on. An entry for an engine that remembers carries only `efforts`
 * (no model, no effort), which is why one is kept even when there is no model.
 */
export function localChoices(): Record<string, LocalChoice> {
  try {
    const raw = obj(JSON.parse(stored(STORAGE.harnessChoices) ?? "{}")) ?? {};
    const out: Record<string, LocalChoice> = {};
    for (const [id, value] of Object.entries(raw)) {
      const last = parseHarnessLast(value);
      const efforts = parseEfforts(obj(value)?.efforts);
      const known = Object.keys(efforts).length > 0;
      // The shape before per-model memory had no `efforts`: it reads as a model and an effort.
      if (last) out[id] = known ? { ...last, efforts } : last;
      else if (known) out[id] = { model: null, effort: null, efforts };
    }
    return out;
  } catch {
    return {};
  }
}

/** The effort each model of each harness was last left on (state/harness.ts `EffortMemory`). */
export function effortMemory(): EffortMemory {
  const out: EffortMemory = {};
  for (const [id, choice] of Object.entries(localChoices())) {
    if (choice.efforts) out[id] = choice.efforts;
  }
  return out;
}

/**
 * Keeps the effort a model was last left on, so picking that model again returns to its level.
 * Keyed by harness and model only, never a login: every account of a harness shares it.
 */
export function rememberModelEffort(
  harness: string | null | undefined,
  model: string | null | undefined,
  effort: string | null | undefined,
) {
  if (!harness || !model || !effort) return;
  const all = localChoices();
  const entry = all[harness];
  // The harness's own last model and effort stay as they are; an entry that has none (an engine
  // that remembers keeps them itself) keeps them empty, so only the memory is added.
  all[harness] = {
    model: entry?.model ?? null,
    effort: entry?.effort ?? null,
    efforts: { ...entry?.efforts, [model]: effort },
  };
  store(STORAGE.harnessChoices, JSON.stringify(all));
}

/** The app's own record of choices, only for an engine that keeps none. */
export function appChoices(snapshot: HarnessSnapshot): Record<string, HarnessLast> | undefined {
  return snapshot.remembers ? undefined : localChoices();
}

/** Mirror a harness's last choice: in the snapshot, and in app storage when the engine does not keep it. */
export function noteChoice(harness: string, last: HarnessLast, host = LOCAL_HOST) {
  const value = last.model || last.effort ? last : null;
  if (host) {
    set((s) => {
      const snapshot = s.remoteHarnesses[host];
      if (!snapshot) return s;
      return {
        remoteHarnesses: {
          ...s.remoteHarnesses,
          [host]: {
            ...snapshot,
            harnesses: snapshot.harnesses.map((h) => (h.id === harness ? { ...h, last: value } : h)),
          },
        },
      };
    });
    return;
  }
  set((s) =>
    s.harnesses
      ? {
          harnesses: {
            ...s.harnesses,
            harnesses: s.harnesses.harnesses.map((h) => (h.id === harness ? { ...h, last: value } : h)),
          },
        }
      : s,
  );
  if (get().harnesses?.remembers) return;
  const all = localChoices();
  // The per-model memory outlives the harness's own last choice: a write keeps it.
  const efforts = all[harness]?.efforts;
  if (value) all[harness] = efforts ? { ...value, efforts } : value;
  else if (efforts) all[harness] = { model: null, effort: null, efforts };
  else delete all[harness];
  store(STORAGE.harnessChoices, JSON.stringify(all));
}

/**
 * The user picked a model or effort in the new-thread picker: it is the choice, and the harness's
 * remembered one (`_rustcode/harnesses/remember`, else app storage), so switching harness and back,
 * navigating into a thread and back, or a restart, starts there again. The model keeps its own
 * effort for the next time it is picked. `host` is the computer the thread runs on: each host keeps
 * its own unstarted draft, and only this computer is asked to remember its engine-side choice.
 */
export function pickNewThreadChoice(choice: NewThreadChoice, host = LOCAL_HOST) {
  setNewThreadChoice(choice, host);
  const last = { model: choice.model, effort: choice.effort };
  noteChoice(choice.harness, last, host);
  rememberModelEffort(choice.harness, choice.model, choice.effort);
  if (host || !get().harnesses?.remembers || !connectionFor("")) return;
  void engine()
    .request(Method.harnessRemember, { harness: choice.harness, ...last }, 15_000)
    .catch(() => undefined);
}

/** Switch the new-thread picker to `harness`, on the model and effort it was last left on. */
export function switchNewThreadHarness(harness: string) {
  // The machine the thread will run on has its own harnesses and its own defaults.
  const s = get();
  const host = s.newThreadHost;
  const snapshot = harnessesOn(s, host);
  if (!snapshot) return;
  const next = snapshot.harnesses.find((h) => h.id === harness);
  const remembered = lastAccountFor(harness);
  const choice = restoreChoice(snapshot, harness, appChoices(snapshot), effortMemory());
  // A harness with named accounts never starts on its own sign-in.
  const account = newThreadAccountOn(next, remembered, s.signedOutLogins);
  const newThreadDrafts = { ...s.newThreadDrafts, [host]: choice };
  const newThreadAccounts = { ...s.newThreadAccounts, [host]: account };
  set({
    newThread: choice,
    newThreadAccount: account,
    newThreadDrafts,
    newThreadAccounts,
  });
  store(STORAGE.newThreadDrafts, JSON.stringify(newThreadDrafts));
  store(STORAGE.newThreadAccounts, JSON.stringify(newThreadAccounts));
  if (harness === CLAUDE_CODE) markClaudePlansStarted();
  // Another harness means another login, whose meter may have no numbers yet.
  void refreshPlanCache();
}

/**
 * Move an idle thread to another account of its harness: the new account's
 * process starts, the conversation moves as a handoff summary with the next
 * message, and the thread keeps its history. Never while a turn runs.
 */
export async function switchThreadAccount(id: string, account: string | null): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || isRunning(thread)) return false;
  const harness = thread.harness?.id;
  if (!harness) return false;
  const host = splitThreadKey(id).host;
  const named = accountsOf(get(), harness, host).find((a) => a.id === account);
  try {
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    const result = (await conn.request(
      Method.sessionHarness,
      { sessionId: splitThreadKey(id).id, harness, account },
      30_000,
    )) as JsonObject | null;
    const next = parseSessionHarness(result) ?? thread.harness;
    updateThread(id, (t) => ({
      ...t,
      harness: next,
      configOptions: parseConfigOptions(result?.configOptions),
    }));
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, account: next?.account } : x)),
    }));
    const harnessName = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness)?.name ?? harness;
    toast(
      named
        ? `This thread now runs on ${named.name}. The conversation so far moves over as a summary with your next message.`
        : `This thread now runs on ${harnessName}'s own sign-in, with the conversation so far as a summary.`,
      "success",
    );
    void refreshPlanUsage(id);
    if (harness === CLAUDE_CODE) markClaudePlansStarted();
    // The account switched to may never have been read: its meter numbers come now.
    void refreshPlanCache();
    return true;
  } catch (error) {
    toast(`Could not switch account: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Move a thread to another harness: the
 * harness starts fresh and its first prompt carries the conversation so far
 * (`_rustcode/session/harness`). It starts on the model and effort it was
 * last left on; the thread shows a divider where it switched. A thread moved to RustCode itself
 * is opened again, since the engine runs it as a built-in thread from then on.
 */
export async function switchThreadHarness(
  id: string,
  harness: string,
  account?: string | null,
  choice: { model?: string; effort?: string; reason?: "limit"; quiet?: boolean } = {},
): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || isRunning(thread)) return false;
  const host = splitThreadKey(id).host;
  const last = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness)?.last;
  try {
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    const result = (await conn.request(
      Method.sessionHarness,
      {
        sessionId: splitThreadKey(id).id,
        harness,
        account: account ?? null,
        model: choice.model ?? last?.model ?? null,
        effort: choice.effort ?? last?.effort ?? null,
        // The engine records why in the thread's activity (`thread.limit_switch`).
        ...(choice.reason ? { reason: choice.reason } : {}),
      },
      30_000,
    )) as JsonObject | null;
    const listed = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness);
    const next =
      parseSessionHarness(result) ?? (listed && { id: harness, capabilities: listed.capabilities });
    updateThread(id, (t) => ({
      ...t,
      harness: next,
      configOptions: parseConfigOptions(result?.configOptions),
    }));
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, harness, account: next?.account ?? account ?? undefined } : x,
      ),
    }));
    if (harness === CLAUDE_CODE) markClaudePlansStarted();
    if (harness === "rustcode") await reloadThread(id);
    const opened = get().threads[id];
    if (opened && lacksModels(opened)) void ensureHarnessModels(harness, host);
    void refreshPlanUsage(id);
    void refreshPlanCache();
    return true;
  } catch (error) {
    if (!choice.quiet) toast(`Could not switch harness: ${errorText(error)}`, "error");
    return false;
  }
}
