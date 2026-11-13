// Folders dropped on the sidebar become projects (issue 3608): the same as ⌘K › "Add project
// folder", without the folder picker. A folder that is a project already opens that project
// instead of adding it twice, and anything that is not a readable folder is refused with a
// toast that says which and why. The plan is pure, so its tests need no shell or store.

import { projectName } from "../acp/types";
import type { PathKind } from "../lib/native";
import { cleanProjectPath } from "./orchestration";

/** What a drop does with each path it carried. */
export interface FolderDropPlan {
  /** Folders that are not a project yet: registered, then a new thread is ready in the first. */
  add: string[];
  /** Folders that are a project already: opened, never registered again. */
  open: string[];
  /** Paths that are not a folder this app can open, with why. */
  rejected: { path: string; kind: Exclude<PathKind, "folder"> }[];
}

/**
 * Sorts a drop's paths by what they are. `kinds` is `pathKinds(paths)`, in the same order;
 * `known` is every project this computer has (its threads' folders and the registered ones).
 * The same folder dropped twice, or spelled with a trailing slash, counts once.
 */
export function planFolderDrop(
  paths: readonly string[],
  kinds: readonly PathKind[],
  known: Iterable<string>,
): FolderDropPlan {
  const projects = new Set([...known].map(cleanProjectPath));
  const seen = new Set<string>();
  const plan: FolderDropPlan = { add: [], open: [], rejected: [] };
  paths.forEach((raw, i) => {
    const path = cleanProjectPath(raw);
    if (!path || seen.has(path)) return;
    seen.add(path);
    const kind = kinds[i] ?? "missing";
    if (kind !== "folder") plan.rejected.push({ path, kind });
    else if (projects.has(path)) plan.open.push(path);
    else plan.add.push(path);
  });
  return plan;
}

/** The folder a drop leaves the new-thread screen in: the first one added, else the first opened. */
export function dropTarget(plan: FolderDropPlan): string | undefined {
  return plan.add[0] ?? plan.open[0];
}

const REASON: Record<Exclude<PathKind, "folder">, string> = {
  file: "is a file, not a folder",
  missing: "could not be found",
  denied: "could not be read (permission denied)",
};

/** What the toasts say about a drop: what it added or opened, and what it refused. */
export function dropMessages(plan: FolderDropPlan): { done?: string; refused?: string } {
  const names = (paths: string[]) => paths.map(projectName).join(", ");
  const parts: string[] = [];
  if (plan.add.length === 1) parts.push(`Added ${names(plan.add)}`);
  else if (plan.add.length > 1) parts.push(`Added ${plan.add.length} projects: ${names(plan.add)}`);
  if (plan.open.length) {
    const what = plan.open.length === 1 ? "a project" : "projects";
    parts.push(`${names(plan.open)} ${plan.open.length === 1 ? "is" : "are"} already ${what}`);
  }
  let refused: string | undefined;
  if (plan.rejected.length === 1) {
    const { path, kind } = plan.rejected[0]!;
    refused = `${projectName(path)} ${REASON[kind]}. Only folders can be added as projects.`;
  } else if (plan.rejected.length > 1) {
    refused = `${plan.rejected.length} items were not added: ${plan.rejected
      .map(({ path, kind }) => `${projectName(path)} ${REASON[kind]}`)
      .join("; ")}. Only folders can be added as projects.`;
  }
  return { done: parts.length ? `${parts.join(". ")}.` : undefined, refused };
}
