/**
 * Skills, themes and prompt commands that travel to a paired machine (docs/desktop.md ›
 * Settings on one machine or all). A machine that follows this computer's settings also gets the
 * config directory's `skills/`, `themes/` and `commands/`, each with its own switch (on unless
 * turned off), when its connection opens and when they change here. **Fetch from ‹machine›**
 * copies the other way once, without following.
 *
 * One entry travels per request (`_rustcode/config/files/{list,get,put,prune}` in
 * `rustcode/engine/src/acp/config_files.rs`): both sides are listed with a content hash, only
 * differing entries are read and written, and the copies taken from the source before that it no
 * longer has are pruned. The receiving engine remembers which entries came from where, so its own
 * skills are never removed.
 */

import { create } from "zustand";
import { arr, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { readStorage, writeStorage } from "../lib/storage";

export type FileKind = "skills" | "themes" | "commands";
export const FILE_KINDS: { kind: FileKind; label: string }[] = [
  { kind: "skills", label: "Skills" },
  { kind: "themes", label: "Themes" },
  { kind: "commands", label: "Prompt commands" },
];

const STORAGE = "rustcode.settingsFollow.filesOff";
const LIST_MS = 15_000;
const ENTRY_MS = 30_000;

function readOff(): Record<string, FileKind[]> {
  try {
    const raw = obj(JSON.parse(readStorage(STORAGE) ?? "{}"));
    const out: Record<string, FileKind[]> = {};
    for (const [id, kinds] of Object.entries(raw ?? {})) {
      out[id] = arr(kinds).filter((kind): kind is FileKind => FILE_KINDS.some((k) => k.kind === kind));
    }
    return out;
  } catch {
    return {};
  }
}

/** The folders each follower does not take, by machine id; every folder travels unless listed. */
export const useFilesFollowOff = create<{ off: Record<string, FileKind[]> }>(() => ({ off: readOff() }));

export function followedKinds(id: string): FileKind[] {
  const off = useFilesFollowOff.getState().off[id] ?? [];
  return FILE_KINDS.map((k) => k.kind).filter((kind) => !off.includes(kind));
}

export function setKindFollowed(id: string, kind: FileKind, on: boolean): void {
  const current = useFilesFollowOff.getState().off;
  const rest = (current[id] ?? []).filter((entry) => entry !== kind);
  const off = { ...current, [id]: on ? rest : [...rest, kind] };
  writeStorage(STORAGE, JSON.stringify(off));
  useFilesFollowOff.setState({ off });
}

export interface ListedEntry {
  kind: FileKind;
  name: string;
  hash: string;
  skipped?: string;
}

export interface FilesCopyReport {
  written: string[];
  removed: string[];
  skipped: string[];
}

type Request = (host: string, method: string, params: JsonObject, timeoutMs: number) => Promise<unknown>;

function listed(answer: unknown): ListedEntry[] {
  return arr(obj(answer)?.entries).flatMap((raw) => {
    const entry = obj(raw);
    const kind = str(entry?.kind) as FileKind | undefined;
    const name = str(entry?.name);
    if (!kind || !name || !FILE_KINDS.some((k) => k.kind === kind)) return [];
    const skipped = str(entry?.skipped);
    return [{ kind, name, hash: str(entry?.hash) ?? "", ...(skipped ? { skipped } : {}) }];
  });
}

/**
 * The entries of `from` that `to` lacks or holds with other content, the ones `to` already holds
 * as they are (still recorded as `source`'s, so a later prune removes them), and the names per kind
 * `to` keeps as `source`'s copies.
 */
export function planCopy(
  from: ListedEntry[],
  to: ListedEntry[],
  kinds: FileKind[],
): { copy: ListedEntry[]; adopt: ListedEntry[]; keep: Record<FileKind, string[]>; skipped: string[] } {
  const there = new Map(to.map((entry) => [`${entry.kind}/${entry.name}`, entry.hash]));
  const keep: Record<FileKind, string[]> = { skills: [], themes: [], commands: [] };
  const copy: ListedEntry[] = [];
  const adopt: ListedEntry[] = [];
  const skipped: string[] = [];
  for (const entry of from) {
    if (!kinds.includes(entry.kind)) continue;
    keep[entry.kind].push(entry.name);
    const label = `${entry.kind}/${entry.name}`;
    if (entry.skipped) skipped.push(`${label} (${entry.skipped})`);
    else if (there.get(label) !== entry.hash) copy.push(entry);
    else adopt.push(entry);
  }
  return { copy, adopt, keep, skipped };
}

function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Copy `kinds` from the machine `from` to the machine `to`, recording them on `to` as copies of
 * `source` (the id of the machine they came from). Answers `null` when either side's engine is too
 * old to list its files; any other failure to list is thrown.
 */
export async function copyFiles(
  request: Request,
  from: string,
  to: string,
  source: string,
  kinds: FileKind[],
): Promise<FilesCopyReport | null> {
  if (!kinds.length) return { written: [], removed: [], skipped: [] };
  let theirs: ListedEntry[];
  let ours: ListedEntry[];
  try {
    [theirs, ours] = await Promise.all([
      request(from, "_rustcode/config/files/list", { kinds }, LIST_MS).then(listed),
      request(to, "_rustcode/config/files/list", { kinds }, LIST_MS).then(listed),
    ]);
  } catch (error) {
    if (error instanceof RpcError && error.isMethodNotFound) return null;
    throw error;
  }
  const plan = planCopy(theirs, ours, kinds);
  const report: FilesCopyReport = { written: [], removed: [], skipped: plan.skipped };
  for (const entry of plan.copy) {
    const label = `${entry.kind}/${entry.name}`;
    try {
      const content = obj(
        await request(from, "_rustcode/config/files/get", { kind: entry.kind, name: entry.name }, ENTRY_MS),
      );
      const answer = obj(
        await request(to, "_rustcode/config/files/put", { source, entry: content ?? {} }, ENTRY_MS),
      );
      if (str(answer?.outcome) === "written") report.written.push(label);
    } catch (error) {
      report.skipped.push(`${label} (${reason(error)})`);
    }
  }
  for (const entry of plan.adopt) {
    try {
      const content = obj(
        await request(to, "_rustcode/config/files/get", { kind: entry.kind, name: entry.name }, ENTRY_MS),
      );
      await request(to, "_rustcode/config/files/put", { source, entry: content ?? {} }, ENTRY_MS);
    } catch (error) {
      report.skipped.push(`${entry.kind}/${entry.name} (${reason(error)})`);
    }
  }
  for (const kind of kinds) {
    try {
      const answer = obj(
        await request(to, "_rustcode/config/files/prune", { source, kind, keep: plan.keep[kind] }, LIST_MS),
      );
      for (const name of arr(answer?.removed))
        if (typeof name === "string") report.removed.push(`${kind}/${name}`);
    } catch (error) {
      report.skipped.push(`${kind} cleanup (${reason(error)})`);
    }
  }
  return report;
}

/** One line for a toast: what changed, or that nothing had to. */
export function describeCopy(report: FilesCopyReport | null, from: string, to: string): string {
  if (!report) return `${from} or ${to} runs a RustCode too old to copy skills, themes and commands.`;
  const parts: string[] = [];
  if (report.written.length) parts.push(`copied ${report.written.length} from ${from}`);
  if (report.removed.length) parts.push(`removed ${report.removed.length} that ${from} no longer has`);
  if (report.skipped.length) parts.push(`skipped ${report.skipped.join(", ")}`);
  if (!parts.length) return `${to} already has ${from}'s skills, themes and commands.`;
  const line = parts.join("; ");
  return `${line.charAt(0).toUpperCase()}${line.slice(1)}.`;
}
