// Usage across this computer and the machines it is paired with. A thread run
// on a server lands in that server's data store and reads that server's
// sign-ins, so this computer alone never sees it (issue: usage never syncs to
// the Mac that drives a VPS). Statistics reads every connected machine's
// `_rustcode/data/usage` and adds the reports up here; Settings › Usage reads
// each machine's `_rustcode/usage/shared` (numbers only, no sign-in leaves it).

import type { Json } from "../acp/jsonrpc";
import {
  type PlanFailure,
  type PlanStatus,
  parsePlans,
  planKey,
  planOf,
  type UsagePlan,
  type UsageReport,
  type UsageRow,
  type UsageWindow,
} from "../acp/rustcode";
import { usageRows } from "./logins";
import { addTotals, emptyTotals, totalsOf } from "./statistics";
import { relativeAge } from "./thread";
import { LOCAL_HOST, threadKey } from "./threadKeys";

/** Which store Statistics reads: every connected machine, or one (`""` is this computer). */
export type MachineScope = "all" | string;

/** The hosts a scope reads: this computer first, then each connected machine. */
export function scopeHosts(scope: MachineScope, connected: readonly string[]): string[] {
  if (scope === "all") return [LOCAL_HOST, ...connected];
  return scope === LOCAL_HOST || connected.includes(scope) ? [scope] : [LOCAL_HOST];
}

/** A row named as it is on this computer: a thread on a machine by its key there. */
function onHost(row: UsageRow, host: string): UsageRow {
  if (!host) return row;
  return {
    ...row,
    ...(row.thread ? { thread: threadKey(host, row.thread) } : {}),
    ...(row.parentId ? { parentId: threadKey(host, row.parentId) } : {}),
  };
}

/** What makes two rows of one grouping the same row. */
function rowKey(row: UsageRow, groupBy: string): string {
  switch (groupBy) {
    case "thread":
      return `t\n${row.thread ?? ""}`;
    case "day":
      return `d\n${row.day ?? ""}`;
    case "project":
      return `p\n${row.project ?? ""}`;
    case "model":
      return `m\n${row.harness}\n${row.model ?? ""}`;
    default:
      return `h\n${row.harness}`;
  }
}

/**
 * The reports of several machines as one: rows of the same key (a model, a
 * day) added, a thread kept apart by the machine it ran on. Counts a machine
 * never reported stay absent unless another one reported them, as in one store.
 */
export function mergeUsageReports(parts: readonly { host: string; report: UsageReport }[]): UsageReport {
  const [only] = parts;
  if (parts.length === 1 && only && !only.host) return only.report;
  const groupBy = parts.find((part) => part.report.groupBy)?.report.groupBy ?? "";
  const rows = new Map<string, UsageRow>();
  let totals = emptyTotals();
  for (const { host, report } of parts) {
    totals = addTotals(totals, report.totals);
    for (const raw of report.rows) {
      const row = onHost(raw, host);
      const key = rowKey(row, groupBy);
      const known = rows.get(key);
      if (!known) {
        rows.set(key, row);
        continue;
      }
      // Two machines' rows of one model or day: the same pair only while both agree.
      const modelHarness = known.modelHarness === row.modelHarness ? known.modelHarness : undefined;
      const longest = Math.max(known.longestTurnMs ?? -1, row.longestTurnMs ?? -1);
      rows.set(key, {
        ...known,
        ...addTotals(totalsOf(known), totalsOf(row)),
        modelHarness,
        longestTurnMs: longest < 0 ? undefined : longest,
      });
    }
  }
  return {
    groupBy,
    rows: [...rows.values()],
    totals,
    truncated: parts.some((part) => part.report.truncated),
    parentsReported: parts.some((part) => part.report.parentsReported),
  };
}

/** One machine's plans as Settings › Usage draws them: only the rows it read. */
export function sharedPlans(result: Json | undefined): UsagePlan[] {
  return parsePlans(result).filter((plan) => plan.status !== "notAsked");
}

/** One machine's reading of a subscription, kept on a merged card for provenance. */
export interface PlanSource {
  /** Machine host id (`""` for this computer). */
  host: string;
  /** Display name of the machine ("This computer", "Lagos VPS", ...). */
  machine: string;
  status: PlanStatus;
  /** When this machine's plans were fetched (ms). */
  readAt?: number;
  /** Cached `asOf` timestamp (ms) when the machine returned recorded numbers. */
  asOf?: number;
  error?: string;
  reason?: PlanFailure;
}

/** One subscription ready to draw in Settings › Usage, possibly merged across machines. */
export interface MergedUsagePlan {
  /** Stable React key across machines. */
  key: string;
  /** Winning reading whose windows or status are drawn. */
  plan: UsagePlan;
  /** Host id of the winning reading (`""` for this computer). */
  winnerHost: string;
  /** Display name of the winning machine. */
  winnerMachine: string;
  /** Every machine that reported this subscription, in host order. */
  sources: PlanSource[];
}

/** One machine's plan payload entering `mergeUsagePlans`. */
export interface HostPlans {
  /** Machine host id (`""` for this computer). */
  host: string;
  /** Display name ("This computer", "Lagos VPS", ...). */
  machine: string;
  plans: UsagePlan[];
  readAt?: number;
}

interface Candidate {
  host: string;
  machine: string;
  plan: UsagePlan;
  readAt?: number;
}

/** Tolerance when matching two live windows' `resetsAt` epochs across machines. */
const RESET_TOLERANCE_MS = 120_000;
/** Two live reads closer than this break ties by higher `usedPercent` instead of poll clock. */
const FRESHNESS_TIE_MS = 15_000;

const norm = (value: string | null | undefined): string => (value ?? "").trim().toLowerCase();

const hasWindows = (plan: UsagePlan): boolean => plan.status === "ok" && plan.windows.length > 0;

/**
 * The window of `windows` that is `window` on another machine: the same label,
 * else (labels absent or renamed) the same length. A plan with several windows
 * of one length (Antigravity's quota groups, each weekly) pairs them by label
 * only, never one group's weekly window with another's.
 */
function counterpart(
  window: UsageWindow & { label: string },
  windows: readonly (UsageWindow & { label: string })[],
) {
  const byLabel = windows.find((candidate) => candidate.label === window.label);
  if (byLabel || window.minutes === undefined) return byLabel;
  const sameLength = windows.filter((candidate) => candidate.minutes === window.minutes);
  return sameLength.length === 1 ? sameLength[0] : undefined;
}

/**
 * Compares active (`usedPercent > 0`) shared windows' `resetsAt` timestamps.
 * Idle 0%-used windows can roll forward with poll time, and a window whose
 * reset has passed is one machine's older read, so only active, current windows
 * establish or refute identity.
 */
function compareActiveResets(a: UsagePlan, b: UsagePlan, now: number): "match" | "conflict" | "none" {
  if (!hasWindows(a) || !hasWindows(b)) return "none";
  let matched = 0;
  for (const wa of a.windows) {
    if (wa.resetsAt === undefined || wa.usedPercent <= 0 || wa.resetsAt < now - RESET_TOLERANCE_MS) continue;
    const wb = counterpart(wa, b.windows);
    if (!wb || wb.resetsAt === undefined || wb.usedPercent <= 0 || wb.resetsAt < now - RESET_TOLERANCE_MS)
      continue;
    if (Math.abs(wa.resetsAt - wb.resetsAt) > RESET_TOLERANCE_MS) return "conflict";
    matched += 1;
  }
  return matched > 0 ? "match" : "none";
}

function plansContradict(a: UsagePlan, b: UsagePlan, now: number): boolean {
  if (a.fingerprint && b.fingerprint && a.fingerprint !== b.fingerprint) return true;
  if (hasWindows(a) && hasWindows(b)) {
    if (a.plan && b.plan && norm(a.plan) !== norm(b.plan)) return true;
    if (compareActiveResets(a, b, now) === "conflict") return true;
  }
  return false;
}

function sameWindowSignature(a: UsagePlan, b: UsagePlan, now: number): boolean {
  if (!hasWindows(a) || !hasWindows(b)) return false;
  if (a.plan && b.plan && norm(a.plan) !== norm(b.plan)) return false;
  return compareActiveResets(a, b, now) === "match";
}

function sameAccountLabel(a: UsagePlan, b: UsagePlan): boolean {
  if (a.account && b.account) {
    return norm(a.account) === norm(b.account) || norm(a.name) === norm(b.name);
  }
  return !a.account && !b.account;
}

function planRank(plan: UsagePlan): number {
  if (plan.status === "ok") {
    if (plan.windows.length === 0) return 2;
    return plan.asOf === undefined ? 4 : 3;
  }
  return plan.status === "unavailable" ? 1 : 0;
}

function maxUsedPercent(plan: UsagePlan): number {
  return plan.windows.reduce((max, w) => Math.max(max, w.usedPercent), -1);
}

/** An active window whose reset has passed: numbers from before it rolled over. */
function outlived(plan: UsagePlan, now: number): boolean {
  return plan.windows.some(
    (w) => w.usedPercent > 0 && w.resetsAt !== undefined && w.resetsAt < now - RESET_TOLERANCE_MS,
  );
}

/**
 * Which of two readings of one subscription the windows themselves say is
 * newer (> 0 for `a`): a later reset of an active window is a later window,
 * and within one window usage only grows. When this computer last received an
 * answer says nothing about when the machine read it: a paired machine answers
 * from its own cache, so a fresh answer can carry old numbers.
 */
function windowProgress(a: UsagePlan, b: UsagePlan, now: number): number {
  const aOld = outlived(a, now);
  const bOld = outlived(b, now);
  if (aOld !== bOld) return aOld ? -1 : 1;
  let votes = 0;
  for (const wa of a.windows) {
    const wb = counterpart(wa, b.windows);
    if (!wb) continue;
    const bothActive = wa.usedPercent > 0 && wb.usedPercent > 0;
    if (bothActive && wa.resetsAt !== undefined && wb.resetsAt !== undefined) {
      const resetDiff = wa.resetsAt - wb.resetsAt;
      if (Math.abs(resetDiff) > RESET_TOLERANCE_MS) {
        votes += Math.sign(resetDiff);
        continue;
      }
    }
    const usedDiff = wa.usedPercent - wb.usedPercent;
    if (Math.abs(usedDiff) > 0.01) votes += Math.sign(usedDiff);
  }
  return votes;
}

/** Returns > 0 when `a` is a better reading to display than `b`. */
function compareReadings(a: Candidate, b: Candidate, now: number): number {
  const rankDiff = planRank(a.plan) - planRank(b.plan);
  if (rankDiff !== 0) return rankDiff;
  const rank = planRank(a.plan);
  if (rank === 4) {
    const progress = windowProgress(a.plan, b.plan, now);
    if (progress !== 0) return progress;
    if (a.readAt !== undefined && b.readAt !== undefined) {
      const ageDiff = a.readAt - b.readAt;
      if (Math.abs(ageDiff) > FRESHNESS_TIE_MS) return ageDiff;
    }
    const usedDiff = maxUsedPercent(a.plan) - maxUsedPercent(b.plan);
    if (Math.abs(usedDiff) > 0.01) return usedDiff;
  } else if (rank === 3) {
    const asOfDiff = (a.plan.asOf ?? 0) - (b.plan.asOf ?? 0);
    if (asOfDiff !== 0) return asOfDiff;
  }
  // Prefer this computer on an exact tie so local wording stays default.
  if (a.host === LOCAL_HOST && b.host !== LOCAL_HOST) return 1;
  if (b.host === LOCAL_HOST && a.host !== LOCAL_HOST) return -1;
  return 0;
}

/**
 * Merges plan usage across this computer and paired machines by subscription:
 * same harness + matching opaque account fingerprint, or matching live window
 * reset times, or matching account name / sole default login (unless live
 * reset times or plan tiers prove they are distinct accounts). Never sums
 * quotas; picks the healthiest, freshest reading and keeps every contributing
 * machine in `sources`.
 */
export function mergeUsagePlans(hosts: readonly HostPlans[], now = Date.now()): MergedUsagePlan[] {
  interface Cluster {
    harness: string;
    hosts: Set<string>;
    members: Candidate[];
  }
  const clusters: Cluster[] = [];

  for (const hostEntry of hosts) {
    const visible = hostEntry.plans.filter((plan) => plan.status !== "notAsked");
    const plans = hostEntry.host === LOCAL_HOST ? visible : usageRows(visible, false).plans;
    for (const plan of plans) {
      const harness = planOf(plan);
      const candidate: Candidate = {
        host: hostEntry.host,
        machine: hostEntry.machine,
        plan,
        readAt: hostEntry.readAt,
      };
      const eligible = clusters.filter((c) => c.harness === harness && !c.hosts.has(hostEntry.host));
      const noConflict = (c: Cluster) => !c.members.some((m) => plansContradict(m.plan, plan, now));

      // Tier 1: exact hashed account fingerprint.
      let target = plan.fingerprint
        ? eligible.find((c) => c.members.some((m) => m.plan.fingerprint === plan.fingerprint))
        : undefined;
      // Tier 2: live window reset signature (same tier + matching active resetsAt).
      if (!target) {
        target = eligible.find(
          (c) => noConflict(c) && c.members.some((m) => sameWindowSignature(m.plan, plan, now)),
        );
      }
      // Tier 3: same named account or sole default login (when live readings don't conflict).
      if (!target) {
        target = eligible.find((c) => noConflict(c) && c.members.some((m) => sameAccountLabel(m.plan, plan)));
      }

      if (target) {
        target.hosts.add(hostEntry.host);
        target.members.push(candidate);
      } else {
        clusters.push({ harness, hosts: new Set([hostEntry.host]), members: [candidate] });
      }
    }
  }

  // Drop remote-only signedOut / notInstalled rows that didn't match anything local.
  const kept = clusters.filter(
    (c) =>
      c.hosts.has(LOCAL_HOST) ||
      c.members.some((m) => m.plan.status !== "signedOut" && m.plan.status !== "notInstalled"),
  );

  const keyCounts = new Map<string, number>();
  return kept.map((cluster) => {
    let winner = cluster.members[0]!;
    for (let i = 1; i < cluster.members.length; i += 1) {
      const next = cluster.members[i]!;
      if (compareReadings(next, winner, now) > 0) winner = next;
    }
    // Preserve a user-chosen account name (e.g. "Paral 2") if the winning copy came
    // from a machine where that same subscription is the unnamed default login.
    const named = cluster.members.find((m) => !!m.plan.account);
    const tier = winner.plan.plan ?? cluster.members.find((m) => !!m.plan.plan)?.plan.plan;
    const plan: UsagePlan = {
      ...winner.plan,
      ...(named && !winner.plan.account ? { name: named.plan.name, account: named.plan.account } : {}),
      ...(tier ? { plan: tier } : {}),
    };
    const baseKey = `${cluster.harness}:${planKey(plan)}`;
    const seen = keyCounts.get(baseKey) ?? 0;
    keyCounts.set(baseKey, seen + 1);
    const key = seen === 0 ? baseKey : `${baseKey}@${winner.host || "local"}#${seen}`;
    return {
      key,
      plan,
      winnerHost: winner.host,
      winnerMachine: winner.machine,
      sources: cluster.members.map((m) => ({
        host: m.host,
        machine: m.machine,
        status: m.plan.status,
        readAt: m.readAt,
        asOf: m.plan.asOf,
        error: m.plan.error,
        reason: m.plan.reason,
      })),
    };
  });
}

/** Group merged plans by harness while keeping each harness's first-seen order. */
export function groupMergedPlans(
  plans: readonly MergedUsagePlan[],
): { harness: string; plans: MergedUsagePlan[] }[] {
  const groups = new Map<string, MergedUsagePlan[]>();
  for (const entry of plans) {
    const harness = planOf(entry.plan);
    const group = groups.get(harness) ?? [];
    group.push(entry);
    groups.set(harness, group);
  }
  return Array.from(groups, ([harness, grouped]) => ({ harness, plans: grouped }));
}

/** Quiet provenance chip + hover detail for a merged card. */
export interface PlanProvenance {
  label: string;
  detail: string;
}

function sourceSummary(source: PlanSource, now: number): string {
  if (source.status === "ok") {
    if (source.asOf !== undefined) return `${source.machine}: cached (${relativeAge(source.asOf, now)})`;
    const age =
      source.readAt !== undefined
        ? now - source.readAt < 60_000
          ? "just now"
          : relativeAge(source.readAt, now)
        : undefined;
    return age ? `${source.machine}: live (${age})` : `${source.machine}: live`;
  }
  if (source.status === "unavailable") {
    if (source.reason === "expired") return `${source.machine}: sign-in expired`;
    if (source.reason === "refused") return `${source.machine}: sign-in refused`;
    if (source.reason === "denied") return `${source.machine}: keychain denied`;
    return `${source.machine}: could not read`;
  }
  if (source.status === "signedOut") return `${source.machine}: signed out`;
  if (source.status === "notInstalled") return `${source.machine}: not installed`;
  return source.machine;
}

/**
 * The quiet provenance pill shown on a card when paired machines are connected
 * under "All machines": names the machine(s) holding the subscription, or
 * `via <machine>` when a remote machine healed a broken or stale local copy.
 */
export function planProvenance(entry: MergedUsagePlan, now = Date.now()): PlanProvenance | null {
  const active = entry.sources.filter((s) => s.status !== "signedOut" && s.status !== "notInstalled");
  if (active.length === 0) return null;

  const healthy = active.filter((s) => s.status === "ok");
  const local = active.find((s) => s.host === LOCAL_HOST);
  const localDegraded =
    local !== undefined &&
    (local.status !== "ok" || (local.asOf !== undefined && entry.plan.asOf === undefined));

  let label: string;
  if (entry.plan.status === "ok" && entry.winnerHost !== LOCAL_HOST && localDegraded) {
    label = `via ${entry.winnerMachine}`;
  } else if (healthy.length >= 2) {
    label =
      healthy.length === 2
        ? `${healthy[0]!.machine} · ${healthy[1]!.machine}`
        : `${healthy[0]!.machine} +${healthy.length - 1}`;
  } else if (healthy.length === 1) {
    label = healthy[0]!.machine;
  } else if (active.length === 1) {
    label = active[0]!.machine;
  } else {
    label =
      active.length === 2
        ? `${active[0]!.machine} · ${active[1]!.machine}`
        : `${active[0]!.machine} +${active.length - 1}`;
  }

  const detail = active.map((s) => sourceSummary(s, now)).join(" · ");
  return { label, detail };
}
