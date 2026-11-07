// Pure harness and onboarding logic: what can be chosen, what is greyed out
// and why. The UI reads capability flags only; it never branches on a harness id.

import type {
  ConfigOption,
  Harness,
  HarnessAccount,
  HarnessCapabilities,
  HarnessLast,
  HarnessModel,
  HarnessSnapshot,
} from "../acp/types";

export type HarnessStatus =
  | { tone: "ok"; label: string }
  | { tone: "warn"; label: string }
  | { tone: "off"; label: string };

/** How one named login is run: the variable naming its directory, and the harness's own sign-in command. */
export interface AccountSignIn {
  env: string;
  command: string[];
}

/**
 * How `harness` signs a named login in, from the engine's own description of
 * it (`account_env`, `account_login_command`): `undefined` for a harness that
 * has no named logins (Pi, OpenCode, Gemini, a custom harness).
 */
export function accountSignIn(
  h: Pick<Harness, "account_env" | "account_login_command">,
): AccountSignIn | undefined {
  if (!h.account_env || !h.account_login_command?.length) return undefined;
  return { env: h.account_env, command: h.account_login_command };
}

/**
 * The copyable line that signs one named login in under its own directory:
 * the harness's own login command, prefixed with the assignment that points it
 * at the directory (nothing at all for the harness's own login, which runs
 * without one). Empty for a harness without named logins.
 */
export function accountLoginCommand(
  h: Pick<Harness, "account_env" | "account_login_command">,
  account: Pick<HarnessAccount, "config_dir" | "own_dir">,
): string {
  const signIn = accountSignIn(h);
  if (!signIn) return "";
  const command = commandLine(signIn.command);
  if (account.own_dir === true) return command;
  return `mkdir -p ${account.config_dir} && ${signIn.env}=${account.config_dir} ${command}`;
}

/**
 * The sign-in state of a harness with named accounts, from the accounts
 * themselves: any signed in makes it ready, only all-known-signed-out makes it
 * blocked, and one not looked at yet is not a reason to block. `undefined`
 * when it has no named accounts and its own `auth` speaks for it.
 */
function accountState(h: Harness): "ok" | "needs_login" | "unknown" | undefined {
  const accounts = h.accounts ?? [];
  if (!accounts.length) return undefined;
  if (accounts.some((account) => account.auth === "ok")) return "ok";
  if (accounts.every((account) => account.auth === "needs_login")) return "needs_login";
  return "unknown";
}

export function harnessStatus(h: Harness): HarnessStatus {
  if (!h.installed) return { tone: "off", label: "Not installed" };
  if (h.error) return { tone: "warn", label: "Error" };
  // A harness with accounts is as ready as its best login: its own sign-in may
  // be missing while `Paral 2` is fine, and the accounts say which.
  const accounts = accountState(h);
  if (accounts === "ok") return { tone: "ok", label: h.version ? `Ready · ${h.version}` : "Ready" };
  if (accounts === "needs_login") return { tone: "warn", label: "Sign in an account" };
  if (accounts === "unknown")
    return { tone: "ok", label: h.version ? `Installed · ${h.version}` : "Installed" };
  if (h.auth === "needs_login") return { tone: "warn", label: "Sign in needed" };
  if (h.auth === "ok") return { tone: "ok", label: h.version ? `Ready · ${h.version}` : "Ready" };
  return { tone: "ok", label: h.version ? `Installed · ${h.version}` : "Installed" };
}

/** Why a harness cannot run a thread right now, or undefined when it can. */
export function unavailableReason(h: Harness): string | undefined {
  if (!h.installed)
    return h.install_hint ? `Not installed. Install with: ${h.install_hint}` : "Not installed";
  if (!h.enabled) return "Turned off in Settings › Agents";
  // With named accounts, the accounts decide: point at them, never at the
  // harness's own sign-in button the card no longer shows.
  const accounts = accountState(h);
  if (accounts === "needs_login") return "Sign in an account";
  if (accounts) return undefined;
  if (h.auth === "needs_login") return "Sign in first";
  return undefined;
}

/** The harness new threads should start on when nobody chose one yet. */
export const PREFERRED_HARNESS = "claude-code";

/**
 * The order harnesses are listed in: Claude Code first, the other coding
 * agents as the engine lists them, and RustCode's own engine last. It runs
 * the TUI, tests and local models, and is not the desktop app's main harness.
 */
export function orderHarnesses(list: Harness[]): Harness[] {
  const rank = (h: Harness) => (h.id === PREFERRED_HARNESS ? 0 : h.builtin ? 2 : 1);
  return [...list].sort((a, b) => rank(a) - rank(b));
}

/** First-run default: Claude Code when it can run, else the first agent that can, else RustCode. */
export function preferredDefault(snapshot: HarnessSnapshot): {
  harness: string;
  model: string | null;
  effort: string | null;
} {
  const installed = (h: Harness) => {
    const accounts = accountState(h);
    return h.installed && (accounts ? accounts !== "needs_login" : h.auth !== "needs_login") && !h.error;
  };
  const pick =
    orderHarnesses(snapshot.harnesses).find((h) => !h.builtin && installed(h)) ??
    snapshot.harnesses.find((h) => h.builtin && h.enabled) ??
    snapshot.harnesses.find((h) => h.builtin);
  return { harness: pick?.id ?? "rustcode", model: null, effort: null };
}

export function usableHarnesses(snapshot: HarnessSnapshot): Harness[] {
  return snapshot.harnesses.filter((h) => unavailableReason(h) === undefined);
}

export function modelEfforts(model: HarnessModel | undefined): string[] {
  return model?.efforts ?? [];
}

/** A default must name a usable harness, a model it lists (when it lists any) and an effort that model offers. */
export function validateDefault(
  snapshot: HarnessSnapshot,
  choice: { harness: string; model: string | null; effort: string | null },
): string | undefined {
  const h = snapshot.harnesses.find((x) => x.id === choice.harness);
  if (!h) return "Choose a harness";
  const reason = unavailableReason(h);
  if (reason) return `${h.name}: ${reason}`;
  if (h.models.length && choice.model && !h.models.some((m) => m.id === choice.model)) {
    return `${h.name} does not list ${choice.model}`;
  }
  const model = h.models.find((m) => m.id === choice.model);
  if (choice.effort && model?.efforts.length && !model.efforts.includes(choice.effort)) {
    return `${model.name} has no ${choice.effort} effort`;
  }
  if (choice.effort && !h.capabilities.effort) return `${h.name} does not take an effort level`;
  return undefined;
}

/** Fix a default choice after the harness or model changed: keep what still fits, else fall back. */
export function normalizeDefault(
  snapshot: HarnessSnapshot,
  choice: { harness: string; model: string | null; effort: string | null },
): { harness: string; model: string | null; effort: string | null } {
  const h = snapshot.harnesses.find((x) => x.id === choice.harness);
  if (!h) return { harness: "rustcode", model: null, effort: null };
  const model =
    h.models.find((m) => m.id === choice.model) ??
    (choice.model && !h.models.length ? undefined : h.models[0]);
  const modelId = model?.id ?? (h.models.length ? null : choice.model);
  let effort = choice.effort;
  if (!h.capabilities.effort) effort = null;
  else if (model?.efforts.length && (!effort || !model.efforts.includes(effort))) {
    effort = model.default_effort ?? model.efforts[0] ?? null;
  }
  return { harness: h.id, model: modelId, effort };
}

/**
 * The choice a new thread on `id` starts with: the harness's remembered model and effort (the
 * engine's `last`, else what the app kept in `local`), else the default when it names this
 * harness, else nothing (the harness picks). A model the harness no longer lists, or an effort its
 * model does not offer, is dropped quietly.
 */
export function restoreChoice(
  snapshot: HarnessSnapshot,
  id: string,
  local?: Record<string, HarnessLast>,
): { harness: string; model: string | null; effort: string | null } {
  const h = snapshot.harnesses.find((x) => x.id === id);
  if (!h) return { harness: "rustcode", model: null, effort: null };
  const last = h.last ?? local?.[id] ?? null;
  const fallback = snapshot.default.harness === id ? snapshot.default : null;
  const listed = (model: string | null) =>
    !!model && (!h.models.length || h.models.some((m) => m.id === model));
  const candidates = [last, fallback].filter((c): c is HarnessLast => !!c);
  const model = candidates.map((c) => c.model).find(listed) ?? null;
  const known = h.models.find((m) => m.id === model);
  const offers = (effort: string | null) =>
    !!effort && h.capabilities.effort && (!known?.efforts.length || known.efforts.includes(effort));
  const effort =
    candidates
      .filter((c) => !c.model || c.model === model)
      .map((c) => c.effort)
      .find(offers) ?? null;
  return { harness: h.id, model, effort };
}

/** The effort a model starts on when none is chosen: its default, else the first it offers. */
export function defaultEffort(model: HarnessModel | undefined, efforts: string[]): string | null {
  const preferred = model?.default_effort;
  if (preferred && efforts.includes(preferred)) return preferred;
  if (efforts.includes("default")) return "default";
  return efforts[0] ?? null;
}

/** A shell-quoted command line for the copy button. */
export function commandLine(command: string[]): string {
  return command
    .map((part) => (/^[\w@%+=:,./-]+$/.test(part) ? part : `'${part.replace(/'/g, `'\\''`)}'`))
    .join(" ");
}

export interface CapabilityRow {
  key: keyof HarnessCapabilities;
  label: string;
  available: boolean;
  detail?: string;
}

/** The capability list a harness card shows, each with whether it works. */
export function capabilityRows(c: HarnessCapabilities): CapabilityRow[] {
  return [
    {
      key: "steer",
      label: "Steer a running turn",
      available: c.steer === "live",
      detail:
        c.steer === "next_turn"
          ? "Read at the next turn"
          : c.steer === "none"
            ? "Messages queue instead"
            : undefined,
    },
    { key: "cancel", label: "Stop a turn", available: c.cancel },
    { key: "resume", label: "Resume after restart", available: c.resume },
    { key: "models", label: "Choose a model", available: c.models !== "none" },
    { key: "effort", label: "Reasoning effort", available: c.effort },
    { key: "images", label: "Images in prompts", available: c.images },
    { key: "subagents", label: "Subagent activity", available: c.subagents },
    { key: "rewind", label: "Rewind", available: c.rewind },
    {
      key: "rustcode_extensions",
      label: "PRs, usage limits, preview",
      available: c.rustcode_extensions,
      detail: c.rustcode_extensions ? undefined : "RustCode harness only",
    },
  ];
}

/** Why a composer control is greyed out for this thread's harness, if it is. */
export function controlDisabledReason(
  control: "steer" | "effort" | "model" | "images" | "cancel",
  caps: HarnessCapabilities | undefined,
  harnessName = "This harness",
): string | undefined {
  if (!caps) return undefined;
  switch (control) {
    case "steer":
      return caps.steer === "live" ? undefined : `${harnessName} cannot take a message mid-turn; it queues`;
    case "effort":
      return caps.effort ? undefined : `${harnessName} has no effort setting`;
    case "model":
      return caps.models === "none" ? `${harnessName} picks its own model` : undefined;
    case "images":
      return caps.images ? undefined : `${harnessName} does not take images`;
    case "cancel":
      return caps.cancel ? undefined : `${harnessName} cannot stop a turn`;
  }
}

export function selectOption(options: ConfigOption[], id: string): ConfigOption | undefined {
  return options.find((o) => o.id === id && o.type === "select");
}
