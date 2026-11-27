/**
 * Grading for the MCP tool-use eval: plain logic over the tool calls and the
 * final text of one sample, so `run.test.ts` covers it in CI.
 */

/** One tool call a model made: the public tool name (`update_thread`, `Bash`) and its input. */
export interface Call {
  tool: string;
  input: Record<string, unknown>;
}

/** A call a check looks for. */
export interface CallMatch {
  tool: string;
  /** Every key here must equal the call's value (deep equality). */
  args?: Record<string, unknown>;
  /** Keys the call must set. */
  has?: string[];
  /** Keys the call must leave out. */
  lacks?: string[];
}

/** What one case expects of every sample. All set fields must hold. */
export interface Checks {
  /** The first call must match. */
  first?: CallMatch;
  /** Each must match some call. */
  calls?: CallMatch[];
  /** At least one must match some call. */
  anyOf?: CallMatch[];
  /** None may match any call. */
  never?: CallMatch[];
  /** No `Bash` command may match these patterns. */
  bashNever?: RegExp[];
  /** Some `Bash` command must match each pattern. */
  bashSome?: RegExp[];
  /** The final text may not match these patterns. */
  textNever?: RegExp[];
}

/** One sample's outcome: what the model did and which checks failed. */
export interface Sample {
  calls: Call[];
  text: string;
}

function equal(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Whether `call` satisfies `match`. */
export function matches(call: Call, match: CallMatch): boolean {
  if (call.tool !== match.tool) return false;
  for (const [key, value] of Object.entries(match.args ?? {})) {
    if (!equal(call.input[key], value)) return false;
  }
  for (const key of match.has ?? []) {
    if (call.input[key] === undefined || call.input[key] === null) return false;
  }
  for (const key of match.lacks ?? []) {
    if (call.input[key] !== undefined && call.input[key] !== null) return false;
  }
  return true;
}

/** How a match reads in a failure line. */
export function describe(match: CallMatch): string {
  const parts = [
    ...Object.entries(match.args ?? {}).map(([key, value]) => `${key}=${JSON.stringify(value)}`),
    ...(match.has ?? []).map((key) => `${key} set`),
    ...(match.lacks ?? []).map((key) => `no ${key}`),
  ];
  return parts.length ? `${match.tool}(${parts.join(", ")})` : match.tool;
}

/** The shell commands a sample ran. */
export function commands(calls: Call[]): string[] {
  return calls
    .filter((call) => call.tool === "Bash")
    .map((call) => String(call.input.command ?? ""));
}

/** The checks `sample` fails, as one line each; empty when it passes. */
export function grade(sample: Sample, checks: Checks): string[] {
  const failures: string[] = [];
  const { calls } = sample;
  if (checks.first) {
    const first = calls[0];
    if (!first || !matches(first, checks.first)) {
      failures.push(`first call was ${first ? first.tool : "none"}, wanted ${describe(checks.first)}`);
    }
  }
  for (const match of checks.calls ?? []) {
    if (!calls.some((call) => matches(call, match))) failures.push(`never called ${describe(match)}`);
  }
  if (checks.anyOf && !checks.anyOf.some((match) => calls.some((call) => matches(call, match)))) {
    failures.push(`called none of ${checks.anyOf.map(describe).join(", ")}`);
  }
  for (const match of checks.never ?? []) {
    if (calls.some((call) => matches(call, match))) failures.push(`called ${describe(match)}`);
  }
  const ran = commands(calls);
  for (const pattern of checks.bashNever ?? []) {
    const hit = ran.find((command) => pattern.test(command));
    if (hit !== undefined) failures.push(`ran \`${hit}\` (matches ${pattern})`);
  }
  for (const pattern of checks.bashSome ?? []) {
    if (!ran.some((command) => pattern.test(command))) failures.push(`ran nothing matching ${pattern}`);
  }
  for (const pattern of checks.textNever ?? []) {
    if (pattern.test(sample.text)) failures.push(`answer matches ${pattern}`);
  }
  return failures;
}

/** A case fails when half or more of its samples fail. */
export function casePassed(failedSamples: number, samples: number): boolean {
  return samples > 0 && failedSamples * 2 < samples;
}
