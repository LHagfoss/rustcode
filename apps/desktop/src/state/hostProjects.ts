// Orchestration calls about projects: reading, creating, cloning, updating and removing a project,
// on this computer or a paired one. Split out of state/host.ts.

import { type JsonObject, RpcError, str } from "../acp/jsonrpc";
import { Method, projectName } from "../acp/types";
import {
  clonedOn,
  type MachineRef,
  machineFields,
  machineProjects,
  permitted,
  repositories,
  requestOn,
} from "./host";
import {
  cleanProjectPath,
  type Failure,
  failure,
  failureJson,
  isFailure,
  projectOf,
  type RuleThread,
  reachOf,
  sameProject,
} from "./orchestration";
import {
  forgetProject,
  projectEntry,
  projectLabel,
  registerProject,
  updateProjectEntry,
} from "./projectBook";
import { remoteRepository } from "./projects";
import { errorText, request, useApp } from "./store";
import { LOCAL_HOST } from "./threadKeys";
/** A folder as the project book files it, the same rule `projectOf` reads. */
export const projectPath = (path: string) => cleanProjectPath(path);

/** The threads that run in `path`, for a project's row: never a worktree's other repository. */
function projectThreads(path: string): { threads: number; working: boolean } {
  const s = useApp.getState();
  let threads = 0;
  let working = false;
  for (const x of s.sessions) {
    if (projectOf({ folder: x.cwd, worktree: x.worktree }) !== path) continue;
    threads += 1;
    working ||= !!s.threads[x.id]?.running || x.status === "running";
  }
  return { threads, working };
}

/** A path as the engine resolved it: what `_rustcode/project/*` answered. */
function projectAnswer(facts: JsonObject, path: string, extra: JsonObject = {}): JsonObject {
  const entry = projectEntry(path);
  return {
    project: {
      ...facts,
      path,
      name: projectLabel(path),
      ...projectThreads(path),
      ...(entry?.harness ? { harness: entry.harness } : {}),
      ...(entry?.model ? { model: entry.model } : {}),
      registered: !!entry?.registeredAt,
      ...extra,
    },
  };
}

/** The engine's project call, answered as a failure the agent can read. */
async function askProject(
  method: string,
  params: JsonObject,
  timeoutMs: number,
): Promise<JsonObject | Failure> {
  try {
    return await request<JsonObject>(method, params, timeoutMs);
  } catch (error) {
    if (error instanceof RpcError) {
      // -32602 is JSON-RPC's `invalid_params`: the folder is not there.
      return failure(error.code === -32602 ? "not_found" : "target_unavailable", error.message);
    }
    return failure("target_unavailable", `the project could not be read: ${errorText(error)}`);
  }
}

/** Why a path may not be read: this thread's project, or another while the setting allows it. */
function projectRefusal(caller: RuleThread, path: string): Failure | null {
  if (sameProject(path, reachOf(caller))) return null;
  if (useApp.getState().readAcrossProjects) return null;
  return failure(
    "not_allowed",
    "that project is outside this thread's; turn on “Let agents read threads in other projects” in Settings › Orchestration",
  );
}

/**
 * `read_project`: one folder's git facts and the app's own entry for it —
 * display name, default harness and model, thread count. A read: no approval.
 */
export async function readProject(caller: RuleThread, path: string): Promise<JsonObject> {
  const asked = projectPath(path);
  const refused = projectRefusal(caller, asked);
  if (refused) return failureJson(refused);
  const facts = await askProject(Method.projectRead, { path: asked }, 15_000);
  if (isFailure(facts)) return failureJson(facts);
  return projectAnswer(facts, projectPath(String(facts.path ?? asked)));
}

/**
 * `read_project` on a paired machine: that folder's git facts, read by the
 * engine there, and how many of its threads this app lists. The user named the
 * machine, so the cross-project setting (this computer's) does not apply.
 */
export async function readProjectOn(machine: MachineRef, path: string): Promise<JsonObject> {
  const asked = projectPath(path);
  let facts: JsonObject;
  try {
    facts = await requestOn<JsonObject>(machine.host, Method.projectRead, { path: asked }, 15_000);
  } catch (error) {
    const code = error instanceof RpcError && error.code === -32602 ? "not_found" : "target_unavailable";
    return failureJson(failure(code, `${machine.name} could not read ${asked}: ${errorText(error)}`));
  }
  const at = projectPath(String(facts.path ?? asked));
  repositories.set(`${machine.host}\n${at}`, remoteRepository(str(facts.remote)));
  const threads = useApp
    .getState()
    .sessions.filter(
      (x) =>
        (x.host ?? LOCAL_HOST) === machine.host &&
        sameProject(projectOf({ folder: x.cwd, worktree: x.worktree }), at),
    );
  return {
    ...machineFields(machine),
    project: {
      ...facts,
      path: at,
      name: projectName(at),
      threads: threads.length,
      working: threads.some((x) => !!useApp.getState().threads[x.id]?.running || x.status === "running"),
    },
  };
}

/** A folder's parent, on whichever platform wrote it. */
const parentFolder = (path: string) => path.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "");

/**
 * `clone_project` on a paired machine: `git clone` by the engine there. The
 * parent is the call's, else the one folder that machine's projects share;
 * with several it asks for one rather than pick. The clone is remembered as
 * one of that machine's projects, so the next `start_session` finds it by
 * its repository.
 */
export async function cloneProjectOn(
  machine: MachineRef,
  caller: RuleThread,
  call: { url: string; parent?: string; name?: string },
): Promise<JsonObject> {
  const parents = [
    ...new Set(machineProjects(machine.host).map((project) => parentFolder(project.path))),
  ].filter(Boolean);
  const parent = call.parent ? projectPath(call.parent) : parents.length === 1 ? parents[0]! : "";
  if (!parent) {
    return failureJson(
      failure(
        "invalid",
        parents.length
          ? `pass parent: the folder on ${machine.name} to clone into, such as ${parents.slice(0, 10).join(", ")}`
          : `pass parent: the absolute folder on ${machine.name} to clone into`,
      ),
    );
  }
  const no = await permitted(caller, `project:clone@${machine.id}:${call.url}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: call.name ? `${parent}/${call.name}` : parent,
    detail: `It would run git clone on ${call.url} in ${parent} on ${machine.name}. A large repository over a slow link can take minutes.`,
    machine: machine.name,
  });
  if (no) return failureJson(no);
  let facts: JsonObject;
  try {
    facts = await requestOn<JsonObject>(
      machine.host,
      Method.projectClone,
      { url: call.url, parent, ...(call.name ? { name: call.name } : {}) },
      620_000,
    );
  } catch (error) {
    const code = error instanceof RpcError && error.code === -32602 ? "not_found" : "target_unavailable";
    return failureJson(failure(code, `the clone on ${machine.name} failed: ${errorText(error)}`));
  }
  const at = projectPath(String(facts.path ?? parent));
  const cloned = clonedOn.get(machine.host) ?? [];
  if (!cloned.includes(at)) clonedOn.set(machine.host, [...cloned, at]);
  repositories.set(`${machine.host}\n${at}`, remoteRepository(str(facts.remote) ?? call.url));
  return {
    ...machineFields(machine),
    project: { ...facts, path: at, name: projectName(at), threads: 0, created: true },
  };
}

/** `create_project`: register an existing folder, or make a new one a repository. */
export async function createProject(
  caller: RuleThread,
  call: { path: string; init?: boolean },
): Promise<JsonObject> {
  const path = projectPath(call.path);
  const no = await permitted(caller, `project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would register ${path} as a project${call.init === false ? "" : ", making it a git repository if it is not one"}.`,
  });
  if (no) return failureJson(no);
  const facts = await askProject(Method.projectCreate, { path, init: call.init ?? true }, 60_000);
  if (isFailure(facts)) return failureJson(facts);
  const resolved = projectPath(String(facts.path ?? path));
  registerProject(resolved);
  return projectAnswer(facts, resolved, { created: true });
}

/** `clone_project`: `git clone` into a parent folder, through the engine, then register it. */
export async function cloneProject(
  caller: RuleThread,
  call: { url: string; parent?: string; name?: string },
): Promise<JsonObject> {
  const s = useApp.getState();
  const parent = projectPath(call.parent || projectOf(caller) || s.projectCwd || s.defaultCwd);
  if (!parent) {
    return failureJson(failure("failed", "there is no folder to clone into; pass parent"));
  }
  const no = await permitted(caller, `project:clone:${call.url}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: call.name ? `${parent}/${call.name}` : parent,
    detail: `It would run git clone on ${call.url} in ${parent} and register the result as a project. A large repository over a slow link can take minutes.`,
  });
  if (no) return failureJson(no);
  const facts = await askProject(
    Method.projectClone,
    { url: call.url, parent, ...(call.name ? { name: call.name } : {}) },
    620_000,
  );
  if (isFailure(facts)) return failureJson(facts);
  const resolved = projectPath(String(facts.path ?? parent));
  registerProject(resolved);
  return projectAnswer(facts, resolved, { created: true });
}

/** `update_project`: a display name, and the harness and model new threads there start on. */
export async function updateProject(
  caller: RuleThread,
  call: { path: string; name?: string; harness?: string; model?: string },
): Promise<JsonObject> {
  const path = projectPath(call.path);
  const harnesses = useApp.getState().harnesses?.harnesses ?? [];
  if (call.harness && !harnesses.some((h) => h.id === call.harness)) {
    return failureJson(
      failure(
        "not_found",
        `no harness ${call.harness}; list_harnesses shows the ids${harnesses.length ? ` (${harnesses.map((h) => h.id).join(", ")})` : ""}`,
      ),
    );
  }
  // A model without a harness still says what it is: the folder's new threads start on it.
  const on = call.harness ?? call.model;
  const no = await permitted(caller, `project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would ${[
      call.name && `call it “${call.name}”`,
      on && `start new threads there on ${on}${call.harness && call.model ? ` with ${call.model}` : ""}`,
    ]
      .filter(Boolean)
      .join(" and ")}. The folder itself is not touched.`,
  });
  if (no) return failureJson(no);
  const entry = updateProjectEntry(path, {
    name: call.name,
    harness: call.harness,
    model: call.model,
  });
  return projectAnswer({}, path, {
    ...(entry.name ? { name: entry.name } : {}),
  });
}

/** `remove_project`: forget the folder; its files and its threads stay. */
export async function removeProject(caller: RuleThread, call: { path: string }): Promise<JsonObject> {
  const path = projectPath(call.path);
  const no = await permitted(caller, `remove-project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would unregister ${path} as a project. The folder and every file in it stay on disk, and its threads stay; only its display name and defaults go.`,
  });
  if (no) return failureJson(no);
  const known = forgetProject(path);
  return { path, removed: true, known, files_deleted: false };
}
