import { harnessPlanKey } from "../acp/rustcode";
import type { Harness, SlashCommand } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import type { AgentChoice, MentionState } from "../state/compose";
import { type MentionScope, SCOPES } from "../state/fileRefs";
import { defaultEffort, orderHarnesses, unavailableReason } from "../state/harness";
import { usageOf } from "../state/harnessUsage";
import { childAccountOn, type SignedOutLogins } from "../state/logins";
import type { MentionEntry, MentionSection } from "../state/mentionDrawer";
import type { CachedPlan } from "../state/store";

/** How long a project's file list is used before `@` lists it again. */
export const FILES_FRESH_MS = 30_000;

/** A row of the composer's drawer: a command, or one of the `@` drawer's own rows. */
export type DrawerEntry = { kind: "command"; command: SlashCommand } | MentionEntry;

/**
 * A row of the drawer as it is drawn: a section's heading, a harness under it, or a bound row the
 * arrow keys can land on — one that points at something, or a section's `Show all`.
 */
export type DrawerRowItem =
  | { kind: "heading"; label: string }
  | { kind: "harness"; harness: string; name: string; account?: string; plan?: string }
  | { kind: "row"; entry: DrawerEntry; index: number }
  | { kind: "more"; section: MentionSection; hidden: number; index: number }
  | { kind: "less"; section: MentionSection; index: number };

/** What a bound row does when it is picked: point at something, or open/close a section. */
export type DrawerPick =
  | { kind: "entry"; entry: DrawerEntry }
  | { kind: "more"; section: MentionSection }
  | { kind: "less"; section: MentionSection };

/** Every match, before the drawer keeps some back behind `Show N more`: what the search looks through. */
export const ALL_MATCHES = 200;

/** Nothing expanded: the drawer's first look at a folder shows a few rows of each section. */
export const NOT_EXPANDED: ReadonlySet<MentionSection> = new Set();

/** No plans to draw: a machine's harness rows, whose plans this computer has not read. */
export const NO_PLANS: Record<string, CachedPlan> = {};

/** What one login's plan has left across its windows: `62% left`, or nothing when it was not read. */
export function planLeft(plan: CachedPlan | undefined): string | undefined {
  const windows = usageOf(plan)?.windows;
  if (!windows?.length) return undefined;
  return `${Math.max(0, Math.round(100 - Math.max(...windows.map((w) => w.used_percent))))}% left`;
}

/**
 * Who a delegate could run on: every available harness's models, with the model a new thread there
 * starts on first, and the effort, login and plan that go with each. The rows are interleaved one
 * model per harness, so the drawer's first rows show what every harness offers rather than one
 * harness's whole list, and the whole catalog is searched by name (`rankAgents`), not just what is
 * on screen.
 */
export function agentChoices(
  harnesses: readonly Harness[],
  plans: Record<string, CachedPlan>,
  signedOut: SignedOutLogins,
  /** The thread the delegate would be started from: a child on its harness inherits its login. */
  parent: { harness?: string; account?: string | null },
): AgentChoice[] {
  const perHarness = orderHarnesses([...harnesses])
    .filter((h) => !unavailableReason(h))
    .map((h) => {
      const accounts = h.accounts ?? [];
      const accountId = accounts.length ? childAccountOn(h, parent, signedOut) : null;
      // Only a harness with logins to choose between names one on its rows.
      const account = accounts.length > 1 ? accounts.find((a) => a.id === accountId)?.name : undefined;
      const plan = planLeft(plans[harnessPlanKey(h.id, accountId)]);
      const lead = h.last?.model;
      const models = [...h.models].sort((a, b) => Number(b.id === lead) - Number(a.id === lead));
      if (!models.length) {
        return [
          {
            harness: h.id,
            harnessName: h.name,
            name: h.name,
            effort: h.capabilities.effort ? (h.last?.effort ?? undefined) : undefined,
            account,
            plan,
            token: h.id,
          },
        ];
      }
      return models.map((m) => ({
        harness: h.id,
        harnessName: h.name,
        model: m.id,
        name: m.name,
        effort: h.capabilities.effort
          ? m.id === lead && h.last?.effort
            ? h.last.effort
            : (defaultEffort(m, m.efforts) ?? undefined)
          : undefined,
        account,
        plan,
        token: `${h.id}/${m.id}`,
      }));
    });
  const rows: AgentChoice[] = [];
  const deepest = Math.max(0, ...perHarness.map((choices) => choices.length));
  for (let i = 0; i < deepest; i++) {
    for (const choices of perHarness) {
      const row = choices[i];
      if (row) rows.push(row);
    }
  }
  return rows;
}

/** The group keywords the drawer offers first, while what is typed is empty or still a prefix. */
export function keywordEntries(query: string, machines: boolean): MentionScope[] {
  const asked = query.trim().toLowerCase();
  return SCOPES.filter((scope) => scope.word.startsWith(asked))
    .map((scope) => scope.scope)
    .filter((scope) => machines || scope !== "machines");
}

/** The folder a thread is grouped by: a worktree's repo, else the thread's own folder. */
export const projectKeyOf = (session: { cwd?: string; repo?: string; worktree?: { repo?: string } }) =>
  session.worktree?.repo || session.repo || session.cwd || "";

/**
 * What a thread is doing, as the `#` list orders it and says it: the app's live turn first, then what
 * the engine listed (a thread working in another window is working here too), and settled only when
 * the engine says so.
 */
export function mentionStateOf(
  session: { status?: ThreadStatus },
  working: boolean,
  asking: boolean,
): MentionState {
  // The app's own reading of a thread (state/store.ts `statusOf`): what it asks of you, then that it
  // works, then what the engine last listed.
  if (asking) return "needs";
  if (working) return "working";
  if (session.status === "needs_input") return "needs";
  if (session.status === "running") return "working";
  return session.status === "done" ? "settled" : "open";
}
