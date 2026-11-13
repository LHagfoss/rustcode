// Delegate presets (issue 3391): named sets of `start_session` / `fan_out`
// arguments, kept as the `delegate-presets` server setting. The engine
// expands a call's `preset` (rustcode/engine/src/orchestration/presets.rs);
// this module only reads, checks and edits the list Settings shows.

import type { Json } from "../acp/jsonrpc";
import { modeLabel, type PermissionMode } from "./permissions";

export interface DelegatePreset {
  name: string;
  harness?: string;
  model?: string;
  effort?: string;
  account?: string;
  /** Only ever lowers what the delegate gets (`state/host.ts` › narrowed). */
  access?: PermissionMode;
  worktree?: boolean;
  instructions?: string;
}

const TEXT_FIELDS = ["harness", "model", "effort", "account", "instructions"] as const;

/** The saved presets; an entry without a name, or not an object, is skipped as the engine skips it. */
export function parsePresets(value: Json | undefined): DelegatePreset[] {
  if (!Array.isArray(value)) return [];
  const presets: DelegatePreset[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const name = typeof entry.name === "string" ? entry.name.trim() : "";
    if (!name) continue;
    const preset: DelegatePreset = { name };
    for (const key of TEXT_FIELDS) {
      const text = entry[key];
      if (typeof text === "string" && text.trim()) preset[key] = key === "instructions" ? text : text.trim();
    }
    if (entry.access === "ask" || entry.access === "edits" || entry.access === "full")
      preset.access = entry.access;
    if (typeof entry.worktree === "boolean") preset.worktree = entry.worktree;
    presets.push(preset);
  }
  return presets;
}

/** A preset as it is stored: empty fields left out. */
export function cleanPreset(preset: DelegatePreset): DelegatePreset {
  const clean: DelegatePreset = { name: preset.name.trim() };
  for (const key of TEXT_FIELDS) {
    const text = preset[key]?.trim();
    if (text) clean[key] = key === "instructions" ? (preset[key] ?? "").trim() : text;
  }
  if (preset.access) clean.access = preset.access;
  if (preset.worktree !== undefined) clean.worktree = preset.worktree;
  return clean;
}

/** Why `preset` cannot be saved in place of `replacing` (its old name), or null. */
export function presetError(
  presets: DelegatePreset[],
  preset: DelegatePreset,
  replacing?: string,
): string | null {
  const name = preset.name.trim();
  if (!name) return "A preset needs a name.";
  if (name.length > 200) return "The name is too long.";
  const taken = presets.some(
    (other) =>
      other.name.toLowerCase() === name.toLowerCase() &&
      other.name.toLowerCase() !== replacing?.toLowerCase(),
  );
  return taken ? `There is already a preset named ${name}.` : null;
}

/** The list with `preset` saved: in its old place when it replaces one, else at the end. */
export function savePreset(
  presets: DelegatePreset[],
  preset: DelegatePreset,
  replacing?: string,
): DelegatePreset[] {
  const clean = cleanPreset(preset);
  const at = replacing ? presets.findIndex((other) => other.name === replacing) : -1;
  if (at < 0) return [...presets, clean];
  return presets.map((other, i) => (i === at ? clean : other));
}

export function removePreset(presets: DelegatePreset[], name: string): DelegatePreset[] {
  return presets.filter((preset) => preset.name !== name);
}

/** What a preset sets, in a line: "codex · gpt-5 · high · work · worktree". */
export function presetSummary(preset: DelegatePreset): string {
  const parts = [preset.harness, preset.model, preset.effort, preset.account].filter(Boolean) as string[];
  if (preset.access) parts.push(`at most ${modeLabel(preset.access)}`);
  if (preset.worktree !== undefined) parts.push(preset.worktree ? "worktree" : "no worktree");
  if (preset.instructions) parts.push("instructions");
  return parts.length ? parts.join(" · ") : "Nothing set";
}

export function presetsJson(presets: DelegatePreset[]): Json {
  return presets.map((preset) => ({ ...cleanPreset(preset) })) as unknown as Json;
}
