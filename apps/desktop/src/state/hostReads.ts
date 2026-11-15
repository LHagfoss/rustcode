// Orchestration reads that need no approval: listing harnesses, threads, worktrees and projects,
// and searching threads. Split out of state/host.ts.

import type { JsonObject } from "../acp/jsonrpc";
import { harnessPlanKey } from "../acp/rustcode";
import type { Harness, HarnessModel, HarnessSnapshot } from "../acp/types";
import { restoreChoice, unavailableReason } from "./harness";
import { usageOf } from "./harnessUsage";
import { type MachineRef, machineFields, requestOn } from "./host";
import { ranWith } from "./hostStart";
import { accountSignedOut } from "./logins";
import {
  failure,
  failureJson,
  MAX_PROJECTS,
  projectOf,
  type RuleThread,
  reachesThread,
  reachOf,
  sameProject,
} from "./orchestration";
import { projectLabel, registeredProjects } from "./projectBook";
import { isDataFolder, isRootFolder } from "./projects";
import {
  type AppState,
  effortMemory,
  ensureHarnessModels,
  localChoices,
  planCacheStale,
  refreshReadPlanLogins,
  searchThreads,
  useApp,
} from "./store";
import { splitThreadKey, threadKey } from "./threadKeys";
/** Models listed per harness: an agent's context pays for each one. */
export const LISTED_MODELS = 12;
/** Models a `model` filter returns: every match worth picking from, still bounded. */
export const FILTERED_MODELS = 50;
/** Said beside `omitted_models`, so an agent narrows instead of guessing at what it cannot see. */
const MODEL_FILTER_HINT = "pass model to search the omitted ones";
/** Said when a `model` filter matched nothing, so an agent narrows it again. */
const MODEL_MISS_HINT = "no model matches; pass a shorter substring";

/** Whether a model matches a `model` filter: its id or its name, either case. */
const matchesModel = (m: HarnessModel, filter: string) =>
  m.id.toLowerCase().includes(filter) || m.name.toLowerCase().includes(filter);

/**
 * The models this project's threads ran on, most recent first, per harness: the
 * models an agent is likeliest to want are the ones the user already uses here.
 */
function usedModels(caller: RuleThread): Map<string, string[]> {
  const reach = reachOf(caller);
  const recent = useApp
    .getState()
    .sessions.filter((t) => t.harness && t.model && sameProject(reach, t.worktree?.repo || t.repo || t.cwd))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));
  const out = new Map<string, string[]>();
  for (const t of recent) {
    const used = out.get(t.harness!) ?? [];
    if (!used.includes(t.model!)) used.push(t.model!);
    out.set(t.harness!, used);
  }
  return out;
}

/**
 * A harness's models in the order that matters: the one a new thread on it starts
 * with, then the ones this project's threads used recently, then the harness's own
 * order. A `filter` narrows to the models whose id or name contains it.
 */
function orderedModels(
  h: Harness,
  configured: string | null,
  used: string[],
  filter: string | undefined,
): HarnessModel[] {
  // Only the models it lists lead: a remembered one this listing no longer has
  // is not offered.
  const ids = new Set(h.models.map((m) => m.id));
  const lead = [configured, ...used].filter((id): id is string => !!id && ids.has(id));
  const rank = new Map<string, number>();
  for (const id of lead) if (!rank.has(id)) rank.set(id, rank.size);
  const models = filter ? h.models.filter((m) => matchesModel(m, filter)) : [...h.models];
  // The sort is stable, so the rest stay in the harness's own order.
  return models.sort((a, b) => (rank.get(a.id) ?? lead.length) - (rank.get(b.id) ?? lead.length));
}

/** The efforts every listed model shares, when they share one and it is not empty. */
function sharedEfforts(models: HarnessModel[]): string[] | undefined {
  const first = models[0]?.efforts ?? [];
  if (!first.length) return undefined;
  const same = models.every(
    (m) => m.efforts.length === first.length && m.efforts.every((effort, i) => effort === first[i]),
  );
  return same ? first : undefined;
}

export async function listHarnesses(
  caller: RuleThread,
  asked: { harness?: string; model?: string } = {},
): Promise<JsonObject> {
  const snapshot = useApp.getState().harnesses;
  const harnesses = snapshot?.harnesses ?? [];
  const signedOut = useApp.getState().signedOutLogins;
  await Promise.all(harnesses.filter((h) => !unavailableReason(h)).map((h) => ensureHarnessModels(h.id)));
  const fresh = useApp.getState().harnesses?.harnesses ?? harnesses;
  // The models a listing just brought in are the ones on record now, so the
  // choice a new thread starts on is restored against those, not the snapshot
  // taken before the ask.
  const current: HarnessSnapshot | undefined = snapshot ? { ...snapshot, harnesses: fresh } : undefined;
  const wanted = asked.harness?.toLowerCase();
  // An id is exact and wins; a display name may be shared by a harness the user
  // added, so several matches are refused rather than one picked silently.
  const byId = wanted ? fresh.filter((h) => h.id.toLowerCase() === wanted) : fresh;
  const named = wanted && !byId.length ? fresh.filter((h) => h.name.toLowerCase() === wanted) : byId;
  if (wanted && !named.length) {
    return failureJson(
      failure(
        "invalid",
        `no harness “${asked.harness}”; list_harnesses has ${fresh.map((h) => h.id).join(", ")}`,
      ),
    );
  }
  if (wanted && named.length > 1) {
    return failureJson(
      failure(
        "invalid",
        `“${asked.harness}” names ${named.length} harnesses; pass the id: ${named.map((h) => h.id).join(", ")}`,
      ),
    );
  }
  const listed = named;
  const filter = asked.model?.toLowerCase();
  const used = usedModels(caller);
  // What a new thread on each harness starts with: the same rule the composer uses.
  const remembered = localChoices();
  const efforts = effortMemory();
  const configuredModels = current
    ? new Map(fresh.map((h) => [h.id, restoreChoice(current, h.id, remembered, efforts).model]))
    : undefined;
  // Each login's plan windows come from the cache Settings › Usage and the
  // composer's meter fill, never from a read this call starts: a model's tool
  // call must not raise a Keychain prompt or an Anthropic request of its own.
  // A stale cache is refreshed in the background so that the next call has
  // newer numbers (`refreshReadPlanLogins`): the default login and the account
  // of the thread on screen, plus every named login that was read before, since
  // those are the accounts an agent is choosing between. It honours the pauses
  // and the read's rate limit, and it is not awaited: this answer is what the
  // cache holds now.
  const cache = useApp.getState().planCache;
  if (planCacheStale(cache)) refreshReadPlanLogins();
  const usage = (harness: string, account?: string | null) =>
    usageOf(cache[harnessPlanKey(harness, account)]);
  return {
    current: { harness: caller.harness ?? "rustcode", ...ranWith(caller.id) },
    harnesses: listed.map((h) => {
      const reason = unavailableReason(h);
      const models = orderedModels(h, configuredModels?.get(h.id) ?? null, used.get(h.id) ?? [], filter);
      const shown = models.slice(0, filter ? FILTERED_MODELS : LISTED_MODELS);
      const omitted = models.length - shown.length;
      const shared = sharedEfforts(shown);
      const entries = shown.map((m) => ({
        id: m.id,
        name: m.name,
        ...(shared || !m.efforts.length ? {} : { efforts: m.efforts }),
      }));
      // The harness's own login has no id, so its windows sit on the harness
      // entry: they are what a thread runs on with no `account`. A named
      // login's own numbers sit on its entry, and are never swapped with it.
      const own = usage(h.id);
      return {
        id: h.id,
        name: h.name,
        available: !reason,
        ...(own ? { usage: own } : {}),
        // The logins start_session and fan_out may name; the harness's own sign-in has no id.
        ...(h.accounts?.length
          ? {
              accounts: h.accounts.map((a) => {
                const numbers = usage(h.id, a.id);
                return {
                  id: a.id,
                  name: a.name,
                  auth: a.auth,
                  // The plan read saw this login signed out: naming it is refused.
                  ...(accountSignedOut(h.id, a.id, signedOut) ? { signed_out: true } : {}),
                  ...(numbers ? { usage: numbers } : {}),
                };
              }),
            }
          : {}),
        ...(reason
          ? { reason }
          : {
              // The efforts every listed model shares sit here once, not on each.
              ...(shared ? { efforts: shared } : {}),
              models: entries,
              ...(omitted ? { omitted_models: omitted, filter: MODEL_FILTER_HINT } : {}),
              ...(!entries.length && filter ? { filter: MODEL_MISS_HINT } : {}),
            }),
      };
    }),
  };
}

/**
 * What a paired machine's own index finds for `query`, keyed as this app
 * keys its threads; empty for a short query or an engine without the index.
 */
async function searchThreadsOn(host: string, query: string): Promise<Record<string, string>> {
  if (query.trim().length < 3) return {};
  try {
    const result = await requestOn<{ results?: { session_id?: string; snippet?: string }[] } | null>(
      host,
      "_rustcode/threads/search",
      { query, limit: 30 },
      30_000,
    );
    const hits: Record<string, string> = {};
    for (const hit of result?.results ?? []) {
      if (hit.session_id) hits[threadKey(host, hit.session_id)] = hit.snippet ?? "";
    }
    return hits;
  } catch {
    return {};
  }
}

/**
 * `search_sessions`: titles and what was said, from the index of the computer
 * the threads run on. A named machine's threads are all of its threads, as
 * `list_sessions` lists them, each with its project there.
 */
export async function searchSessions(
  all: RuleThread[],
  caller: RuleThread,
  query: string,
  allProjects = false,
  machine?: MachineRef,
): Promise<JsonObject> {
  const host = machine?.host ?? "";
  const said = host ? await searchThreadsOn(host, query) : await searchThreads(query);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = all
    .filter((t) => (host ? splitThreadKey(t.id).host === host : allProjects || reachesThread(caller, t)))
    .filter((t) => t.id in said || words.every((w) => t.title.toLowerCase().includes(w)))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 30);
  return {
    ...(machine ? machineFields(machine) : {}),
    sessions: matches.map((t) => ({
      session_id: t.id,
      title: t.title,
      status: t.status ?? (t.running ? "running" : "idle"),
      ...(allProjects ? { project: projectOf(t) } : {}),
      ...(said[t.id] ? { snippet: said[t.id] } : {}),
    })),
  };
}

/** `list_worktrees`: this project's worktree threads, every project's, or every one a named machine has. */
export function listWorktrees(
  all: RuleThread[],
  caller: RuleThread,
  allProjects = false,
  machine?: MachineRef,
): JsonObject {
  const host = machine?.host ?? "";
  const reached = (t: RuleThread) =>
    host ? splitThreadKey(t.id).host === host : allProjects || reachesThread(caller, t);
  return {
    ...(machine ? machineFields(machine) : {}),
    worktrees: all.flatMap((t) =>
      t.worktree && reached(t)
        ? [
            {
              session_id: t.id,
              title: t.title,
              ...(allProjects ? { project: projectOf(t) } : {}),
              branch: t.worktree.branch,
              path: t.worktree.path,
            },
          ]
        : [],
    ),
  };
}

/**
 * `list_projects`: every project the user has a thread in or registered,
 * most recently updated first. A registered folder with no thread yet shows
 * with 0, so a project created or cloned before its first thread is there.
 */
export function listProjects(s: AppState): JsonObject {
  const byPath = new Map<string, { path: string; threads: number; working: boolean; updatedAt: number }>();
  for (const x of s.sessions) {
    // A thread whose machine never recorded its folder has no project to count (issue 3425).
    if (x.workspaceUnknown) continue;
    const path = projectOf({ folder: x.cwd, worktree: x.worktree });
    if (!path || isRootFolder(path) || isDataFolder(path)) continue;
    const entry = byPath.get(path) ?? { path, threads: 0, working: false, updatedAt: 0 };
    entry.threads += 1;
    entry.working ||= !!s.threads[x.id]?.running || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    byPath.set(path, entry);
  }
  for (const { path, entry } of registeredProjects()) {
    if (byPath.has(path)) continue;
    byPath.set(path, {
      path,
      threads: 0,
      working: false,
      updatedAt: entry.registeredAt ?? 0,
    });
  }
  const projects = [...byPath.values()]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, MAX_PROJECTS)
    .map((p) => ({ path: p.path, name: projectLabel(p.path), threads: p.threads, working: p.working }));
  return { projects };
}
