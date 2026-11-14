import type { Harness } from "../acp/types";
import { buildHarnessRequest, type HarnessForm } from "./harnessForm";

export function buildHarnessChange(harness: Harness, form: HarnessForm, retained: string[]) {
  const request = buildHarnessRequest(form, []);
  const change: { id: string; command?: string[]; env?: Record<string, string>; retain_env?: string[] } = {
    id: harness.id,
  };
  if (JSON.stringify(request.command) !== JSON.stringify(harness.command)) change.command = request.command;
  const keep = retained.filter((name) => form.env.some((row) => row.name === name && row.value === ""));
  const env = Object.fromEntries(Object.entries(request.env).filter(([name]) => !keep.includes(name)));
  const original = [...(harness.env_names ?? [])].sort();
  if (Object.keys(env).length || JSON.stringify([...keep].sort()) !== JSON.stringify(original)) {
    change.env = env;
    change.retain_env = keep;
  }
  return change;
}
