// Where an orchestrator thread's delegates stand, read off data the app already holds: the live
// panel above the composer and the reply to a bare "status" message both draw it (issue 3651).
// Pure, so the panel, the local reply and their tests share one answer. The pull request and its
// checks come from the same read `thread_status` makes (`state/delegatePulls.ts`); everything else
// is the link book, the thread list and the transcripts, so nothing here asks the engine.

import type { DelegatePull } from "../acp/rustcode";
import type { StackAgent } from "./agentStack";
import { AGENT_WORD, type AgentState } from "./delegates";
import { formatDuration } from "./thread";

/** The pull request a row shows: the fields of `thread_status`'s `pull`, or why there is none. */
export interface StatusPull {
  /** `null` when the branch has no pull request yet. */
  pull: DelegatePull | null;
  /** Why it could not be read (a remote with no provider, `gh` signed out). */
  note?: string;
}

/** One delegate as the panel and the reply say it. */
export interface DelegateStatusRow {
  /** The delegate's thread. */
  id: string;
  title: string;
  state: AgentState;
  /** The last tool it called, when its transcript is loaded. */
  lastTool?: string;
  /** How long it has shown nothing, while it works. */
  idleMs?: number;
  harness?: string;
  account?: string;
  model?: string;
  branch?: string;
  /** Absent until the pull request has been read. */
  pull?: StatusPull;
}

/** What the rows are built from, per delegate. */
export interface StatusInput {
  id: string;
  title: string;
  state: AgentState;
  /** The last activity (`activityOf`) of its transcript, when the app holds one. */
  lastAt?: number;
  lastTool?: string;
  harness?: string;
  account?: string;
  model?: string;
  branch?: string;
  pull?: StatusPull;
}

/**
 * A bare `status` or `status?` (any case, with room around it) is a question the app answers
 * itself. Anything longer is a message for the model: "status of the PR?" is not this.
 */
export function isStatusQuery(text: string): boolean {
  return /^\s*status\s*\?*\s*$/i.test(text);
}

/** The rows, working ones first (what the reader asks about), then the rest in the order given. */
export function statusRows(inputs: readonly StatusInput[], now: number): DelegateStatusRow[] {
  const rank = (state: AgentState) => (state === "needs" ? 0 : state === "working" ? 1 : 2);
  return inputs
    .map((input, index) => ({ input, index }))
    .sort((a, b) => rank(a.input.state) - rank(b.input.state) || a.index - b.index)
    .map(({ input }) => {
      const working = input.state === "working";
      return {
        id: input.id,
        title: input.title,
        state: input.state,
        ...(input.lastTool ? { lastTool: input.lastTool } : {}),
        ...(working && input.lastAt !== undefined ? { idleMs: Math.max(0, now - input.lastAt) } : {}),
        ...(input.harness ? { harness: input.harness } : {}),
        ...(input.account ? { account: input.account } : {}),
        ...(input.model ? { model: input.model } : {}),
        ...(input.branch ? { branch: input.branch } : {}),
        ...(input.pull ? { pull: input.pull } : {}),
      };
    });
}

/** The pull request number with its checks or outcome (`checks passing`, `2 failing`, `merged`), `no pull request`, or `—` while unread. */
export function pullText(pull: StatusPull | undefined): string {
  if (!pull) return "—";
  if (!pull.pull) return pull.note ?? "no pull request";
  const p = pull.pull;
  const head = `#${p.number}`;
  if (p.merged) return `${head} · merged`;
  if (p.state === "closed") return `${head} · closed`;
  const draft = p.draft ? " · draft" : "";
  switch (p.ci) {
    case "none":
      return `${head}${draft} · no checks`;
    case "passing":
      return `${head}${draft} · checks passing`;
    case "pending":
      return `${head}${draft} · ${p.pending} pending`;
    case "failing":
      return `${head}${draft} · ${p.failing.length || 1} failing`;
  }
}

/** How long a working delegate has shown nothing: `idle 2m`, or `active` inside the first minute. */
export function idleText(idleMs: number | undefined): string {
  if (idleMs === undefined) return "";
  return idleMs < 60_000 ? "active" : `idle ${formatDuration(idleMs)}`;
}

/** The model and the account it runs on, `opus · work`; either alone, or empty. */
export function runsOn(row: Pick<DelegateStatusRow, "harness" | "model" | "account">): string {
  const what = row.model ?? row.harness;
  return [what, row.account].filter(Boolean).join(" · ");
}

/** `2 working · 1 needs you · 3 done`: the panel's header, and the reply's first line. */
export function statusSummary(rows: readonly DelegateStatusRow[]): string {
  const count = (state: AgentState) => rows.filter((row) => row.state === state).length;
  const needs = count("needs");
  const parts = [
    count("working") ? `${count("working")} working` : "",
    needs ? `${needs} ${needs === 1 ? "needs" : "need"} you` : "",
    count("failed") ? `${count("failed")} failed` : "",
    count("done") ? `${count("done")} done` : "",
    count("stopped") ? `${count("stopped")} stopped` : "",
  ].filter(Boolean);
  return parts.join(" · ") || "no delegates";
}

/** The reply to `status`, as Markdown: one line a delegate, in the same words as the panel. */
export function statusReply(rows: readonly DelegateStatusRow[]): string {
  if (!rows.length) return "No delegates yet.";
  const lines = rows.map((row) => {
    const bits = [
      AGENT_WORD[row.state],
      row.lastTool ? `last tool ${row.lastTool}` : "",
      idleText(row.idleMs),
      runsOn(row),
      row.branch ? `\`${row.branch}\`` : "",
      row.pull ? pullText(row.pull) : "",
    ].filter(Boolean);
    return `- **${row.title}** — ${bits.join(" · ")}`;
  });
  return `${statusSummary(rows)}\n\n${lines.join("\n")}`;
}

/** Whether a row is work the reader can still stop: working or waiting on them. */
export function canStop(row: Pick<DelegateStatusRow, "state">): boolean {
  return row.state === "working" || row.state === "needs";
}

/**
 * The collapsed panel's one line: `3 delegates · 2 working · 1 needs you`. The count comes first, so
 * the line reads the same whatever the states; it is `statusSummary` after it.
 */
export function panelLabel(rows: readonly DelegateStatusRow[]): string {
  const count = `${rows.length} ${rows.length === 1 ? "delegate" : "delegates"}`;
  return rows.length ? `${count} · ${statusSummary(rows)}` : count;
}

/** What needs the reader first, so the collapsed line can say it in its own tint. */
export function panelAttention(rows: readonly DelegateStatusRow[]): "needs" | "failed" | undefined {
  if (rows.some((row) => row.state === "needs")) return "needs";
  if (rows.some((row) => row.state === "failed")) return "failed";
  return undefined;
}

/** The harness marks the collapsed panel draws: the delegates working or waiting, one icon each. */
export function panelMarks(rows: readonly DelegateStatusRow[]): StackAgent[] {
  return rows.filter(canStop).map((row) => ({
    id: row.id,
    state: row.state === "needs" ? "needs" : "working",
    kind: "thread" as const,
    harness: row.harness,
    label: row.title,
  }));
}
