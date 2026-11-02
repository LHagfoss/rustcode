// A fake `rustcode --acp` engine with canned traffic, for tests and for
// running the UI without an engine (`bun run dev:mock`, or `?transport=mock`).
// It speaks the same JSON-RPC lines as the real engine, including the
// harness contract in docs/harnesses.md, so the UI works before that lands.

import { arr, type Json, type JsonObject, obj, str } from "./jsonrpc";
import type { Transport, TransportEvent } from "./transport";
import { BUILTIN_CAPABILITIES, type HarnessCapabilities, type HarnessSnapshot } from "./types";

export interface MockOptions {
  /** `default`: onboarded with history. `onboarding`: first run. `empty`: no threads. */
  scenario?: string;
  /** Milliseconds between streamed chunks; 0 for tests. */
  delayMs?: number;
}

interface MockThread {
  id: string;
  cwd: string;
  title?: string;
  updatedAt: number;
  harness: string;
  model: string;
  effort: string;
  history: JsonObject[];
}

const RUSTCODE_MODELS = [
  { value: "chatgpt/gpt-6-luna", name: "chatgpt/gpt-6-luna", description: "gpt-6-luna" },
  { value: "chatgpt/gpt-6-astra", name: "chatgpt/gpt-6-astra", description: "gpt-6-astra" },
  { value: "claude-sonnet-5", name: "claude-sonnet-5", description: "claude-sonnet-5" },
];
const EFFORTS = ["low", "medium", "high", "xhigh"];

const CODEX_CAPS: HarnessCapabilities = {
  ...BUILTIN_CAPABILITIES,
  steer: "live",
  rewind: false,
  permission_modes: [],
  subagents: false,
  smart_ui: false,
  rustcode_extensions: false,
};
const ACP_CAPS: HarnessCapabilities = {
  ...CODEX_CAPS,
  steer: "none",
  models: "static",
  effort: false,
  resume: false,
  permission_modes: [],
};

function initialHarnesses(onboarded: boolean): HarnessSnapshot {
  return {
    harnesses: [
      {
        id: "rustcode",
        name: "RustCode",
        kind: "builtin",
        builtin: true,
        enabled: true,
        command: [],
        installed: true,
        auth: "ok",
        capabilities: BUILTIN_CAPABILITIES,
        models: RUSTCODE_MODELS.map((m) => ({
          id: m.value,
          name: m.name,
          efforts: EFFORTS,
          default_effort: "medium",
        })),
        error: null,
      },
      {
        id: "codex",
        name: "Codex",
        kind: "codex",
        builtin: false,
        enabled: onboarded,
        command: ["codex", "app-server"],
        installed: true,
        version: "0.162.1",
        auth: "ok",
        login_command: ["codex", "login"],
        install_hint: "brew install codex",
        capabilities: CODEX_CAPS,
        models: [],
        error: null,
      },
      {
        id: "claude-code",
        name: "Claude Code",
        kind: "acp",
        builtin: false,
        enabled: false,
        command: ["claude-agent-acp"],
        installed: true,
        version: "2.4.0",
        auth: "needs_login",
        login_command: ["claude", "/login"],
        install_hint: "npm install -g @zed-industries/claude-agent-acp",
        capabilities: ACP_CAPS,
        models: [],
        error: null,
      },
      {
        id: "gemini",
        name: "Gemini CLI",
        kind: "acp",
        builtin: false,
        enabled: false,
        command: ["gemini", "--experimental-acp"],
        installed: false,
        auth: "unknown",
        login_command: ["gemini"],
        install_hint: "brew install gemini-cli",
        capabilities: ACP_CAPS,
        models: [],
        error: null,
      },
      {
        id: "opencode",
        name: "OpenCode",
        kind: "acp",
        builtin: false,
        enabled: false,
        command: ["opencode", "acp"],
        installed: true,
        version: "1.9.2",
        auth: "unknown",
        login_command: ["opencode", "auth", "login"],
        install_hint: "brew install opencode",
        capabilities: ACP_CAPS,
        models: [],
        error: null,
      },
    ],
    default: { harness: "rustcode", model: "chatgpt/gpt-6-luna", effort: "medium" },
    onboarded,
  };
}

const HARNESS_MODELS: Record<
  string,
  { id: string; name: string; efforts: string[]; default_effort?: string }[]
> = {
  codex: [
    { id: "gpt-6-luna", name: "gpt-6-luna", efforts: ["low", "medium", "high"], default_effort: "medium" },
    { id: "gpt-6-astra", name: "gpt-6-astra", efforts: ["low", "medium", "high"], default_effort: "medium" },
  ],
  "claude-code": [
    { id: "sonnet", name: "Claude Sonnet", efforts: [] },
    { id: "opus", name: "Claude Opus", efforts: [] },
  ],
  opencode: [{ id: "default", name: "Default", efforts: [] }],
};

const now = Date.now();
const minutes = (n: number) => now - n * 60_000;

function update(sessionId: string, body: JsonObject): JsonObject {
  return { sessionId, update: body };
}

const text = (t: string) => ({ type: "text", text: t });

function seedThreads(): MockThread[] {
  const a = "mock-thread-1";
  const b = "mock-thread-2";
  const c = "mock-thread-3";
  const turn = (elapsedMs: number) => ({
    _meta: {
      rustcode: {
        turn: { id: "t1", elapsedMs, outcome: "completed", endedAt: new Date(minutes(5)).toISOString() },
      },
    },
  });
  return [
    {
      id: a,
      cwd: "/Users/you/code/rustcode",
      title: "Fix the composer focus ring",
      updatedAt: minutes(5),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      history: [
        update(a, {
          sessionUpdate: "user_message_chunk",
          content: text("The composer loses its focus ring after sending. Can you fix it?"),
        }),
        update(a, {
          sessionUpdate: "agent_thought_chunk",
          content: text("**Finding the composer**\nLooking for where focus is restored after submit."),
          _meta: { rustcode: { thoughtSource: "summary", thoughtMs: 1800 } },
        }),
        update(a, {
          sessionUpdate: "tool_call",
          toolCallId: "call-1",
          title: "rg",
          kind: "search",
          rawInput: { pattern: "focusComposer", path: "src", summary: "Find where the composer takes focus" },
        }),
        update(a, {
          sessionUpdate: "tool_call_update",
          toolCallId: "call-1",
          status: "completed",
          rawOutput: {
            content: "src/ui/Composer.tsx:42:  focusComposer();\nsrc/ui/App.tsx:88:  focusComposer();",
          },
        }),
        update(a, {
          sessionUpdate: "tool_call",
          toolCallId: "call-2",
          title: "edit src/ui/Composer.tsx",
          kind: "edit",
          rawInput: { path: "src/ui/Composer.tsx", summary: "Restore focus after the draft clears" },
        }),
        update(a, {
          sessionUpdate: "tool_call_update",
          toolCallId: "call-2",
          status: "completed",
          content: [
            {
              type: "diff",
              path: "src/ui/Composer.tsx",
              oldText: '  setDraft("");\n  submit(text);',
              newText:
                '  setDraft("");\n  submit(text);\n  requestAnimationFrame(() => input.current?.focus());',
              _meta: { rustcode: { oldStart: 40, newStart: 40 } },
            },
          ],
        }),
        update(a, {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "Fixed. The draft cleared before the textarea re-rendered, so focus went to `body`. It now refocuses on the next frame:\n\n```tsx\nrequestAnimationFrame(() => input.current?.focus());\n```\n\n- Sending with **⏎** keeps the ring\n- Escape still blurs it",
          ),
          ...turn(14_000),
        }),
      ],
    },
    {
      id: b,
      cwd: "/Users/you/code/rustcode",
      title: "Explain the sidecar restart backoff",
      updatedAt: minutes(80),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "low",
      history: [
        update(b, {
          sessionUpdate: "user_message_chunk",
          content: text("How does the sidecar restart backoff work?"),
        }),
        update(b, {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "It doubles from 0.5 s up to 30 s, and resets after the engine stays up for a minute. See https://github.com/LHagfoss/rustcode/pull/2788 and the timer in https://docs.rs/tokio/latest/tokio/time/fn.sleep.html.",
          ),
          ...turn(4_000),
        }),
      ],
    },
    {
      id: c,
      cwd: "/Users/you/code/website",
      title: "Add a dark mode toggle",
      updatedAt: minutes(60 * 26),
      harness: "rustcode",
      model: "claude-sonnet-5",
      effort: "high",
      history: [
        update(c, {
          sessionUpdate: "user_message_chunk",
          content: text("Add a dark mode toggle to the header"),
        }),
        update(c, {
          sessionUpdate: "agent_message_chunk",
          content: text("Added `ThemeToggle` to the header; it follows the system until you pick one."),
          ...turn(31_000),
        }),
      ],
    },
  ];
}

const COMMANDS: Json[] = [
  { name: "compact", description: "Summarize the conversation to free context" },
  {
    name: "context",
    description: "Show what fills the context window",
    _meta: { rustcode: { report: true } },
  },
  {
    name: "review",
    description: "Review the working tree or a pull request",
    input: { hint: "what to review" },
  },
  { name: "model", description: "Switch the model", input: { hint: "model id" } },
  { name: "skills", description: "List the skills this thread can load" },
];

function configOptions(thread: MockThread, harnesses: HarnessSnapshot): Json[] {
  const harness = harnesses.harnesses.find((h) => h.id === thread.harness);
  const models =
    thread.harness === "rustcode"
      ? RUSTCODE_MODELS
      : (HARNESS_MODELS[thread.harness] ?? []).map((m) => ({
          value: m.id,
          name: m.name,
          description: m.name,
        }));
  const options: Json[] = [
    {
      id: "model",
      name: "Model",
      category: "model",
      type: "select",
      currentValue: thread.model,
      options: models,
    },
  ];
  if (harness?.capabilities.effort !== false) {
    options.push({
      id: "effort",
      name: "Effort",
      category: "thought_level",
      type: "select",
      currentValue: thread.effort,
      options: EFFORTS.map((e) => ({ value: e, name: e })),
    });
  }
  return options;
}

export class MockEngine {
  private threads: MockThread[];
  private harnesses: HarnessSnapshot;
  private running = new Map<string, { cancelled: boolean; steers: string[] }>();
  private nextRpcId = 1000;
  private waiting = new Map<number, (result: JsonObject) => void>();
  private counter = 0;

  constructor(
    private readonly emit: (line: string) => void,
    private readonly options: MockOptions = {},
  ) {
    const scenario = options.scenario ?? "default";
    this.threads = scenario === "default" ? seedThreads() : [];
    this.harnesses = initialHarnesses(scenario !== "onboarding");
  }

  private get delay() {
    return this.options.delayMs ?? 40;
  }

  private sleep(ms = this.delay): Promise<void> {
    return ms > 0 ? new Promise((r) => setTimeout(r, ms)) : Promise.resolve();
  }

  private send(message: JsonObject) {
    this.emit(JSON.stringify({ jsonrpc: "2.0", ...message }));
  }

  private notify(params: JsonObject) {
    this.send({ method: "session/update", params });
  }

  /** Ask the client something and wait for its answer. */
  private ask(method: string, params: JsonObject): Promise<JsonObject> {
    const id = this.nextRpcId++;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.send({ id, method, params });
    });
  }

  receive(line: string): void {
    let message: JsonObject;
    try {
      message = JSON.parse(line) as JsonObject;
    } catch {
      return;
    }
    const method = str(message.method);
    const id = message.id as number | string | undefined;
    if (!method) {
      if (typeof id === "number") {
        const waiter = this.waiting.get(id);
        this.waiting.delete(id);
        waiter?.(obj(message.result) ?? {});
      }
      return;
    }
    const params = obj(message.params) ?? {};
    void this.handle(method, params).then(
      (result) => {
        if (id !== undefined) this.send({ id, result });
      },
      (error: Error & { code?: number }) => {
        if (id !== undefined)
          this.send({ id, error: { code: error.code ?? -32603, message: error.message } });
      },
    );
  }

  private thread(id: unknown): MockThread {
    const thread = this.threads.find((t) => t.id === id);
    if (!thread) throw Object.assign(new Error("Unknown session"), { code: -32602 });
    return thread;
  }

  private snapshot(): Json {
    return this.harnesses as unknown as Json;
  }

  private async handle(method: string, params: JsonObject): Promise<Json> {
    switch (method) {
      case "initialize":
        return {
          protocolVersion: 1,
          agentCapabilities: {
            loadSession: true,
            promptCapabilities: { image: true, audio: false, embeddedContext: false },
            sessionCapabilities: { list: {}, close: {} },
            _meta: { rustcode: { steer: 1, agents: 2, rewind: 1, pullRequests: 3 } },
          },
          authMethods: [],
        };
      case "session/list":
        return {
          sessions: this.threads
            .filter((t) => t.history.length > 0)
            .map((t) => ({
              sessionId: t.id,
              cwd: t.cwd,
              title: t.title ?? null,
              updatedAt: new Date(t.updatedAt).toISOString(),
              _meta: { model: t.model, rustcode: { harness: t.harness } },
            })),
        };
      case "session/new": {
        const meta = obj(obj(params._meta)?.rustcode);
        const harness = str(meta?.harness) ?? this.harnesses.default.harness;
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const thread: MockThread = {
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          cwd: str(params.cwd) ?? "/",
          updatedAt: Date.now(),
          harness,
          model:
            str(meta?.model) ??
            (harness === this.harnesses.default.harness ? this.harnesses.default.model : null) ??
            (harness === "rustcode" ? "chatgpt/gpt-6-luna" : (HARNESS_MODELS[harness]?.[0]?.id ?? "default")),
          effort: str(meta?.effort) ?? this.harnesses.default.effort ?? "medium",
          history: [],
        };
        this.threads.unshift(thread);
        queueMicrotask(() =>
          this.notify(
            update(thread.id, { sessionUpdate: "available_commands_update", availableCommands: COMMANDS }),
          ),
        );
        return {
          sessionId: thread.id,
          configOptions: configOptions(thread, this.harnesses),
          _meta: { rustcode: { harness: { id: harness, capabilities: h.capabilities as unknown as Json } } },
        };
      }
      case "session/load": {
        const thread = this.thread(params.sessionId);
        for (const line of thread.history) this.notify(line);
        this.notify(
          update(thread.id, { sessionUpdate: "available_commands_update", availableCommands: COMMANDS }),
        );
        this.notify(
          update(thread.id, {
            sessionUpdate: "usage_update",
            used: 18_400,
            size: 272_000,
            _meta: { rustcode: { estimated: true, inputTokens: 90_000, outputTokens: 3_200 } },
          }),
        );
        const h = this.harnesses.harnesses.find((x) => x.id === thread.harness);
        return {
          configOptions: configOptions(thread, this.harnesses),
          _meta: {
            rustcode: { harness: { id: thread.harness, capabilities: (h?.capabilities ?? {}) as Json } },
          },
        };
      }
      case "session/close":
        return {};
      case "session/set_config_option": {
        const thread = this.thread(params.sessionId);
        const value = str(params.value);
        if (params.configId === "model" && value) thread.model = value;
        if (params.configId === "effort" && value) thread.effort = value;
        return { configOptions: configOptions(thread, this.harnesses) };
      }
      case "session/cancel": {
        const run = this.running.get(str(params.sessionId) ?? "");
        if (run) run.cancelled = true;
        return null;
      }
      case "session/prompt":
        return this.prompt(params);
      case "_rustcode/steer": {
        const run = this.running.get(str(params.sessionId) ?? "");
        if (!run) return { accepted: false };
        run.steers.push(str(params.text) ?? "");
        return { accepted: true };
      }
      case "_rustcode/warm":
        return {};
      case "_rustcode/harnesses":
        return this.snapshot();
      case "_rustcode/harnesses/models": {
        const id = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) throw Object.assign(new Error(`Unknown harness ${id}`), { code: -32602 });
        await this.sleep(this.delay * 6);
        if (!h.installed) throw new Error(`${h.name} is not installed (${h.command[0]} not on PATH).`);
        if (h.auth === "needs_login") throw new Error(`${h.name} needs you to sign in first.`);
        h.models = h.builtin ? h.models : (HARNESS_MODELS[id] ?? []);
        if (h.auth === "unknown") h.auth = "ok";
        return { models: h.models as unknown as Json };
      }
      case "_rustcode/harnesses/set": {
        const id = str(params.id) ?? "";
        let h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) {
          h = {
            id,
            name: str(params.name) ?? id,
            kind: "acp",
            builtin: false,
            enabled: true,
            command: arr(params.command).map(String),
            installed: true,
            auth: "unknown",
            capabilities: ACP_CAPS,
            models: [],
            error: null,
          };
          this.harnesses.harnesses.push(h);
        }
        if (typeof params.enabled === "boolean") h.enabled = params.enabled;
        if (Array.isArray(params.command)) h.command = params.command.map(String);
        return this.snapshot();
      }
      case "_rustcode/harnesses/remove": {
        this.harnesses.harnesses = this.harnesses.harnesses.filter(
          (h) =>
            h.id !== params.id || ["rustcode", "codex", "claude-code", "gemini", "opencode"].includes(h.id),
        );
        return this.snapshot();
      }
      case "_rustcode/harnesses/default":
        this.harnesses.default = {
          harness: str(params.harness) ?? "rustcode",
          model: str(params.model) ?? null,
          effort: str(params.effort) ?? null,
        };
        return this.snapshot();
      case "_rustcode/onboarding/complete":
        this.harnesses.onboarded = true;
        return this.snapshot();
      case "_rustcode/capabilities":
        return {
          configPath: "~/.config/rustcode/config.toml",
          skills: [
            {
              name: "release-automation",
              description: "Cut a release with scripts/release.sh",
              path: "~/.agents/skills/release-automation",
              source: "universal",
              enabled: true,
            },
            {
              name: "frontend-design",
              description: "Design guidance for web UIs",
              path: ".rustcode/skills/frontend-design",
              source: "workspace",
              enabled: false,
            },
          ],
          mcpServers: [
            {
              name: "github",
              transport: "http",
              target: "https://api.githubcopilot.com/mcp/",
              enabled: true,
              scope: "user",
              connection: "connected",
              tools: 41,
              error: null,
            },
            {
              name: "sqlite",
              transport: "stdio",
              target: "uvx mcp-server-sqlite",
              enabled: false,
              scope: "user",
              connection: "disabled",
              tools: 0,
              error: null,
            },
          ],
        };
      case "_rustcode/capabilities/set":
        return this.handle("_rustcode/capabilities", {});
      case "_rustcode/connections":
        return {
          github: {
            cliInstalled: true,
            accounts: [{ host: "github.com", login: "octocat", active: true, valid: true, scopes: ["repo"] }],
            pending: null,
            error: null,
          },
          accounts: [
            {
              provider: "openai",
              account: "a1",
              display: "you@example.com",
              method: "chat_gpt",
              active: true,
              expiresAt: null,
              needsSignIn: false,
            },
          ],
        };
      default:
        throw Object.assign(new Error("Method not found"), { code: -32601 });
    }
  }

  private async prompt(params: JsonObject): Promise<Json> {
    const thread = this.thread(params.sessionId);
    const prompt = arr(params.prompt)
      .map((b) => str(obj(b)?.text) ?? "")
      .join("");
    const sid = thread.id;
    const run = { cancelled: false, steers: [] as string[] };
    this.running.set(sid, run);
    const push = (body: JsonObject) => {
      const line = update(sid, body);
      thread.history.push(line);
      this.notify(line);
    };
    thread.history.push(update(sid, { sessionUpdate: "user_message_chunk", content: text(prompt) }));
    thread.updatedAt = Date.now();
    const started = Date.now();
    const done = (stopReason: string) => {
      this.running.delete(sid);
      return { stopReason, _meta: { rustcode: { unusedSteers: run.steers } } };
    };
    try {
      if (!thread.title) {
        thread.title = prompt.split("\n")[0]?.slice(0, 60) || "New thread";
        this.notify(update(sid, { sessionUpdate: "session_info_update", title: thread.title }));
      }
      for (const chunk of [
        "**Reading the request**\n",
        "Checking which files are involved ",
        "before editing.",
      ]) {
        if (run.cancelled) return done("cancelled");
        push({ sessionUpdate: "agent_thought_chunk", content: text(chunk) });
        await this.sleep();
      }
      push({
        sessionUpdate: "plan",
        entries: [
          { content: "Find the code", status: "completed", priority: "medium" },
          { content: "Make the change", status: "in_progress", priority: "medium" },
          { content: "Run the checks", status: "pending", priority: "low" },
        ],
      });
      if (/question/i.test(prompt)) {
        const answer = await this.ask("session/request_permission", {
          sessionId: sid,
          toolCall: {
            toolCallId: "q-1",
            title: "Which test runner?",
            rawInput: {
              header: "Tests",
              question: "Which test runner should I use?",
              options: ["bun test", "vitest"],
              descriptions: ["Built in, no dependency", "Familiar, needs a dependency"],
              multiple: false,
            },
          },
          options: [
            { optionId: "question_option_0", name: "bun test", kind: "allow_once" },
            { optionId: "question_option_1", name: "vitest", kind: "allow_once" },
            { optionId: "question_cancel", name: "Cancel", kind: "reject_once" },
          ],
          _meta: {
            rustcode: { kind: "question", version: 2, questionId: "q-1", answerKinds: ["options", "text"] },
          },
        });
        if (
          str(obj(answer.outcome)?.outcome) === "cancelled" ||
          obj(answer.outcome)?.optionId === "question_cancel"
        ) {
          return done("cancelled");
        }
      }
      const callId = `call-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: "bash",
        kind: "execute",
        status: "in_progress",
        rawInput: { command: "bun test src", summary: "Run the unit tests" },
      });
      for (const line of ["bun test v1.3.14\n", "src/state/reducer.test.ts:\n", "✓ 24 pass, 0 fail\n"]) {
        if (run.cancelled) return done("cancelled");
        await this.sleep();
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          _meta: { rustcode: { outputDelta: line } },
        });
      }
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: callId,
        status: "completed",
        rawOutput: {
          content: "bun test v1.3.14\nsrc/state/reducer.test.ts:\n✓ 24 pass, 0 fail\n",
          exitCode: 0,
        },
      });
      const editId = `call-${++this.counter}`;
      const diff = {
        type: "diff",
        path: "src/app.ts",
        oldText: 'const greeting = "hello";\nexport default greeting;',
        newText: 'const greeting = "hello, world";\nexport default greeting;',
        _meta: { rustcode: { oldStart: 1, newStart: 1 } },
      };
      push({
        sessionUpdate: "tool_call",
        toolCallId: editId,
        title: "edit src/app.ts",
        kind: "edit",
        status: "pending",
        rawInput: { path: "src/app.ts", summary: "Update the greeting" },
      });
      if (!/yolo/i.test(prompt)) {
        const answer = await this.ask("session/request_permission", {
          sessionId: sid,
          toolCall: {
            toolCallId: editId,
            title: "edit src/app.ts",
            kind: "edit",
            content: [diff],
            rawInput: { path: "src/app.ts" },
          },
          options: [
            { optionId: "allow", name: "Allow", kind: "allow_once" },
            { optionId: "always", name: "Always allow edits", kind: "allow_always" },
            { optionId: "reject", name: "Reject", kind: "reject_once" },
          ],
        });
        const outcome = obj(answer.outcome);
        if (str(outcome?.outcome) === "cancelled") return done("cancelled");
        if (outcome?.optionId === "reject") {
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: editId,
            status: "failed",
            rawOutput: { content: "Rejected by the user." },
          });
        } else {
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: editId,
            status: "completed",
            content: [diff],
          });
        }
      } else {
        push({ sessionUpdate: "tool_call_update", toolCallId: editId, status: "completed", content: [diff] });
      }
      const reply = [
        "Done. ",
        "I updated the greeting in `src/app.ts` ",
        "and the tests pass.\n\n",
        "```ts\n",
        'const greeting = "hello, world";\n',
        "export default greeting;\n",
        "```\n\n",
        "| Check | Result |\n|---|---|\n| tests | 24 pass |\n| lint | clean |\n",
      ];
      for (const chunk of reply) {
        if (run.cancelled) return done("cancelled");
        await this.sleep();
        push({ sessionUpdate: "agent_message_chunk", content: text(chunk) });
      }
      for (const steer of run.steers.splice(0)) {
        push({ sessionUpdate: "agent_message_chunk", content: text(`\n\nNoted: “${steer}”.`) });
      }
      push({
        sessionUpdate: "plan",
        entries: [
          { content: "Find the code", status: "completed" },
          { content: "Make the change", status: "completed" },
          { content: "Run the checks", status: "completed" },
        ],
      });
      this.notify(
        update(sid, {
          sessionUpdate: "usage_update",
          used: 24_000 + Math.round((Date.now() - started) / 10),
          size: 272_000,
          _meta: { rustcode: { estimated: false, inputTokens: 120_000, outputTokens: 4_100 } },
        }),
      );
      return done("end_turn");
    } finally {
      this.running.delete(sid);
    }
  }
}

const svgIcon = (fill: string, letter: string) =>
  `data:image/svg+xml;base64,${btoa(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="4" fill="${fill}"/><text x="8" y="12" font-size="10" font-family="sans-serif" text-anchor="middle" fill="#fff">${letter}</text></svg>`,
  )}`;

/** Canned link metadata, as the shell's `link_meta` answers. */
export function mockLinkMeta(url: string): JsonObject | null {
  const host = (() => {
    try {
      return new URL(url).host;
    } catch {
      return "";
    }
  })();
  if (host.endsWith("github.com")) {
    const path = new URL(url).pathname.split("/").filter(Boolean);
    return {
      title:
        path.length >= 4
          ? `${path[2] === "pull" ? "Pull request" : "Issue"} #${path[3]} · ${path[0]}/${path[1]}`
          : `${path.join("/") || "GitHub"}`,
      siteName: "GitHub",
      color: "#1e2327",
      icon: svgIcon("#1e2327", "G"),
    };
  }
  if (host.endsWith("docs.rs"))
    return { title: "tokio - Rust", color: "#e57300", icon: svgIcon("#e57300", "D") };
  if (host.endsWith("example.com")) return null;
  return { title: host, icon: svgIcon("#435c6e", host.charAt(0).toUpperCase() || "?") };
}

export class MockTransport implements Transport {
  readonly name = "mock";
  private engine: MockEngine | null = null;
  private onEvent: ((event: TransportEvent) => void) | null = null;

  constructor(private readonly options: MockOptions = {}) {}

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    this.engine = new MockEngine(
      (line) => queueMicrotask(() => onEvent({ type: "line", line })),
      this.options,
    );
    onEvent({ type: "log", line: "mock engine started" });
    onEvent({ type: "open", engine: "mock" });
  }

  async restart(): Promise<void> {
    const onEvent = this.onEvent;
    if (!onEvent) return;
    onEvent({ type: "closed", reason: "Restarting the engine." });
    await this.start(onEvent);
  }

  send(line: string): void {
    this.engine?.receive(line);
  }

  async call(command: string, args: Record<string, unknown>): Promise<unknown> {
    if (command === "link_meta") return mockLinkMeta(String(args.url));
    throw new Error(`The mock does not run ${command}.`);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }
}
