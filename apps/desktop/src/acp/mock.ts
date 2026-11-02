// design-lint: allow-file (fake link colours in canned data)
// A fake `rustcode --acp` engine with canned traffic, for tests and for
// running the UI without an engine (`bun run dev:mock`, or `?transport=mock`).
// It speaks the same JSON-RPC lines as the real engine, including the
// harness contract in docs/harnesses.md, so the UI works before that lands.

import { BUILTIN_THEMES } from "../lib/themes";
import { arr, type Json, type JsonObject, obj, str } from "./jsonrpc";
import type { Transport, TransportEvent } from "./transport";
import { BUILTIN_CAPABILITIES, type HarnessCapabilities, type HarnessSnapshot } from "./types";

export interface MockOptions {
  /** `default`: onboarded with history. `onboarding`: first run. `empty`: no threads. */
  scenario?: string;
  /** Milliseconds between streamed chunks; 0 for tests. */
  delayMs?: number;
  /** Milliseconds the engine takes to start (`?boot=`), to see the slow-start toast. */
  bootMs?: number;
  /** Crash this many ms after the first start (`?crash=`); it comes back after `bootMs`, as the shell restarts it. */
  crashAfterMs?: number;
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
  worktree?: MockWorktree;
  status?: string;
  pinned?: boolean;
  checkpoints: MockCheckpoint[];
}

interface MockWorktree {
  path: string;
  branch: string;
  repo: string;
  base: string;
  created_at: string;
  dirty: boolean;
  ahead: number;
  behind: number;
}

interface MockCheckpoint {
  turn: number;
  prompt: string;
  created_at: string;
  files: { path: string; status: string; insertions: number; deletions: number; patch: string }[];
}

const COMPOSER_PATCH = `@@ -40,2 +40,3 @@
   setDraft("");
   submit(text);
+  requestAnimationFrame(() => input.current?.focus());`;

const APP_PATCH = `@@ -1,2 +1,2 @@
-const greeting = "hello";
+const greeting = "hello, world";
 export default greeting;`;

const LOGIN_PATCH = `@@ -0,0 +1,14 @@
+export function LoginForm({ onSubmit }: { onSubmit: (email: string) => void }) {
+  const [email, setEmail] = useState("");
+  return (
+    <form onSubmit={(e) => { e.preventDefault(); onSubmit(email); }}>
+      <label>
+        Email
+        <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
+      </label>
+      <button type="submit">Sign in</button>
+    </form>
+  );
+}
+
+export default LoginForm;`;

function slug(text: string): string {
  return (
    text
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 40) || "thread"
  );
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
      checkpoints: [
        {
          turn: 1,
          prompt: "The composer loses its focus ring after sending. Can you fix it?",
          created_at: new Date(minutes(6)).toISOString(),
          files: [
            {
              path: "src/ui/Composer.tsx",
              status: "modified",
              insertions: 1,
              deletions: 0,
              patch: COMPOSER_PATCH,
            },
          ],
        },
      ],
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
      checkpoints: [],
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
      status: "done",
      checkpoints: [],
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
    {
      id: "mock-thread-4",
      cwd: "/Users/you/.config/rustcode/worktrees/rustcode/feat-login-form",
      title: "Add the login form",
      updatedAt: minutes(30),
      harness: "codex",
      model: "gpt-6-astra",
      effort: "medium",
      status: "review",
      worktree: {
        path: "/Users/you/.config/rustcode/worktrees/rustcode/feat-login-form",
        branch: "feat/login-form",
        repo: "/Users/you/code/rustcode",
        base: "c1ace820",
        created_at: new Date(minutes(45)).toISOString(),
        dirty: true,
        ahead: 2,
        behind: 0,
      },
      checkpoints: [
        {
          turn: 1,
          prompt: "Add a login form with an email field",
          created_at: new Date(minutes(40)).toISOString(),
          files: [
            {
              path: "src/ui/LoginForm.tsx",
              status: "added",
              insertions: 14,
              deletions: 0,
              patch: LOGIN_PATCH,
            },
          ],
        },
      ],
      history: [
        update("mock-thread-4", {
          sessionUpdate: "user_message_chunk",
          content: text("Add a login form with an email field"),
        }),
        update("mock-thread-4", {
          sessionUpdate: "tool_call",
          toolCallId: "lf-1",
          title: "edit src/ui/LoginForm.tsx",
          kind: "edit",
          status: "completed",
          content: [
            { type: "diff", path: "src/ui/LoginForm.tsx", newText: "export function LoginForm() {}\n" },
          ],
        }),
        update("mock-thread-4", {
          sessionUpdate: "agent_message_chunk",
          content: text("Added `LoginForm` with an email field and a submit button, on `feat/login-form`."),
          _meta: { rustcode: { turn: { id: "t1", elapsedMs: 52_000, outcome: "completed" } } },
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
  private agents = new Map<string, JsonObject[]>();
  /** The client's `_meta.rustcode.orchestration` at initialize. */
  private orchestration = 0;
  private removedWorktrees = new Set<string>();
  /** The engine's themes: the app's built-ins plus one user theme, as `_rustcode/themes` lists them. */
  private themes: JsonObject[] = [
    ...BUILTIN_THEMES.map((t) => ({ ...t, tokens: { ...t.tokens } }) as unknown as JsonObject),
    {
      name: "graphite",
      description: "Graphite: lighter greys, round corners",
      source: "user",
      path: "~/.config/rustcode/themes/graphite.toml",
      mode: "dark",
      radius: "round",
      tokens: {
        ...BUILTIN_THEMES[0]!.tokens,
        bg: "#101012",
        sidebar: "#141416",
        surface: "#1a1a1d",
        surface_2: "#212125",
        surface_3: "#2a2a2f",
        accent: "#8b8bf5",
        focus: "#8b8bf5",
        selection: "#8b8bf54d",
      },
    },
  ];
  private activeTheme = "rustcode";

  private themesSnapshot(): JsonObject {
    return { active: this.activeTheme, themes: this.themes as unknown as Json };
  }

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

  /** The shell's thread actions, on the mock's own threads. */
  renameThread(id: string, title: string): string {
    const thread = this.thread(id);
    thread.title = title.split(/\s+/).join(" ").trim();
    return thread.title;
  }

  trashThread(id: string) {
    this.thread(id);
    this.threads = this.threads.filter((t) => t.id !== id);
  }

  private snapshot(): Json {
    return this.harnesses as unknown as Json;
  }

  private async handle(method: string, params: JsonObject): Promise<Json> {
    switch (method) {
      case "initialize": {
        const rc = obj(obj(obj(params.clientCapabilities)?._meta)?.rustcode);
        this.orchestration = typeof rc?.orchestration === "number" ? rc.orchestration : 0;
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
      }
      case "session/list":
        return {
          sessions: this.threads
            .filter((t) => t.history.length > 0)
            .map((t) => ({
              sessionId: t.id,
              cwd: t.cwd,
              title: t.title ?? null,
              updatedAt: new Date(t.updatedAt).toISOString(),
              _meta: {
                model: t.model,
                rustcode: {
                  harness: t.harness,
                  ...(t.status ? { status: t.status } : {}),
                  ...(t.pinned ? { pinned: true } : {}),
                  ...(t.worktree ? { worktree: t.worktree as unknown as Json } : {}),
                },
              },
            })),
        };
      case "session/new": {
        const meta = obj(obj(params._meta)?.rustcode);
        const harness = str(meta?.harness) ?? this.harnesses.default.harness;
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const wanted = obj(meta?.worktree);
        let worktree: MockWorktree | undefined;
        if (wanted) {
          const repo = str(wanted.repo) ?? str(params.cwd) ?? "/";
          worktree = this.makeWorktree(repo, str(wanted.branch), str(wanted.title));
        }
        const thread: MockThread = {
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          cwd: worktree?.path ?? str(params.cwd) ?? "/",
          worktree,
          checkpoints: [],
          updatedAt: Date.now(),
          harness,
          model:
            str(meta?.model) ??
            (harness === this.harnesses.default.harness ? this.harnesses.default.model : null) ??
            (harness === "rustcode" ? "chatgpt/gpt-6-luna" : (HARNESS_MODELS[harness]?.[0]?.id ?? "default")),
          // Like the real engine, the built-in harness takes its effort from config, not from `session/new`.
          effort:
            (harness === "rustcode" ? undefined : str(meta?.effort)) ??
            this.harnesses.default.effort ??
            "medium",
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
          _meta: {
            rustcode: {
              harness: { id: harness, capabilities: h.capabilities as unknown as Json },
              ...(worktree ? { worktree: worktree as unknown as Json } : {}),
            },
          },
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
      case "_rustcode/agents": {
        const sid = str(params.sessionId) ?? "";
        const list = this.agents.get(sid) ?? [];
        const action = str(params.action);
        if (action === "list") return { agents: list };
        const agent = list.find((a) => a.id === params.id);
        if (!agent) throw Object.assign(new Error(`unknown subagent: ${params.id}`), { code: -32602 });
        if (action === "history") {
          return {
            agent,
            updates: [
              { sessionUpdate: "user_message_chunk", content: text(String(agent.task)) },
              {
                sessionUpdate: "tool_call",
                toolCallId: `a${agent.id}-1`,
                title: "rg",
                kind: "search",
                status: "completed",
                rawInput: { pattern: "Composer", summary: "Find the composer" },
              },
              ...(agent.status === "completed"
                ? [{ sessionUpdate: "agent_message_chunk", content: text(String(agent.completion)) }]
                : []),
            ],
          };
        }
        if (action === "cancel") {
          agent.status = "cancelled";
          agent.finishedAtMs = Date.now();
          return { settled: true };
        }
        if (action === "send") return { queued: false };
        throw Object.assign(new Error("bad action"), { code: -32602 });
      }
      case "_rustcode/usage/limits":
        return {
          source: "codex",
          primary: { usedPercent: 42, windowMinutes: 300, resetsAt: Math.floor(Date.now() / 1000) + 7200 },
          secondary: {
            usedPercent: 71,
            windowMinutes: 10080,
            resetsAt: Math.floor(Date.now() / 1000) + 3 * 86400,
          },
        };
      case "_rustcode/pulls":
        return {
          list: {
            provider: "github",
            hasMore: false,
            pulls: [
              {
                number: 2788,
                title: "feat(desktop): Tauri desktop app on every harness",
                author: "LHagfoss",
                state: "open",
                draft: true,
                review: "review_required",
                checks: { passed: 5, failed: 0, pending: 1, skipped: 0 },
                headRef: "feat/desktop-tauri-harnesses",
                baseRef: "main",
                headSha: "58961fc5aa",
                updatedAt: new Date(minutes(20)).toISOString(),
                url: "https://github.com/LHagfoss/rustcode/pull/2788",
                additions: 9120,
                deletions: 12,
                changedFiles: 64,
              },
              {
                number: 2781,
                title: "fix(engine): keep the steer queue across a restart",
                author: "LHagfoss",
                state: "open",
                draft: false,
                review: "approved",
                checks: { passed: 6, failed: 1, pending: 0, skipped: 0 },
                headRef: "fix/engine-steer-restart",
                baseRef: "main",
                headSha: "a1b2c3d4",
                updatedAt: new Date(minutes(300)).toISOString(),
                url: "https://github.com/LHagfoss/rustcode/pull/2781",
                additions: 40,
                deletions: 9,
                changedFiles: 3,
              },
            ],
          },
        };
      case "_rustcode/pulls/view":
        return {
          pull: {
            number: params.number as number,
            title: "feat(desktop): Tauri desktop app on every harness",
            author: "LHagfoss",
            state: "open",
            draft: true,
            checks: { passed: 5, failed: 0, pending: 1, skipped: 0 },
            headRef: "feat/desktop-tauri-harnesses",
            baseRef: "main",
            headSha: "58961fc5aa",
            url: `https://github.com/LHagfoss/rustcode/pull/${params.number}`,
            additions: 9120,
            deletions: 12,
            changedFiles: 64,
            body: "Adds `apps/desktop`: a Tauri shell around `rustcode --acp`.\n\n- Composer, approvals, harness pickers\n- Link chips and Smart UI",
            checkRuns: [
              { name: "desktop / check", status: "passed" },
              { name: "rust / test", status: "pending" },
            ],
            files: [{ path: "apps/desktop/src/ui/Composer.tsx", additions: 420, deletions: 0 }],
            activity: [
              {
                author: "reviewer",
                kind: "COMMENTED",
                at: new Date(minutes(40)).toISOString(),
                body: "Looks good so far.",
              },
            ],
          },
        };
      case "_rustcode/pulls/review":
        return { review: { url: "https://github.com/LHagfoss/rustcode/pull/2788#pullrequestreview-1" } };
      case "_rustcode/rewind": {
        const thread = this.thread(params.sessionId);
        const wanted = Number(params.userIndex);
        let n = -1;
        const at = thread.history.findIndex(
          (u) => str(obj(u.update)?.sessionUpdate) === "user_message_chunk" && ++n === wanted,
        );
        if (at < 0) throw Object.assign(new Error("No such message"), { code: -32602 });
        thread.history.splice(at);
        return {};
      }
      case "_rustcode/warm":
        return {};
      case "_rustcode/worktrees/list": {
        const repo = str(params.repo);
        if (!repo) throw Object.assign(new Error("repo is required"), { code: -32602 });
        return {
          worktrees: this.threads
            .filter(
              (t) => t.worktree && t.worktree.repo === repo && !this.removedWorktrees.has(t.worktree.path),
            )
            .map((t) => ({ ...t.worktree, session_ids: [t.id] }) as unknown as Json),
        };
      }
      case "_rustcode/worktrees/create": {
        const repo = str(params.repo);
        if (!repo) throw Object.assign(new Error("repo is required"), { code: -32602 });
        return this.makeWorktree(repo, str(params.branch), str(params.title)) as unknown as Json;
      }
      case "_rustcode/worktrees/remove": {
        const path = str(params.path);
        if (!path) throw Object.assign(new Error("path is required"), { code: -32602 });
        const t = this.threads.find((x) => x.worktree?.path === path);
        if (!t?.worktree) throw Object.assign(new Error("No such worktree"), { code: -32602 });
        if (params.force !== true && t.worktree.dirty) {
          throw Object.assign(new Error("The worktree has uncommitted changes."), {
            code: -32000,
            data: { reason: "dirty" },
          });
        }
        if (params.force !== true && t.worktree.ahead > 0) {
          throw Object.assign(new Error("The branch has commits that are on no remote."), {
            code: -32000,
            data: { reason: "unpushed" },
          });
        }
        this.removedWorktrees.add(path);
        return { removed: true };
      }
      case "_rustcode/session/status": {
        const status = str(params.status);
        if (!status || !["review", "done", "idle"].includes(status)) {
          throw Object.assign(new Error("status must be review, done or idle"), { code: -32602 });
        }
        const thread = this.thread(params.session_id);
        thread.status = status;
        return {};
      }
      case "_rustcode/checkpoints/list": {
        const thread = this.thread(params.session_id);
        return {
          checkpoints: thread.checkpoints.map((c) => ({
            turn: c.turn,
            ref: `refs/rustcode/checkpoints/${thread.id}/${c.turn}`,
            created_at: c.created_at,
            prompt: c.prompt.slice(0, 80),
            files_changed: c.files.length,
            insertions: c.files.reduce((n, f) => n + f.insertions, 0),
            deletions: c.files.reduce((n, f) => n + f.deletions, 0),
          })),
        };
      }
      case "_rustcode/checkpoints/diff": {
        const thread = this.thread(params.session_id);
        const c = thread.checkpoints.find((x) => x.turn === params.turn);
        if (!c) throw Object.assign(new Error("No such checkpoint"), { code: -32602 });
        return { files: c.files as unknown as Json };
      }
      case "_rustcode/checkpoints/restore": {
        const thread = this.thread(params.session_id);
        const at = thread.checkpoints.findIndex((x) => x.turn === params.turn);
        if (at < 0) throw Object.assign(new Error("No such checkpoint"), { code: -32602 });
        const canRewind = thread.harness === "rustcode";
        if (params.rewind === true && canRewind) {
          let n = 0;
          const cut = thread.history.findIndex(
            (u) => str(obj(u.update)?.sessionUpdate) === "user_message_chunk" && ++n === at + 1,
          );
          if (cut >= 0) thread.history.splice(cut);
        }
        thread.checkpoints.splice(at);
        return { restored: true, conversation: params.rewind === true && canRewind ? "rewound" : "kept" };
      }
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
      case "_rustcode/themes":
        return this.themesSnapshot();
      case "_rustcode/themes/set": {
        const name = str(params.name);
        if (!name || !this.themes.some((t) => t.name === name))
          throw Object.assign(new Error(`no theme named '${name}'`), { code: -32602 });
        this.activeTheme = name;
        const snapshot = this.themesSnapshot();
        queueMicrotask(() => this.send({ method: "_rustcode/themes/changed", params: snapshot }));
        return snapshot;
      }
      case "_rustcode/themes/new": {
        const name = str(params.name) ?? "";
        const from = this.themes.find((t) => t.name === (str(params.from) ?? "rustcode")) ?? this.themes[0]!;
        if (!/^[a-z0-9][a-z0-9-]*$/.test(name) || this.themes.some((t) => t.name === name))
          throw Object.assign(new Error(`cannot create '${name}'`), { code: -32602 });
        const path = `~/.config/rustcode/themes/${name}.toml`;
        this.themes.push({ ...from, name, description: name, source: "user", path });
        return { path };
      }
      case "_rustcode/session/pin": {
        const thread = this.thread(params.session_id);
        if (typeof params.pinned !== "boolean")
          throw Object.assign(new Error("pinned must be a boolean"), { code: -32602 });
        thread.pinned = params.pinned;
        return {};
      }
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

  private makeWorktree(repo: string, branch?: string, title?: string): MockWorktree {
    let name = branch || `feat/${slug(title ?? "thread")}`;
    const taken = new Set(this.threads.flatMap((t) => (t.worktree ? [t.worktree.branch] : [])));
    for (let n = 2; taken.has(name); n++) name = `${branch || `feat/${slug(title ?? "thread")}`}-${n}`;
    const repoName = repo.split("/").filter(Boolean).pop() ?? "repo";
    return {
      path: `/Users/you/.config/rustcode/worktrees/${repoName}/${name.replace(/\//g, "-")}`,
      branch: name,
      repo,
      base: "c1ace820",
      created_at: new Date().toISOString(),
      dirty: false,
      ahead: 0,
      behind: 0,
    };
  }

  /** The agent's side of orchestration: a tool call forwarded to the client, as the engine does. */
  private orchestrate(sid: string, tool: string, args: JsonObject): Promise<JsonObject> {
    return this.ask("_rustcode/orchestration", { sessionId: sid, tool, args });
  }

  /** "fan out …", "ask the other thread …", "start a thread …": the v2 tools, when the client offers them. */
  private async orchestrationTurn(
    sid: string,
    prompt: string,
    push: (body: JsonObject) => void,
  ): Promise<string | null> {
    const tool = /fan.?out/i.test(prompt)
      ? "fan_out"
      : /start (a |another )?thread/i.test(prompt)
        ? "start_session"
        : /(ask|message) (the )?(other|another) thread/i.test(prompt)
          ? "message_session"
          : null;
    if (!tool) return null;
    if (this.orchestration < (tool === "message_session" ? 1 : 2)) {
      return "I can't reach other threads here: turn on Settings › General › Threads can start and message other threads, then restart the engine.";
    }
    const call = (name: string, args: JsonObject) => {
      const callId = `orch-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: name,
        kind: "other",
        status: "in_progress",
        rawInput: args,
      });
      return this.orchestrate(sid, name, args).then((result) => {
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: obj(result.error) ? "failed" : "completed",
          rawOutput: { content: JSON.stringify(result) },
        });
        return result;
      });
    };
    const task =
      prompt.replace(
        /^.*?(fan.?out|start (a |another )?thread|(ask|message) (the )?(other|another) thread)[:,]?\s*/i,
        "",
      ) || "Update the greeting";
    let links: string[] = [];
    if (tool === "fan_out") {
      const result = await call("fan_out", {
        prompt: `${task} (yolo)`,
        title: task.slice(0, 50),
        harnesses: [{ harness: "rustcode" }, { harness: "codex", model: "gpt-6-luna", effort: "low" }],
      });
      const error = obj(result.error);
      if (error) return `The fan-out did not start: ${str(error.message)}.`;
      links = arr(result.links).flatMap((l) => str(obj(l)?.link_id) ?? []);
    } else if (tool === "start_session") {
      const result = await call("start_session", { prompt: `${task} (yolo)`, title: task.slice(0, 50) });
      const error = obj(result.error);
      if (error) return `The thread did not start: ${str(error.message)}.`;
      links = [str(result.link_id) ?? ""];
    } else {
      const list = await call("list_sessions", {});
      const target = str(obj(arr(list.sessions)[0])?.session_id);
      if (!target) return "There is no other thread in this project to ask.";
      const result = await call("message_session", { session_id: target, message: `${task} (yolo)` });
      const error = obj(result.error);
      if (error) return `The message was not sent: ${str(error.message)}.`;
      links = [str(result.link_id) ?? ""];
    }
    const answers: string[] = [];
    for (let i = 0; i < 40 && answers.length < links.length; i++) {
      const pending = links.filter((_, j) => !answers[j]);
      const waited = await call("wait_session", { link_ids: pending, timeout_ms: 30_000 });
      for (const l of arr(waited.links)) {
        const o = obj(l);
        const at = links.indexOf(str(o?.link_id) ?? "");
        if (at >= 0 && str(o?.status) !== "working" && str(o?.status) !== "queued") {
          answers[at] =
            `${str(o?.title)}: ${str(o?.status)}${str(o?.answer) ? ` — ${str(o?.answer)?.split("\n")[0]}` : ""}`;
        }
      }
    }
    return `Here is what came back:\n\n${answers.map((a) => `- ${a}`).join("\n")}`;
  }

  /** "One agent spawns several": two children, started by `spawn_agent` calls, that finish a moment later. */
  private async spawnAgents(sid: string, push: (body: JsonObject) => void, run: { cancelled: boolean }) {
    const tasks: [string, string][] = [
      ["explorer", "Map where the composer handles paste and drop"],
      ["worker", "Write tests for link chips in the composer"],
    ];
    const list = this.agents.get(sid) ?? [];
    this.agents.set(sid, list);
    for (const [type, task] of tasks) {
      const id = list.length + 1;
      const callId = `spawn-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: "spawn_agent",
        kind: "other",
        status: "in_progress",
        rawInput: { task, agent_type: type },
      });
      list.push({
        id,
        name: `agent-${id}`,
        task,
        model: "gpt-6-luna",
        agentType: type,
        status: "running",
        activeTurn: true,
        depth: 1,
        startedAtMs: Date.now(),
        lastActivityMs: Date.now(),
        toolCalls: 3,
        requests: 2,
        inputTokens: 18_000,
        outputTokens: 900,
        cachedInputTokens: 12_000,
      });
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: callId,
        status: "completed",
        rawOutput: { content: `{"agent_id":${id},"name":"agent-${id}"}` },
      });
    }
    const finish = () => {
      for (const agent of list) {
        if (agent.status !== "running") continue;
        agent.status = "completed";
        agent.activeTurn = false;
        agent.finishedAtMs = Date.now();
        agent.completion = `Done: ${String(agent.task).toLowerCase()}.`;
      }
    };
    if (this.delay) setTimeout(finish, this.delay * 40);
    else finish();
    if (run.cancelled) return;
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
      if (/^From thread/.test(prompt) === false) {
        const answer = await this.orchestrationTurn(sid, prompt, push);
        if (answer !== null) {
          push({ sessionUpdate: "agent_message_chunk", content: text(answer) });
          return done("end_turn");
        }
      }
      if (/\bagents\b|subagents?/i.test(prompt)) await this.spawnAgents(sid, push, run);
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
      thread.checkpoints.push({
        turn: (thread.checkpoints.at(-1)?.turn ?? 0) + 1,
        prompt: prompt.slice(0, 80),
        created_at: new Date().toISOString(),
        files: [{ path: "src/app.ts", status: "modified", insertions: 1, deletions: 1, patch: APP_PATCH }],
      });
      if (thread.worktree) thread.worktree.dirty = true;
      if (!/yolo/i.test(prompt) && !/^From thread/.test(prompt)) {
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
      const reply = /\bview\b/i.test(prompt)
        ? MOCK_VIEW
        : /\bcharts?\b/i.test(prompt)
          ? MOCK_VIZ
          : [
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

/** A `rustcode-viz` reply: a bar chart and a checklist. */
export const MOCK_VIZ = [
  "Test time by crate, and where the migration stands:\n\n",
  "```rustcode-viz\n",
  '{"version":1,"type":"bar_chart","title":"Test time by crate","summary":"engine dominates the run","unit":"s","items":[{"label":"engine","value":42},{"label":"tui","value":10.5},{"label":"core","value":3}]}\n',
  "```\n\n",
  "```rustcode-viz\n",
  '{"version":1,"type":"checklist","title":"Orchestrator v2","items":[{"label":"Worktrees","status":"done"},{"label":"Checkpoints","status":"in_progress"},{"label":"Fan-out compare","status":"todo","note":"needs the engine"}]}\n',
  "```\n",
];

/** A Smart UI reply: a themed chart and a button that asks a follow-up through `rustcode.send`. */
export const MOCK_VIEW = [
  "Here are this week's test runs. Pick a day to dig in.\n\n",
  "```rustcode-ui v1\n",
  "<title>Test runs this week</title>\n",
  '<div class="rc-row rc-gap-3" style="margin-bottom:8px">\n',
  '  <div class="rc-card rc-col"><span class="rc-muted">Runs</span><b class="rc-num" style="font-size:20px">128</b></div>\n',
  '  <div class="rc-card rc-col"><span class="rc-muted">Failed</span><b class="rc-num" style="font-size:20px;color:var(--rc-danger, red)">7</b></div>\n',
  '  <div class="rc-card rc-col"><span class="rc-muted">Median</span><b class="rc-num" style="font-size:20px">41 s</b></div>\n',
  "</div>\n",
  '<div style="height:180px"><canvas id="c"></canvas></div>\n',
  '<div class="rc-row rc-gap-2" style="margin-top:8px">\n',
  '  <button class="primary" id="ask">Explain Thursday\'s failures</button>\n',
  '  <button id="copy">Copy numbers</button>\n',
  "</div>\n",
  '<script src="rustcode-lib:chart.js"></script>\n',
  "<script>\n",
  "new Chart(document.getElementById('c'), { type: 'bar', data: { labels: ['Mon','Tue','Wed','Thu','Fri'],\n",
  "  datasets: [{ label: 'Passed', data: [24, 27, 22, 19, 29] }, { label: 'Failed', data: [1, 0, 2, 4, 0] }] },\n",
  "  options: { scales: { x: { stacked: true }, y: { stacked: true } } } });\n",
  "document.getElementById('ask').onclick = () => window.rustcode?.send(\"Why did 4 tests fail on Thursday?\");\n",
  "document.getElementById('copy').onclick = () => window.rustcode?.copy('Mon 24/1, Tue 27/0, Wed 22/2, Thu 19/4, Fri 29/0');\n",
  "</script>\n",
  "```\n",
];

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
    if (this.options.bootMs) await new Promise((r) => setTimeout(r, this.options.bootMs));
    onEvent({ type: "log", line: "mock engine started" });
    onEvent({ type: "open", engine: "mock" });
    const crash = this.options.crashAfterMs;
    if (crash && !this.crashed) {
      this.crashed = true;
      setTimeout(() => {
        onEvent({ type: "closed", reason: "The engine exited unexpectedly (signal 9)." });
        void this.start(onEvent);
      }, crash);
    }
  }
  private crashed = false;

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
    if (command === "thread_rename") return this.engine?.renameThread(String(args.id), String(args.title));
    if (command === "thread_trash") return this.engine?.trashThread(String(args.id));
    if (command === "git_changes")
      return mockGitChanges(String(args.cwd), args.base ? String(args.base) : undefined);
    if (
      command === "reveal_path" ||
      command === "open_terminal_at" ||
      command === "open_path" ||
      command === "open_in_editor"
    )
      return null;
    if (command === "skill_create") return `~/.config/rustcode/skills/${String(args.name)}/SKILL.md`;
    throw new Error(`The mock does not run ${command}.`);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }
}

/** Canned `git_changes`, as the shell answers it: the branch since its base adds the login form. */
export function mockGitChanges(cwd: string, base?: string): JsonObject {
  const files: Json[] = [
    { path: "src/app.ts", status: "M", added: 1, removed: 1, binary: false },
    { path: "src/ui/Composer.tsx", status: "M", added: 1, removed: 0, binary: false },
  ];
  let diff = `diff --git a/src/app.ts b/src/app.ts\nindex 1..2 100644\n--- a/src/app.ts\n+++ b/src/app.ts\n${APP_PATCH}\ndiff --git a/src/ui/Composer.tsx b/src/ui/Composer.tsx\nindex 3..4 100644\n--- a/src/ui/Composer.tsx\n+++ b/src/ui/Composer.tsx\n${COMPOSER_PATCH}\n`;
  if (base || cwd.includes("worktrees")) {
    files.push({ path: "src/ui/LoginForm.tsx", status: "A", added: 14, removed: 0, binary: false });
    diff += `diff --git a/src/ui/LoginForm.tsx b/src/ui/LoginForm.tsx\nnew file mode 100644\n--- /dev/null\n+++ b/src/ui/LoginForm.tsx\n${LOGIN_PATCH}\n`;
  }
  return { branch: cwd.includes("worktrees") ? "feat/login-form" : "main", files, diff };
}
