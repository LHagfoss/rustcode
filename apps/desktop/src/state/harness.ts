// Pure harness and onboarding logic: what can be chosen, what is greyed out
// and why. The UI reads capability flags only; it never branches on a harness id.

import type { ConfigOption, Harness, HarnessCapabilities, HarnessModel, HarnessSnapshot } from "../acp/types";

export type HarnessStatus =
  | { tone: "ok"; label: string }
  | { tone: "warn"; label: string }
  | { tone: "off"; label: string };

export function harnessStatus(h: Harness): HarnessStatus {
  if (!h.installed) return { tone: "off", label: "Not installed" };
  if (h.error) return { tone: "warn", label: "Error" };
  if (h.auth === "needs_login") return { tone: "warn", label: "Sign in needed" };
  if (h.auth === "ok") return { tone: "ok", label: h.version ? `Ready · ${h.version}` : "Ready" };
  return { tone: "ok", label: h.version ? `Installed · ${h.version}` : "Installed" };
}

/** Why a harness cannot run a thread right now, or undefined when it can. */
export function unavailableReason(h: Harness): string | undefined {
  if (!h.installed)
    return h.install_hint ? `Not installed. Install with: ${h.install_hint}` : "Not installed";
  if (!h.enabled) return "Turned off in Settings › Harnesses";
  if (h.auth === "needs_login") return "Sign in first";
  return undefined;
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
