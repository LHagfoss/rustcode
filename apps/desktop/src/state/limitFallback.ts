// Switching a delegate at a limit (issue 3392): with the setting on, a delegate
// that stops on a Claude or Codex usage limit goes on at once on the other
// provider's model of the same tier, instead of waiting for the reset. The
// tiers are a table the user keeps (`limit-fallback-map`, Settings › Harnesses);
// this module only reads, checks and edits that table and picks a row. The move
// itself is `switchOnLimit` in state/store.ts, which uses the same
// `_rustcode/session/harness` request as `transfer_session`.

import type { Json } from "../acp/jsonrpc";

/** What a limited thread ran on. `model` matches part of the thread's model id or name. */
export interface FallbackFrom {
  harness: string;
  model?: string;
}

/** What it goes on with. Empty fields are the target harness's own defaults. */
export interface FallbackTo {
  harness: string;
  model?: string;
  effort?: string;
  account?: string;
}

export interface FallbackRule {
  from: FallbackFrom;
  to: FallbackTo;
}

const text = (value: Json | undefined): string | undefined => {
  const trimmed = typeof value === "string" ? value.trim() : "";
  return trimmed || undefined;
};

const object = (value: Json | undefined): Record<string, Json> | undefined =>
  value && typeof value === "object" && !Array.isArray(value) ? value : undefined;

/** The saved table; a row without both harnesses is skipped, as a hand-edited file may hold one. */
export function parseRules(value: Json | undefined): FallbackRule[] {
  if (!Array.isArray(value)) return [];
  const rules: FallbackRule[] = [];
  for (const entry of value) {
    const row = object(entry);
    const from = object(row?.from);
    const to = object(row?.to);
    const fromHarness = text(from?.harness);
    const toHarness = text(to?.harness);
    if (!from || !to || !fromHarness || !toHarness) continue;
    rules.push(
      cleanRule({
        from: { harness: fromHarness, model: text(from.model) },
        to: { harness: toHarness, model: text(to.model), effort: text(to.effort), account: text(to.account) },
      }),
    );
  }
  return rules;
}

/** A row as it is stored: empty fields left out. */
export function cleanRule(rule: FallbackRule): FallbackRule {
  const keep = <T extends object>(side: T): T =>
    Object.fromEntries(
      Object.entries(side).flatMap(([key, value]) =>
        typeof value === "string" && value.trim() ? [[key, value.trim()]] : [],
      ),
    ) as T;
  return { from: keep(rule.from), to: keep(rule.to) };
}

export function rulesJson(rules: FallbackRule[]): Json {
  return rules.map(cleanRule) as unknown as Json;
}

const sameSide = (a: FallbackFrom, b: FallbackFrom) =>
  a.harness === b.harness && (a.model ?? "").toLowerCase() === (b.model ?? "").toLowerCase();

/** Why `rule` cannot be saved in place of `replacing`, or null. */
export function ruleError(
  rules: FallbackRule[],
  rule: FallbackRule,
  replacing?: FallbackFrom,
): string | null {
  const clean = cleanRule(rule);
  if (!clean.from.harness) return "Choose the harness that hits the limit.";
  if (!clean.to.harness) return "Choose the harness to continue on.";
  if (clean.from.harness === clean.to.harness) {
    return "A limit on one harness cannot be fixed by the same harness; switching between accounts stays manual.";
  }
  const taken = rules.some(
    (other) => sameSide(other.from, clean.from) && !(replacing && sameSide(other.from, replacing)),
  );
  return taken ? "There is already a row for that harness and model." : null;
}

/** The table with `rule` saved: in the old row's place when it replaces one, else at the end. */
export function saveRule(
  rules: FallbackRule[],
  rule: FallbackRule,
  replacing?: FallbackFrom,
): FallbackRule[] {
  const clean = cleanRule(rule);
  const at = replacing ? rules.findIndex((other) => sameSide(other.from, replacing)) : -1;
  if (at < 0) return [...rules, clean];
  return rules.map((other, i) => (i === at ? clean : other));
}

export function removeRule(rules: FallbackRule[], from: FallbackFrom): FallbackRule[] {
  return rules.filter((rule) => !sameSide(rule.from, from));
}

/** A row in a line: "claude-code · opus → codex · gpt-5.5 · high". */
export function ruleSummary(rule: FallbackRule): string {
  const side = (parts: (string | undefined)[]) => parts.filter(Boolean).join(" · ");
  return `${side([rule.from.harness, rule.from.model ?? "any model"])} → ${side([rule.to.harness, rule.to.model, rule.to.effort, rule.to.account])}`;
}

/**
 * The row a limited thread follows, or null. A row names the harness the thread
 * runs on; one with a model is for threads whose model id or name contains it
 * (case aside) and beats a row for the harness alone. A row whose target the
 * thread already left on a limit is skipped, so two harnesses at their limits
 * never hand a thread back and forth.
 */
export function pickFallback(
  rules: readonly FallbackRule[],
  ran: { harness: string | undefined; model?: string },
  left: ReadonlySet<string> = new Set(),
): FallbackRule | null {
  if (!ran.harness) return null;
  const model = (ran.model ?? "").toLowerCase();
  const usable = rules.filter(
    (rule) =>
      rule.from.harness === ran.harness && !left.has(rule.to.harness) && rule.to.harness !== ran.harness,
  );
  const exact = usable.find((rule) => rule.from.model && model.includes(rule.from.model.toLowerCase()));
  return exact ?? usable.find((rule) => !rule.from.model) ?? null;
}

/** What the thread is told when it goes on elsewhere after a limit. */
export function switchText(from: string, to: string): string {
  return `${from} hit its usage limit, so this conversation moved to ${to}. Continue where you left off.`;
}

/** The quiet row the thread shows: which model it moved to and why. */
export function movedNotice(from: string, to: string): string {
  return `Moved to ${to} because ${from} hit its usage limit.`;
}
