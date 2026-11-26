/**
 * Workflow jobs (issue 4443): a job that runs a saved `.rustcode/workflows/<name>.js` instead of a
 * prompt. Saved through `_rustcode/loops/save` with `workflow` and `args`; the editor's picker lists
 * a project's workflows through `_rustcode/workflows/saved`.
 */
import { arr, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { connectionFor, request } from "./store";

/** Typed args as the job saves them: a JSON object, `{}` when blank, `null` when not an object. */
export function parseWorkflowArgs(text: string | undefined): JsonObject | null {
  const trimmed = (text ?? "").trim();
  if (!trimmed) return {};
  try {
    return obj(JSON.parse(trimmed) as Json) ?? null;
  } catch {
    return null;
  }
}

/** What a workflow args field shows as wrong, or null when it saves. */
export function workflowArgsProblem(text: string | undefined): string | null {
  return parseWorkflowArgs(text) ? null : 'Args are a JSON object, like {"label": "bug"}.';
}

/**
 * A Loop's save parameters turned into a workflow job's: the workflow and its args in place of
 * the prompt, a script, a thread or a model, which a workflow job does not take.
 */
export function workflowParams(params: JsonObject, workflow: string, args: string | undefined): JsonObject {
  const {
    prompt: _prompt,
    script: _script,
    threadId: _thread,
    model: _model,
    effort: _effort,
    ...rest
  } = params;
  return { ...rest, prompt: "", workflow: workflow.trim(), args: parseWorkflowArgs(args) ?? {} };
}

/** The workflows saved in a project, by name; none when the folder is missing or the call fails. */
export async function savedWorkflows(workspace: string, host = ""): Promise<string[]> {
  try {
    const conn = host ? connectionFor(host) : null;
    if (host && !conn) return [];
    const answer = conn
      ? await conn.request<Json>(Method.workflowsSaved, { workspace }, 10_000)
      : await request<Json>(Method.workflowsSaved, { workspace }, 10_000);
    return arr(obj(answer)?.names).flatMap((name) => str(name) ?? []);
  } catch {
    return [];
  }
}
