// Does an agent pick the right thread tool? Each case is a prompt a user
// would type; the agent gets the orchestration tools the engine's MCP server
// lists (with their real descriptions) as stand-ins, and nothing else. A case
// passes when the agent called every tool it must and none it must not.
//
//   bun evals/thread-tools/run.ts [--model <model>] [--case <id>] [--samples <n>] [--concurrency <n>]
//
// No app, no engine socket, no real effects: every call is answered from a
// canned result and appended to a log. See README.md.

import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../..");
const CASES = join(HERE, "cases.jsonl");
const EXTENSION = join(HERE, "extension.ts");
const RUNS = join(HERE, "runs");
const DEFAULT_MODEL = "deepseek/deepseek-flash:high";
const TOOL_TIMEOUT_MS = 90_000;
const LIST_TIMEOUT_MS = 30_000;

/** What each tool answers with, so a call can be followed by another. */
const MOCKS: Record<string, unknown> = {
  list_sessions: {
    sessions: [
      { session_id: "t1", title: "Add the login form", working: true, status: "running" },
      { session_id: "t2", title: "Fix the composer focus ring", working: false, status: "idle" },
    ],
  },
  list_harnesses: {
    current: { harness: "rustcode" },
    harnesses: [
      {
        id: "codex",
        name: "Codex",
        available: true,
        models: [
          { id: "gpt-5-codex", name: "GPT-5 Codex", efforts: ["low", "medium", "high"] },
          { id: "gpt-5-mini", name: "GPT-5 mini", efforts: ["low", "high"] },
        ],
      },
    ],
  },
  search_sessions: {
    sessions: [{ session_id: "t1", title: "Add the login form", status: "idle", snippet: "…the login form…" }],
  },
  list_worktrees: {
    worktrees: [{ session_id: "t1", title: "Add the login form", branch: "feat/login-form", path: "/wt/login" }],
  },
  list_pending_requests: {
    requests: [{ session_id: "t1", title: "Add the login form", kind: "approval", asks: "Run: cargo test" }],
  },
  transfer_session: { session_id: "t1", harness: "codex", moved: true },
  list_projects: {
    projects: [{ path: "/Users/you/code/rustcode", name: "rustcode", threads: 3, working: false }],
  },
  list_pull_requests: {
    pull_requests: [
      {
        number: 42,
        title: "Add the login form",
        state: "open",
        branch: "feat/login-form",
        url: "https://github.com/example/rustcode/pull/42",
      },
    ],
  },
  read_session: { title: "Add the login form", status: "idle", turns: [] },
  update_session: { session_id: "self", title: "Renamed thread", pinned: false, status: "settled" },
  interrupt_session: { session_id: "t1", interrupted: true },
  configure_session: { session_id: "self", harness: "rustcode", access: "auto", effort: "high" },
  start_session: { session_id: "thread-3", link_id: "self/1", harness: "codex" },
  message_session: { link_id: "self/1", receipt: "started", session_id: "t1" },
  wait_session: { links: [{ link_id: "self/1", status: "done" }] },
  fan_out: { links: [] },
};

interface Case {
  id: string;
  kind: "bad" | "guard" | "control";
  prompt: string;
  must: string[];
  /** One of these is enough: several tools answer the same question. */
  oneOf: string[];
  mustNot: string[];
}

interface Call {
  tool: string;
  args: Record<string, unknown>;
}

interface Outcome {
  ok: boolean;
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

function run(
  command: string,
  args: string[],
  options: { env?: Record<string, string>; cwd?: string; stdin?: string; timeoutMs: number },
): Promise<Outcome> {
  return new Promise((done) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, options.timeoutMs);
    child.stdout.on("data", (data) => (stdout += String(data)));
    child.stderr.on("data", (data) => (stderr += String(data)));
    child.on("error", (error) => {
      clearTimeout(timer);
      done({ ok: false, code: null, stdout, stderr: `${stderr}${error.message}`, timedOut });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      done({ ok: code === 0, code, stdout, stderr, timedOut });
    });
    child.stdin.end(options.stdin ?? "");
  });
}

function parseArgs(argv: string[]) {
  const options = {
    model: DEFAULT_MODEL,
    cases: [] as string[],
    samples: 1,
    concurrency: 3,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = argv[i + 1];
    if (arg === "--model" && value) {
      options.model = value;
      i++;
    } else if (arg === "--case" && value) {
      options.cases.push(...value.split(",").filter(Boolean));
      i++;
    } else if (arg === "--samples" && value) {
      options.samples = Math.max(1, Number(value) || 1);
      i++;
    } else if (arg === "--concurrency" && value) {
      options.concurrency = Math.max(1, Number(value) || 1);
      i++;
    } else if (arg === "--help" || arg === "-h") {
      console.log("usage: bun evals/thread-tools/run.ts [--model <model>] [--case <id>] [--samples <n>] [--concurrency <n>]");
      process.exit(0);
    } else {
      console.error(`unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  return options;
}

const binary = () => {
  const given = process.env.RUSTCODE_BIN ?? join(ROOT, "target/debug/rustcode");
  return isAbsolute(given) ? given : resolve(ROOT, given);
};

/** The orchestration tools the engine's MCP server lists, as the harness sees them. */
async function toolList(scratch: string): Promise<{ name: string; description?: string; inputSchema?: unknown }[]> {
  const bin = binary();
  if (!existsSync(bin)) {
    throw new Error(`no rustcode binary at ${bin}; build it with cargo build -p rustcode-tui --bin rustcode or set RUSTCODE_BIN`);
  }
  const session = [
    {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "thread-tools-eval", version: "0" },
      },
    },
    { jsonrpc: "2.0", method: "notifications/initialized", params: {} },
    { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
  ]
    .map((message) => `${JSON.stringify(message)}\n`)
    .join("");
  const outcome = await run(
    bin,
    ["mcp-orchestrator", "--session", "s", "--socket", "/nonexistent"],
    {
      cwd: scratch,
      env: { RUSTCODE_ORCHESTRATOR_TOKEN: "x", RUSTCODE_CONFIG_DIR: scratch },
      stdin: session,
      timeoutMs: LIST_TIMEOUT_MS,
    },
  );
  for (const line of outcome.stdout.split("\n")) {
    if (!line.trim()) continue;
    let message: { id?: number; result?: { tools?: unknown[] }; error?: { message?: string } };
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (message.id !== 2) continue;
    if (message.error) throw new Error(`tools/list failed: ${message.error.message ?? "unknown error"}`);
    const tools = (message.result?.tools ?? []) as { name?: string }[];
    return tools.flatMap((tool) => (tool.name ? [tool as { name: string }] : []));
  }
  throw new Error(
    `tools/list did not answer (exit ${outcome.code}${outcome.timedOut ? ", timed out" : ""}): ${outcome.stderr.trim()}`,
  );
}

function readCases(): Case[] {
  return readFileSync(CASES, "utf8")
    .split("\n")
    .filter((line) => line.trim())
    .map((line, i) => {
      const parsed = JSON.parse(line) as Partial<Case>;
      const where = `cases.jsonl line ${i + 1}`;
      if (!parsed.id || !parsed.prompt) throw new Error(`${where}: id and prompt are required`);
      if (parsed.kind !== "bad" && parsed.kind !== "guard" && parsed.kind !== "control") {
        throw new Error(`${where}: kind must be bad, guard or control`);
      }
      return {
        id: parsed.id,
        kind: parsed.kind,
        prompt: parsed.prompt,
        must: parsed.must ?? [],
        oneOf: parsed.oneOf ?? [],
        mustNot: parsed.mustNot ?? [],
      };
    });
}

function readCalls(log: string): Call[] {
  if (!existsSync(log)) return [];
  return readFileSync(log, "utf8")
    .split("\n")
    .filter((line) => line.trim())
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as Call];
      } catch {
        return [];
      }
    });
}

const passing = (c: Case, called: string[]) =>
  c.must.every((tool) => called.includes(tool)) &&
  (!c.oneOf.length || c.oneOf.some((tool) => called.includes(tool))) &&
  !c.mustNot.some((tool) => called.includes(tool));

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const all = readCases();
  const cases = options.cases.length ? all.filter((c) => options.cases.includes(c.id)) : all;
  if (!cases.length) throw new Error("no cases to run");
  const unknown = options.cases.filter((id) => !all.some((c) => c.id === id));
  if (unknown.length) throw new Error(`no such case: ${unknown.join(", ")}`);

  const scratch = mkdtempSync(join(tmpdir(), "thread-tools-"));
  const tools = await toolList(scratch);
  if (!tools.length) throw new Error("the MCP server listed no tools");
  const toolset = join(scratch, "tools.json");
  writeFileSync(toolset, JSON.stringify(tools, null, 2));
  const names = tools.map((tool) => `mcp__rustcode__${tool.name}`);

  mkdirSync(RUNS, { recursive: true });
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const results = join(RUNS, `${stamp}.jsonl`);
  const lines: string[] = [];

  const queue = cases.flatMap((c) => Array.from({ length: options.samples }, (_, sample) => ({ c, sample })));
  let passed = 0;
  let failed = 0;
  let index = 0;

  const worker = async () => {
    for (;;) {
      const item = queue[index++];
      if (!item) return;
      const { c, sample } = item;
      const log = join(scratch, `${c.id}-${sample}.jsonl`);
      const started = Date.now();
      const outcome = await run(
        "pi",
        [
          "-p",
          "--no-session",
          "--no-extensions",
          "--no-tools",
          "--no-skills",
          "--no-context-files",
          // `--no-tools` also hides extension tools, so name only the stand-ins.
          "--tools",
          names.join(","),
          "--extension",
          EXTENSION,
          "--model",
          options.model,
          c.prompt,
        ],
        {
          cwd: scratch,
          env: {
            EVAL_TOOLS: toolset,
            EVAL_MOCKS: JSON.stringify(MOCKS),
            EVAL_LOG: log,
          },
          timeoutMs: TOOL_TIMEOUT_MS,
        },
      );
      const calls = readCalls(log);
      const called = calls.map((call) => call.tool);
      const ok = !outcome.timedOut && outcome.ok && passing(c, called);
      if (ok) passed++;
      else failed++;

      const label = `${c.id}${options.samples > 1 ? `#${sample + 1}` : ""}`;
      const detail = called.length ? called.join(", ") : "no tool";
      console.log(
        `${ok ? "PASS" : "FAIL"}  ${label} (${c.kind}) — called: ${detail}` +
          (outcome.timedOut ? " [timed out]" : outcome.ok ? "" : ` [exit ${outcome.code}]`),
      );
      if (!ok) {
        if (c.must.length) console.log(`      must: ${c.must.join(", ")}`);
        if (c.oneOf.length) console.log(`      one of: ${c.oneOf.join(", ")}`);
        if (c.mustNot.length) console.log(`      must not: ${c.mustNot.join(", ")}`);
        const tail = (outcome.stderr || outcome.stdout).trim().split("\n").slice(-4).join("\n");
        if (tail) console.log(`      ${tail.replace(/\n/g, "\n      ")}`);
      }

      lines.push(
        JSON.stringify({
          case: c.id,
          kind: c.kind,
          sample,
          model: options.model,
          prompt: c.prompt,
          must: c.must,
          oneOf: c.oneOf,
          mustNot: c.mustNot,
          pass: ok,
          called,
          calls,
          exit: outcome.code,
          timedOut: outcome.timedOut,
          ms: Date.now() - started,
          reply: outcome.stdout.trim().slice(0, 2000),
          error: outcome.ok ? undefined : outcome.stderr.trim().slice(0, 2000),
        }),
      );
    }
  };

  await Promise.all(Array.from({ length: Math.min(options.concurrency, queue.length) }, worker));
  writeFileSync(results, `${lines.join("\n")}\n`);
  rmSync(scratch, { recursive: true, force: true });

  const total = passed + failed;
  console.log(`\n${passed}/${total} passed${failed ? `, ${failed} failed` : ""} (${options.model})`);
  console.log(`results: ${results}`);
  process.exit(failed ? 1 : 0);
}

main().catch((error) => {
  console.error(`thread-tools eval: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(2);
});
