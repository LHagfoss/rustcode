// One Check per harness, as pure logic: what it needs to do next and where it
// ends. A Check always lands in a final state: Ready, Needs sign-in, Not
// installed, or an error with its text (docs/harnesses.md).

import type { Harness } from "../acp/types";
import { accountState } from "./harness";

export type CheckState = "ready" | "needs_login" | "not_installed" | "error";

export interface CheckResult {
  state: CheckState;
  /** The one line the card shows. */
  text: string;
}

/** The step a running Check is on. */
export type CheckStep = "detect" | "models";

export const CHECK_STEP_LABEL: Record<CheckStep, string> = {
  // Install, then each account's sign-in: one request to the engine.
  detect: "Checking…",
  models: "Listing models…",
};

/** What a card shows for a harness: a Check on its way, or the one it ended in. */
export type CheckEntry = { running: CheckStep } | { result: CheckResult };

/**
 * The result a Check can already give once it has looked at the install and
 * the sign-ins, or null when the harness must still be started to list its
 * models. A harness is not started when there is nothing to run or no login.
 */
export function settledEarly(h: Harness): CheckResult | null {
  if (!h.installed) {
    const program = h.command[0];
    return {
      state: "not_installed",
      text: h.known || !program ? "Not installed" : `Not installed: ${program} was not found on PATH`,
    };
  }
  const accounts = accountState(h);
  if (accounts === "needs_login") return { state: "needs_login", text: "Needs sign-in: sign in an account" };
  if (accounts === undefined && h.auth === "needs_login")
    return { state: "needs_login", text: "Needs sign-in" };
  return null;
}

/**
 * What a card says about `h` after a Check: the result it ended in, unless the
 * harness has changed since (uninstalled, signed out) and says otherwise.
 */
export function currentResult(h: Harness, entry: CheckEntry | undefined): CheckResult | null {
  if (!entry || "running" in entry) return null;
  const now = settledEarly(h);
  return now && now.state !== entry.result.state ? now : entry.result;
}

/** Listing the models worked: the harness started, spoke ACP and answered. */
export function readyResult(count: number, version?: string): CheckResult {
  const models = count === 1 ? "1 model" : `${count} models`;
  return {
    state: "ready",
    text: ["Ready", version, count > 0 ? models : "no model list"].filter(Boolean).join(" · "),
  };
}

/** Something failed on the way; the text is the error itself. */
export function failedResult(error: unknown): CheckResult {
  const text = (error instanceof Error ? error.message : String(error)).trim();
  return { state: "error", text: text || "The check failed without saying why." };
}

/** The status dot's tone for a result. */
export function resultTone(result: CheckResult): "ok" | "warn" | "off" {
  return result.state === "ready" ? "ok" : result.state === "not_installed" ? "off" : "warn";
}
