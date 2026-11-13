// What a Settings row changes: this app, a server (one machine or all of
// them), or one machine only (docs/desktop.md › Settings on one machine or
// all). One table, so a row cannot move between kinds by accident and the
// scope bar, the pages and the tests all read the same answer.
//
// Server settings are read from and written to the machine that runs them.
// Machine-only settings — credentials, accounts, local paths, the remote
// gateway and the machines list — are shown for one machine at a time and are
// never sent to another machine.

import type { Json } from "../acp/jsonrpc";
import type { SettingsSection } from "./store";

export type SettingKind = "app" | "server" | "machine";

/**
 * What a page is mostly about. A page with any server row shows the
 * **Applying to** selector; a page whose rows are all app preferences shows
 * none; a machine-only page shows one machine at a time.
 */
export const SECTION_KIND: Record<SettingsSection, SettingKind> = {
  general: "server",
  chat: "app",
  agents: "server",
  connections: "machine",
  capabilities: "server",
  remote: "machine",
  machines: "machine",
  appearance: "app",
  keyboard: "app",
  usage: "app",
  statistics: "app",
  advanced: "server",
  diagnostics: "machine",
  about: "app",
};

/** The rows whose kind differs from their page's, or that a page picks by name. */
const ROW_KINDS: { section: SettingsSection; title: string; kind: SettingKind }[] = [
  // General keeps app preferences and this computer's own rows beside the server settings.
  { section: "general", title: "Default project folder", kind: "machine" },
  { section: "general", title: "Harness, model and effort", kind: "server" },
  { section: "general", title: "Default access", kind: "server" },
  { section: "general", title: "Continue threads after a restart", kind: "server" },
  { section: "general", title: "Continue threads on their own", kind: "server" },
  { section: "general", title: "Auto-compact", kind: "server" },
  { section: "general", title: "Auto-compact threshold", kind: "server" },
  { section: "general", title: "System notifications", kind: "app" },
  { section: "general", title: "Only when RustCode is in the background", kind: "app" },
  { section: "general", title: "While-you-were-away recap", kind: "app" },
  { section: "general", title: "Away for", kind: "app" },
  { section: "general", title: "Discord Rich Presence", kind: "machine" },
  { section: "general", title: "Tab moves focus between controls", kind: "app" },
  { section: "general", title: "Trackpad haptics", kind: "app" },
  // Harnesses: the default is a server setting, the cards and accounts are machine-only.
  { section: "agents", title: "New threads start on", kind: "server" },
  { section: "agents", title: "Harnesses", kind: "server" },
  { section: "agents", title: "Claude accounts", kind: "machine" },
  { section: "agents", title: "An account's sign-in command", kind: "machine" },
  { section: "agents", title: "Threads can start and message other threads", kind: "server" },
  { section: "agents", title: "Delegation", kind: "server" },
  { section: "agents", title: "Delegates across all threads", kind: "server" },
  { section: "agents", title: "Delegates one thread can run at once", kind: "server" },
  { section: "agents", title: "Let agents read threads in other projects", kind: "server" },
  // Credentials and accounts never leave their machine.
  { section: "connections", title: "GitHub", kind: "machine" },
  { section: "connections", title: "Model accounts", kind: "machine" },
  // Skills and MCP servers are server settings, listed per machine.
  { section: "capabilities", title: "MCP servers", kind: "server" },
  { section: "capabilities", title: "Skills", kind: "server" },
  // The rest of the machine-only pages.
  { section: "remote", title: "Share this computer's threads with a phone", kind: "machine" },
  { section: "remote", title: "Gateway address", kind: "machine" },
  { section: "machines", title: "Environments", kind: "machine" },
  // Advanced's telemetry switches are kept by each server.
  { section: "advanced", title: "Provider request bodies", kind: "server" },
  { section: "advanced", title: "Harness messages", kind: "server" },
];

const ROW_INDEX = new Map(ROW_KINDS.map((row) => [`${row.section}\u0000${row.title}`, row.kind]));

/** What one Settings row changes, by its page and its title. */
export function rowKind(section: SettingsSection, title: string): SettingKind {
  return ROW_INDEX.get(`${section}\u0000${title}`) ?? SECTION_KIND[section];
}

/** Whether a page shows the **Applying to** selector (it has server rows). */
export function pageHasServerSettings(section: SettingsSection): boolean {
  if (SECTION_KIND[section] === "server") return true;
  return ROW_KINDS.some((row) => row.section === section && row.kind === "server");
}

/** Whether a page is machine-only: its rows are shown for one machine at a time. */
export function pageIsMachineOnly(section: SettingsSection): boolean {
  return SECTION_KIND[section] === "machine" && !pageHasServerSettings(section);
}

// ------------------------------------------------------------ server settings

/** The generic server settings, kept in each machine's `settings.json`. */
export type ServerSettingKey =
  | "default-permission-mode"
  | "resume-after-restart"
  | "continue-after-limit"
  | "auto-compact-mode"
  | "auto-compact-threshold"
  | "orchestration"
  | "delegates-total"
  | "delegates-per-thread"
  | "read-across-projects"
  | "delegate-presets";

export interface ServerSettingSpec {
  key: ServerSettingKey;
  label: string;
  type: "boolean" | "number" | "text" | "list";
  default: Json;
}

/**
 * The same keys the engine's `_rustcode/settings` registry accepts
 * (`rustcode/engine/src/acp/settings.rs`), with the default the engine
 * answers before a machine has stored one.
 */
export const SERVER_SETTINGS: readonly ServerSettingSpec[] = [
  {
    key: "default-permission-mode",
    label: "Default access",
    type: "text",
    default: "ask",
  },
  { key: "resume-after-restart", label: "Continue threads after a restart", type: "boolean", default: true },
  {
    key: "continue-after-limit",
    label: "Continue threads on their own",
    type: "boolean",
    default: true,
  },
  { key: "auto-compact-mode", label: "Auto-compact", type: "text", default: "size" },
  {
    key: "auto-compact-threshold",
    label: "Auto-compact threshold",
    type: "number",
    default: 150_000,
  },
  {
    key: "orchestration",
    label: "Threads can start and message other threads",
    type: "boolean",
    default: true,
  },
  { key: "delegates-total", label: "Delegates across all threads", type: "number", default: 8 },
  { key: "delegates-per-thread", label: "Delegates one thread can run at once", type: "number", default: 4 },
  {
    key: "read-across-projects",
    label: "Let agents read threads in other projects",
    type: "boolean",
    default: true,
  },
  { key: "delegate-presets", label: "Delegate presets", type: "list", default: [] },
];

export const SERVER_SETTING_KEYS: readonly ServerSettingKey[] = SERVER_SETTINGS.map((s) => s.key);

export function serverSetting(key: ServerSettingKey): ServerSettingSpec {
  const found = SERVER_SETTINGS.find((spec) => spec.key === key);
  if (!found) throw new Error(`unknown server setting ${key}`);
  return found;
}

// ------------------------------------------------------------ values

/** One machine's value of a setting; `undefined` until it has been read. */
export interface ScopeValueEntry {
  host: string;
  name: string;
  online: boolean;
  value?: Json;
}

/** How a machine's value reads under **Mixed**. */
export interface ScopeBreakdownEntry extends ScopeValueEntry {
  /** The value in the row's own words, or why it has none. */
  label: string;
}

/**
 * Whether the connected machines disagree about a value. An offline machine
 * keeps its value and is not compared; a machine whose value has not arrived
 * yet does not turn a settled row into Mixed.
 */
export function scopeValuesMixed(entries: ScopeValueEntry[]): boolean {
  const known = entries.filter((entry) => entry.online && entry.value !== undefined);
  if (known.length < 2) return false;
  const first = JSON.stringify(known[0]!.value);
  return known.some((entry) => JSON.stringify(entry.value) !== first);
}

/** One machine a page can show, local or paired. */
export interface ScopedMachine {
  host: string;
  name: string;
  online: boolean;
  local: boolean;
}

/** The panel's one choice for every server setting. */
export type ScopeChoice = { mode: "this" } | { mode: "all" } | { mode: "machine"; host: string };

/** The machines a choice shows: All machines includes the offline ones. */
export function scopedMachines(choice: ScopeChoice, machines: ScopedMachine[]): ScopedMachine[] {
  if (choice.mode === "all") return machines;
  if (choice.mode === "machine") return machines.filter((machine) => machine.host === choice.host);
  return machines.filter((machine) => machine.local);
}

/**
 * The machines a write reaches: All machines skips every offline one, so it
 * keeps its value, and one named offline machine is not written either.
 */
export function writableMachines(choice: ScopeChoice, machines: ScopedMachine[]): ScopedMachine[] {
  return scopedMachines(choice, machines).filter((machine) => machine.online);
}

/** One machine a fan-out failed on, and why. */
export interface MachineRunFailure {
  host: string;
  name: string;
  message: string;
}

/** Every machine a fan-out was sent to, and the ones that failed. */
export interface MachineRunResult {
  attempted: number;
  succeeded: ScopedMachine[];
  failures: MachineRunFailure[];
}

/**
 * Send work to every machine and wait for all of them: one failure never stops
 * the next machine's write, and the result names the machine that failed so
 * the page can say so instead of one anonymous toast.
 */
export async function runOnMachines(
  machines: ScopedMachine[],
  run: (machine: ScopedMachine) => Promise<unknown>,
): Promise<MachineRunResult> {
  const outcomes = await Promise.allSettled(machines.map((machine) => run(machine)));
  const succeeded: ScopedMachine[] = [];
  const failures: MachineRunFailure[] = [];
  outcomes.forEach((outcome, index) => {
    const machine = machines[index]!;
    if (outcome.status === "fulfilled") succeeded.push(machine);
    else
      failures.push({
        host: machine.host,
        name: machine.name,
        message: outcome.reason instanceof Error ? outcome.reason.message : String(outcome.reason),
      });
  });
  return { attempted: machines.length, succeeded, failures };
}

/** "Lars's Linux box: that machine is not connected; Studio: …" */
export function failureSummary(result: Pick<MachineRunResult, "failures">): string {
  return result.failures.map((failure) => `${failure.name}: ${failure.message}`).join("; ");
}

/**
 * The values to put on a server that has none yet: only the keys its
 * `settings.json` does not hold, so a value another machine wrote first stays.
 */
export function missingSettingValues(stored: string[], values: Record<string, Json>): Record<string, Json> {
  return Object.fromEntries(Object.entries(values).filter(([key]) => !stored.includes(key)));
}

/** Each machine's value in words, for the **Mixed** hover card. */
export function scopeBreakdown(
  entries: ScopeValueEntry[],
  format: (value: Json | undefined, entry: ScopeValueEntry) => string,
): ScopeBreakdownEntry[] {
  return entries.map((entry) => ({
    ...entry,
    label: entry.online
      ? format(entry.value, entry)
      : `${format(entry.value, entry)} — offline, keeps its value`,
  }));
}

// ------------------------------------------------------------ copying MCP servers

/** The fields a copy needs; the snapshot never carries a secret value. */
export interface CopyableMcpServer {
  name: string;
  transport: string;
  /** The target when it has nothing to mask; null when a secret would leak. */
  copyTarget: string[] | null;
  envKeys: string[];
  headerKeys: string[];
}

export interface McpCopy {
  name: string;
  transport: string;
  target: string[];
  /** Blank values for the target, by key: the definition, never the secret. */
  env: string[];
  headers: string[];
  /** The keys that must be set on the target machine. */
  secretKeys: string[];
}

const SECRET_WORDS = ["token", "secret", "password", "passwd", "apikey", "api-key", "api_key", "auth"];

/** The engine's `is_secret_name`, so both sides hide the same arguments. */
function isSecretName(name: string): boolean {
  const clean = name.replace(/^-+/, "").toLowerCase();
  return SECRET_WORDS.some((word) => clean.includes(word));
}

/**
 * Why a server target must stay on the machine it was typed on, or null when
 * it can go to another machine. The client-side half of the engine's
 * `copy_target_parts`: a stdio argument masking would hide, or a URL with
 * userinfo, a query or a fragment, never travels in the clear.
 */
export function secretTargetReason(target: string[], transport: string): string | null {
  if (transport === "http" || transport === "sse") {
    const raw = target.join(" ");
    let url: URL;
    try {
      url = new URL(raw);
    } catch {
      return "its URL could not be read";
    }
    if (url.username || url.password) return "its URL carries a user name or password";
    if (url.search) return "its URL carries a query string";
    if (url.hash) return "its URL carries a fragment";
    return null;
  }
  let flag: string | null = null;
  for (const part of target.slice(1)) {
    if (flag) return `its command carries a secret in ${flag}`;
    if (part.includes("=")) {
      const key = part.split("=")[0] ?? "";
      if (isSecretName(key)) return `its command carries a secret in ${key}`;
    } else if (part.startsWith("-") && isSecretName(part)) {
      flag = part;
    }
  }
  return flag ? `its command carries a secret in ${flag}` : null;
}

/**
 * The definition to send to another machine: the target, transport and the
 * names of the environment variables and headers, with every value blank. A
 * target with a masked secret cannot be copied faithfully, so it returns null
 * rather than a broken command.
 */
export function mcpCopy(server: CopyableMcpServer): McpCopy | null {
  if (!server.copyTarget?.length) return null;
  const secretKeys = [...server.envKeys, ...server.headerKeys];
  return {
    name: server.name,
    transport: server.transport,
    target: server.copyTarget,
    env: server.envKeys.map((key) => `${key}=`),
    headers: server.headerKeys.map((key) => `${key}: `),
    secretKeys,
  };
}
