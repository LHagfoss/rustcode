/**
 * MCP tool-use eval: replays each case on a Claude model with RustCode's MCP
 * instructions and tools from this checkout (`rc tools manifest`), answers
 * tool calls with canned results, and grades the calls. See README.md.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { CASES, type Case } from "./cases.ts";
import { type Call, casePassed, grade } from "./grade.ts";

const ROOT = resolve(import.meta.dir, "../..");
const PREFIX = "mcp__rustcode__";

interface Manifest {
  instructions: string;
  tools: { name: string; description?: string; inputSchema?: unknown }[];
}

interface Options {
  out: string;
  model: string;
  samples: number;
  cases: string[];
  baseline?: string;
  manifest?: string;
  maxTurns: number;
}

interface Row {
  case: string;
  sample: number;
  model: string;
  failures: string[];
  calls: Call[];
  text: string;
  usage: { input: number; output: number };
}

function parseArgs(argv: string[]): Options {
  const options: Options = { out: "", model: "claude-sonnet-5-5", samples: 3, cases: [], maxTurns: 6 };
  for (let i = 0; i < argv.length; i++) {
    const value = argv[i + 1];
    switch (argv[i]) {
      case "--out": options.out = value; i++; break;
      case "--model": options.model = value; i++; break;
      case "--samples": options.samples = Number(value); i++; break;
      case "--case": options.cases = value.split(","); i++; break;
      case "--baseline": options.baseline = value; i++; break;
      case "--manifest": options.manifest = value; i++; break;
      case "--max-turns": options.maxTurns = Number(value); i++; break;
      default: throw new Error(`unknown flag ${argv[i]}`);
    }
  }
  if (!options.out) throw new Error("--out <dir> is required");
  return options;
}

function loadEnv() {
  const file = join(ROOT, ".env");
  if (!existsSync(file)) return;
  for (const line of readFileSync(file, "utf8").split("\n")) {
    const found = /^\s*([A-Z0-9_]+)\s*=\s*(.*)\s*$/.exec(line);
    if (found && process.env[found[1]] === undefined) {
      process.env[found[1]] = found[2].replace(/^["']|["']$/g, "");
    }
  }
}

/** The manifest from a file, or from `rc tools manifest` built from this checkout. */
function loadManifest(file?: string): Manifest {
  if (file) return JSON.parse(readFileSync(file, "utf8"));
  const built = Bun.spawnSync(
    ["nice", "-n", "10", "cargo", "run", "--quiet", "-p", "rustcode-cli", "--bin", "rc", "--", "tools", "manifest"],
    { cwd: ROOT, stderr: "inherit" },
  );
  if (built.exitCode !== 0) throw new Error("`rc tools manifest` failed; pass --manifest <file>");
  return JSON.parse(built.stdout.toString());
}

const BASH_TOOL = {
  name: "Bash",
  description: "Run a shell command in the project folder and return its output.",
  input_schema: {
    type: "object",
    properties: { command: { type: "string" }, description: { type: "string" } },
    required: ["command"],
  },
};

/** The system prompt a harness gives: its own preamble, project instructions, then the MCP server's. */
export function systemPrompt(manifest: Manifest, testCase: Case, agentsMd: string): string {
  const parts = [
    "You are a coding agent working in the user's project at /home/user/app (the GitHub repository acme/app), on the main branch. Use the tools to act; keep answers short.",
  ];
  if (testCase.harnessNote) parts.push(testCase.harnessNote);
  if (testCase.agentsMd) parts.push(`Project instructions (AGENTS.md):\n\n${agentsMd}`);
  parts.push(`# MCP Server Instructions\n\n## rustcode\n${manifest.instructions}`);
  return parts.join("\n\n");
}

function anthropicTools(manifest: Manifest) {
  return [
    BASH_TOOL,
    ...manifest.tools.map((tool) => ({
      name: PREFIX + tool.name,
      description: tool.description ?? "",
      input_schema: tool.inputSchema ?? { type: "object", properties: {} },
    })),
  ];
}

/** The canned answer to one call. */
export function answer(testCase: Case, call: Call): string {
  if (call.tool === "Bash") {
    const command = String(call.input.command ?? "");
    return testCase.shell?.find((shell) => shell.match.test(command))?.output ?? "";
  }
  return testCase.results?.[call.tool] ?? '{"ok":true}';
}

async function sample(options: Options, testCase: Case, system: string, tools: unknown[]): Promise<Omit<Row, "case" | "sample" | "model" | "failures">> {
  const messages: { role: string; content: unknown }[] = [
    ...(testCase.history ?? []),
    { role: "user", content: testCase.prompt },
  ];
  const calls: Call[] = [];
  const usage = { input: 0, output: 0 };
  let text = "";
  for (let turn = 0; turn < options.maxTurns; turn++) {
    const response = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-api-key": process.env.ANTHROPIC_API_KEY ?? "",
        "anthropic-version": "2023-06-01",
      },
      body: JSON.stringify({ model: options.model, max_tokens: 2048, system, tools, messages }),
    });
    if (!response.ok) throw new Error(`Anthropic API ${response.status}: ${await response.text()}`);
    const body = (await response.json()) as {
      content: { type: string; id?: string; name?: string; input?: Record<string, unknown>; text?: string }[];
      stop_reason: string;
      usage: { input_tokens: number; output_tokens: number };
    };
    usage.input += body.usage.input_tokens;
    usage.output += body.usage.output_tokens;
    messages.push({ role: "assistant", content: body.content });
    const results = [];
    for (const block of body.content) {
      if (block.type === "text") text += block.text ?? "";
      if (block.type !== "tool_use") continue;
      const call = { tool: (block.name ?? "").replace(PREFIX, ""), input: block.input ?? {} };
      calls.push(call);
      results.push({ type: "tool_result", tool_use_id: block.id, content: answer(testCase, call) });
    }
    if (body.stop_reason !== "tool_use") break;
    messages.push({ role: "user", content: results });
  }
  return { calls, text, usage };
}

function readRows(file: string): Row[] {
  return readFileSync(file, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

/** `passed/samples` per case. */
export function tally(rows: Row[]): Map<string, { failed: number; samples: number }> {
  const byCase = new Map<string, { failed: number; samples: number }>();
  for (const row of rows) {
    const entry = byCase.get(row.case) ?? { failed: 0, samples: 0 };
    entry.samples++;
    if (row.failures.length) entry.failed++;
    byCase.set(row.case, entry);
  }
  return byCase;
}

/** The Markdown summary a person reads: each case's result, its baseline and the first failures. */
export function summary(rows: Row[], model: string, baseline?: Row[]): string {
  const now = tally(rows);
  const before = baseline ? tally(baseline) : undefined;
  const cell = (entry?: { failed: number; samples: number }) =>
    entry ? `${entry.samples - entry.failed}/${entry.samples} ${casePassed(entry.failed, entry.samples) ? "pass" : "FAIL"}` : "–";
  const lines = [
    `# MCP tool use: ${model}`,
    "",
    `| Case | Result |${before ? " Baseline |" : ""}`,
    `| --- | --- |${before ? " --- |" : ""}`,
  ];
  for (const [id, entry] of now) {
    lines.push(`| ${id} | ${cell(entry)} |${before ? ` ${cell(before.get(id))} |` : ""}`);
  }
  const input = rows.reduce((sum, row) => sum + row.usage.input, 0);
  const output = rows.reduce((sum, row) => sum + row.usage.output, 0);
  lines.push("", `Tokens: ${input} input, ${output} output.`);
  const failing = rows.filter((row) => row.failures.length);
  if (failing.length) {
    lines.push("", "## Failures", "");
    for (const row of failing) lines.push(`- ${row.case} #${row.sample}: ${row.failures.join("; ")}`);
  }
  return `${lines.join("\n")}\n`;
}

async function main() {
  loadEnv();
  const options = parseArgs(process.argv.slice(2));
  if (!process.env.ANTHROPIC_API_KEY) throw new Error("ANTHROPIC_API_KEY is not set (repo root .env)");
  const cases = options.cases.length ? CASES.filter((c) => options.cases.includes(c.id)) : CASES;
  const unknown = options.cases.filter((id) => !CASES.some((c) => c.id === id));
  if (unknown.length) throw new Error(`unknown case ${unknown.join(", ")}`);
  const manifest = loadManifest(options.manifest);
  const agentsMd = readFileSync(join(ROOT, "AGENTS.md"), "utf8");
  const tools = anthropicTools(manifest);
  mkdirSync(options.out, { recursive: true });
  const rows: Row[] = [];
  for (const testCase of cases) {
    const system = systemPrompt(manifest, testCase, agentsMd);
    const runs = Array.from({ length: options.samples }, (_, index) =>
      sample(options, testCase, system, tools).then((result) => ({
        case: testCase.id,
        sample: index + 1,
        model: options.model,
        failures: grade(result, testCase.checks),
        ...result,
      })),
    );
    for (const row of await Promise.all(runs)) {
      rows.push(row);
      console.log(`${row.case} #${row.sample}: ${row.failures.length ? row.failures.join("; ") : "pass"}`);
    }
  }
  writeFileSync(join(options.out, "results.jsonl"), rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
  const text = summary(rows, options.model, options.baseline ? readRows(options.baseline) : undefined);
  writeFileSync(join(options.out, "summary.md"), text);
  console.log(`\n${text}`);
}

if (import.meta.main) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
