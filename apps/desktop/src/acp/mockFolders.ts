// The mock engine's folders, as `_rustcode/project/browse` and `/create` answer them: a small
// fixed tree, so Browse Folders… on a paired machine has something to walk, and a folder made
// there shows up in its parent the next time it is listed.

import type { JsonObject } from "./jsonrpc";

const HOME = "/home/me";
const tree = new Map<string, string[]>([
  ["/", ["etc", "home", "srv", "var"]],
  ["/home", ["me"]],
  [HOME, ["code", "Downloads", "notes"]],
  [`${HOME}/code`, ["api", "website"]],
  ["/srv", ["apps"]],
]);
const repositories = new Set([`${HOME}/code/api`, `${HOME}/code/website`]);

const parentOf = (path: string) => (path === "/" ? null : path.replace(/\/[^/]+$/, "") || "/");
const join = (parent: string, name: string) => (parent === "/" ? `/${name}` : `${parent}/${name}`);

/** `_rustcode/project/browse`: the folders in one, home when none is named. */
export function mockBrowse(params: JsonObject): JsonObject {
  const path = typeof params.path === "string" && params.path !== "~" ? params.path : HOME;
  return {
    path,
    parent: parentOf(path),
    home: HOME,
    is_git: repositories.has(path),
    folders: (tree.get(path) ?? [])
      .slice()
      .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))
      .map((name) => ({ name, path: join(path, name), is_git: repositories.has(join(path, name)) })),
    truncated: false,
  };
}

/** `_rustcode/project/create`: the folder, a repository unless `init` is false. */
export function mockCreate(params: JsonObject): JsonObject {
  const path = typeof params.path === "string" && params.path ? params.path : "/Users/me/code/new-project";
  const init = params.init !== false;
  const name = path.split("/").filter(Boolean).pop() ?? path;
  const parent = parentOf(path);
  if (parent) tree.set(parent, [...new Set([...(tree.get(parent) ?? []), name])]);
  if (init) repositories.add(path);
  return {
    path,
    name,
    is_git: init,
    remote: null,
    branch: init ? "main" : null,
    default_branch: init ? "main" : null,
    created: true,
    initialized: init,
  };
}
