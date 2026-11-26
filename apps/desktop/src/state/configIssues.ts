import { create } from "zustand";
import { arr, type Json, obj } from "../acp/jsonrpc";

/** A config file the engine could not read (`configIssues` on `_rustcode/settings/get`). */
export interface ConfigIssue {
  path: string;
  scope: "user" | "project";
  message: string;
}

/** The unreadable config files of this computer's engine; empty when every file loaded. */
export const useConfigIssues = create<{ issues: ConfigIssue[] }>(() => ({ issues: [] }));

/** The issues in a settings answer; an engine that predates the field reports none. */
export function parseConfigIssues(answer: Json | undefined): ConfigIssue[] {
  return arr(obj(answer)?.configIssues).flatMap((raw) => {
    const issue = obj(raw);
    const path = issue?.path;
    const message = issue?.message;
    if (typeof path !== "string" || typeof message !== "string") return [];
    return [{ path, message, scope: issue?.scope === "project" ? "project" : "user" }];
  });
}

export function rememberConfigIssues(answer: Json | undefined): void {
  useConfigIssues.setState({ issues: parseConfigIssues(answer) });
}
