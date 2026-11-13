// The `@` drawer's shape: the agents under their own harness, the threads and the
// project's files under their own headings, a few of each up front and the rest
// behind a `Show all` row. What is typed searches every item, never only the few
// that are on screen, and the rows still say which section and harness they are in.

import { fuzzyScore } from "../lib/modelSearch";
import type { AgentChoice, ThreadChoice } from "./compose";
import type { MentionScope } from "./fileRefs";

/** A row of the drawer that points at something. */
export type MentionEntry =
  | { kind: "keyword"; scope: MentionScope }
  | { kind: "agent"; agent: AgentChoice }
  | { kind: "thread"; thread: ThreadChoice }
  | { kind: "file"; path: string };

/** A group of the drawer that keeps something back behind its `Show all` row. */
export type MentionSection = "agents" | "threads" | "files";

/** How the drawer draws a section: its name, a harness's name, a row, or its rest and its end. */
export type MentionNode =
  | { kind: "heading"; label: string }
  /** A harness's name, with the login and plan its rows run on: said once here, not on every row. */
  | { kind: "harness"; harness: string; name: string; account?: string; plan?: string }
  | { kind: "entry"; entry: MentionEntry }
  /** The rows a section kept back: `Show N more` draws them. */
  | { kind: "more"; section: MentionSection; hidden: number }
  /** The section is open whole: `Show fewer` puts the short list back. */
  | { kind: "less"; section: MentionSection };

/**
 * What a section shows while nothing is typed: enough rows to recognise what is
 * there, not the whole project. The rest waits behind `Show all (N)`.
 */
export const SHORT_ROWS = 5;

/** What it shows while something is typed: the best matches, still not every one of them. */
export const QUERY_ROWS = 8;

/** The section each heading names, in the order the drawer lists them. */
const LABEL: Record<MentionSection, string> = {
  agents: "Agents",
  threads: "Threads",
  files: "Files",
};

export interface MentionInput {
  /** What is typed after the `@` (or the `#`): anything at all searches every item. */
  query: string;
  /** The group a keyword narrowed the drawer to (`@agents pi`), else every group. */
  scope?: MentionScope | null;
  /** The `@agents`, `@threads` and `@files` rows, already narrowed to what is typed. */
  keywords?: readonly MentionScope[];
  /** The agents of every harness that can run, ranked; the harness's own last model first. */
  agents: readonly AgentChoice[];
  /** The threads this one started and still runs, on the harnesses they run on, ranked. */
  delegates: readonly ThreadChoice[];
  /** The threads the message can point at, ranked. */
  threads: readonly ThreadChoice[];
  /** The project's files, ranked. */
  files: readonly string[];
  /** The sections the reader asked to see whole. */
  expanded?: ReadonlySet<MentionSection>;
  /** What a harness is called, for a delegate whose harness lists no agent row here. */
  harnessNames?: Readonly<Record<string, string>>;
}

/** The harness a row is grouped under: a thread that names none runs on the app's own engine. */
function harnessOf(entry: MentionEntry): string {
  if (entry.kind === "agent") return entry.agent.harness;
  if (entry.kind === "thread") return entry.thread.harness || "rustcode";
  return "";
}

/** The login and plan a harness's rows share: the first row that names them speaks for the group. */
function groupDetail(entries: readonly MentionEntry[]): { account?: string; plan?: string } {
  const agents = entries.flatMap((entry) => (entry.kind === "agent" ? [entry.agent] : []));
  return {
    account: agents.find((agent) => agent.account)?.account,
    plan: agents.find((agent) => agent.plan)?.plan,
  };
}

/** How well a section's best row answers what is typed: the sections are listed by it. */
function sectionScore(query: string, nodes: readonly MentionNode[]): number {
  let best = Number.NEGATIVE_INFINITY;
  for (const node of nodes) {
    if (node.kind !== "entry") continue;
    const entry = node.entry;
    const label =
      entry.kind === "agent"
        ? entry.agent.name
        : entry.kind === "thread"
          ? entry.thread.title
          : entry.kind === "file"
            ? entry.path.slice(entry.path.lastIndexOf("/") + 1)
            : "";
    const score = label ? fuzzyScore(query, label) : null;
    if (score !== null && score > best) best = score;
  }
  return best;
}

/** What a row calls itself, so a harness's heading can be left out when the row already says it. */
function nameOfEntry(entry: MentionEntry): string {
  if (entry.kind === "agent") return entry.agent.name;
  if (entry.kind === "thread") return entry.thread.title;
  return "";
}

/** `entries` bucketed by harness, each harness where its first row put it, rows in the order given. */
function byHarness(entries: readonly MentionEntry[]): { harness: string; entries: MentionEntry[] }[] {
  const order: string[] = [];
  const buckets = new Map<string, MentionEntry[]>();
  for (const entry of entries) {
    const harness = harnessOf(entry);
    const held = buckets.get(harness);
    if (held) held.push(entry);
    else {
      buckets.set(harness, [entry]);
      order.push(harness);
    }
  }
  return order.map((harness) => ({ harness, entries: buckets.get(harness)! }));
}

/**
 * The drawer's rows, in the order it draws them: the keywords first when it is
 * not narrowed, then each section whose rows it has, with its heading when it
 * lists more than one section and a `Show N more` row for what it keeps back —
 * or, once that is open, a `Show fewer` row that puts the short list back. A
 * section with nothing in it is left out, heading and all.
 */
export function mentionNodes(input: MentionInput): MentionNode[] {
  const { query, scope, expanded, harnessNames } = input;
  const asked = query.trim().length > 0;
  const cap = (section: MentionSection) =>
    expanded?.has(section) ? Number.POSITIVE_INFINITY : asked ? QUERY_ROWS : SHORT_ROWS;
  const groups: { section: MentionSection; nodes: MentionNode[] }[] = [];
  // What each harness is called: the name its own rows carry, else the app's catalog, else its id.
  const called = new Map(input.agents.map((agent) => [agent.harness, agent.harnessName]));
  const nameOf = (harness: string) => called.get(harness) ?? harnessNames?.[harness] ?? harness;

  // The keywords are where to look, not something to point at: no heading, and nothing kept back.
  const keywords: MentionNode[] = scope
    ? []
    : (input.keywords ?? []).map(
        (entry): MentionNode => ({
          kind: "entry",
          entry: { kind: "keyword", scope: entry },
        }),
      );

  if (!scope || scope === "agents") {
    // A harness's models first, then the threads this one already runs on that harness: the
    // delegate under the harness it works for reads as one group.
    const entries: MentionEntry[] = [
      ...input.agents.map((agent): MentionEntry => ({ kind: "agent", agent })),
      ...input.delegates.map((thread): MentionEntry => ({ kind: "thread", thread })),
    ];
    const harnesses = byHarness(entries);
    const limit = cap("agents");
    const nodes: MentionNode[] = [];
    let kept = 0;
    let hidden = 0;
    for (const group of harnesses) {
      const shown = group.entries.slice(0, limit);
      if (!shown.length) continue;
      // A harness's name is worth a line of its own when it says more than its rows do: more than
      // one row under it, or a row that is not simply the harness by name (`Local Agent` over one
      // `Local Agent` row reads as one row, not a group).
      const name = nameOf(group.harness);
      if (shown.length > 1 || nameOfEntry(shown[0]!) !== name) {
        nodes.push({ kind: "harness", harness: group.harness, name, ...groupDetail(group.entries) });
      }
      for (const entry of shown) nodes.push({ kind: "entry", entry });
      kept += shown.length;
      hidden += group.entries.length - shown.length;
    }
    if (kept) {
      if (hidden > 0) nodes.push({ kind: "more", section: "agents", hidden });
      else if (expanded?.has("agents")) nodes.push({ kind: "less", section: "agents" });
      groups.push({ section: "agents", nodes });
    }
  }

  if (!scope || scope === "threads") {
    const entries = input.threads.map((thread): MentionEntry => ({ kind: "thread", thread }));
    const shown = entries.slice(0, cap("threads"));
    if (shown.length) {
      const nodes: MentionNode[] = shown.map((entry) => ({ kind: "entry", entry }));
      const hidden = entries.length - shown.length;
      if (hidden > 0) nodes.push({ kind: "more", section: "threads", hidden });
      else if (expanded?.has("threads")) nodes.push({ kind: "less", section: "threads" });
      groups.push({ section: "threads", nodes });
    }
  }

  if (!scope || scope === "files") {
    const entries = input.files.map((path): MentionEntry => ({ kind: "file", path }));
    const shown = entries.slice(0, cap("files"));
    if (shown.length) {
      const nodes: MentionNode[] = shown.map((entry) => ({ kind: "entry", entry }));
      const hidden = entries.length - shown.length;
      if (hidden > 0) nodes.push({ kind: "more", section: "files", hidden });
      else if (expanded?.has("files")) nodes.push({ kind: "less", section: "files" });
      groups.push({ section: "files", nodes });
    }
  }

  // Something typed lists the section with the closest match first, so the highlighted first row is
  // the best answer whichever group it is in; nothing typed keeps the drawer's own order.
  if (asked && !scope) {
    const scores = new Map(groups.map((group) => [group, sectionScore(query, group.nodes)]));
    // `|| 0`: two sections with nothing scored are -Infinity apart, which is NaN, not a tie.
    groups.sort((a, b) => scores.get(b)! - scores.get(a)! || 0);
  }
  // A heading only separates groups the drawer draws beside each other; one group needs none.
  const labelled = groups.length > 1;
  const nodes: MentionNode[] = asked ? [] : [...keywords];
  for (const group of groups) {
    if (labelled) nodes.push({ kind: "heading", label: LABEL[group.section] });
    nodes.push(...group.nodes);
  }
  // A keyword that is still being typed (`@fi`) follows the matches rather than leading them.
  if (asked) nodes.push(...keywords);
  return nodes;
}
