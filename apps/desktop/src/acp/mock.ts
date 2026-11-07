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
  /**
   * `default`: onboarded with history. `onboarding`: first run. `empty`: no threads.
   * `delegates`: a thread with two levels of delegates in mixed states, for the details tree.
   */
  scenario?: string;
  /** Milliseconds between streamed chunks; 0 for tests. */
  delayMs?: number;
  /**
   * Seed the needs-you inbox: two approvals and a question across threads, one of
   * them on a thread whose last turn failed. Off by default so a test starts from
   * silence; the browser mock turns it on unless `?needsYou=0`.
   */
  needsYou?: boolean;
  /** Milliseconds the engine takes to start (`?boot=`), to see the slow-start toast. */
  bootMs?: number;
  /** Crash this many ms after the first start (`?crash=`); it comes back after `bootMs`, as the shell restarts it. */
  crashAfterMs?: number;
  /**
   * How the default Claude login's plan reads (`?claudeUsage=`), for looking at
   * the expired and denied states without a Keychain: `expired` is the sign-in
   * RustCode cannot use until Claude Code renews it, `denied` a Keychain prompt
   * the user declined, `forbidden` Anthropic refusing this sign-in access.
   * Named accounts stay readable, as their own rows.
   */
  claudeUsage?: "expired" | "refused" | "forbidden" | "denied";
  /**
   * The link book the engine keeps: data on disk, so it outlives a restart of the engine (a new
   * `MockEngine` on the same transport). Seeded here when a test starts from a stored book.
   */
  links?: JsonObject;
  /**
   * The orchestration command receipts the engine's data store keeps, by
   * `<caller>\u0000<client_request_id>`. Kept across an engine restart like
   * `links`, since surviving one is the point of a receipt.
   */
  receipts?: JsonObject;
}

interface MockThread {
  id: string;
  cwd: string;
  title?: string;
  updatedAt: number;
  harness: string;
  /** The named account of that harness, when the thread runs on one. */
  account?: string;
  model: string;
  effort: string;
  history: JsonObject[];
  worktree?: MockWorktree;
  status?: string;
  pinned?: boolean;
  /** The thread an agent started this one from, as the engine records it. */
  parent?: { id?: string; title?: string };
  checkpoints: MockCheckpoint[];
}

interface MockWorktree {
  path: string;
  branch: string;
  repo: string;
  base: string;
  base_ref?: string;
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
  // `turn/steer` takes the message's input items, pictures included.
  steering: {
    live: true,
    attachments: true,
    while_awaiting_user: true,
    interrupt_restart: true,
    queue: true,
  },
  rewind: false,
  permission_modes: [],
  subagents: false,
  smart_ui: false,
  rustcode_extensions: false,
};
const ACP_CAPS: HarnessCapabilities = {
  ...CODEX_CAPS,
  steer: "none",
  // An ACP agent that advertised no `_session/steering`: the message waits for the next turn.
  steering: {
    live: false,
    attachments: false,
    while_awaiting_user: true,
    interrupt_restart: true,
    queue: true,
  },
  models: "static",
  effort: false,
  resume: false,
  permission_modes: [],
};

/** Claude Code's adapter advertises `_session/steering`: a message goes into the running turn. */
const CLAUDE_CAPS: HarnessCapabilities = {
  ...ACP_CAPS,
  steer: "live",
  steering: {
    live: true,
    attachments: true,
    while_awaiting_user: true,
    interrupt_restart: true,
    queue: true,
  },
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
        accounts: [],
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
        // How a named Codex login is run, as the engine describes it.
        account_env: "CODEX_HOME",
        account_login_command: ["codex", "login"],
        accounts: [
          {
            id: "paral",
            name: "Paral",
            config_dir: "~/.codex-paral",
            auth: "ok",
          },
        ],
        // The Mac's own Codex home, which no account names yet.
        discovered_accounts: [{ config_dir: "~/.codex", suggested_name: "Codex" }],
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
        capabilities: CLAUDE_CAPS,
        account_env: "CLAUDE_CONFIG_DIR",
        account_login_command: ["claude", "auth", "login"],
        accounts: [
          {
            id: "paral-2",
            name: "Paral 2",
            config_dir: "~/.claude-paral-2",
            auth: "ok",
          },
        ],
        // Logins the engine finds on this Mac that no account names yet, so
        // Settings › Agents' "Found on this Mac" can be seen and tried.
        discovered_accounts: [
          { config_dir: "~/.claude", suggested_name: "Claude" },
          { config_dir: "~/.claude-personal", suggested_name: "Personal" },
        ],
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
        accounts: [],
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
        accounts: [],
        models: [],
        error: null,
      },
    ],
    default: { harness: "rustcode", model: "chatgpt/gpt-6-luna", effort: "medium" },
    onboarded,
  };
}

/** Each harness's remembered new-thread choice, as the engine's snapshot carries it (`last`). */
function withLast(snapshot: HarnessSnapshot): HarnessSnapshot {
  return { ...snapshot, harnesses: snapshot.harnesses.map((h) => ({ ...h, last: h.last ?? null })) };
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

/** An orchestration call's receipt key: the caller thread and its retry key, as the store holds it. */
function receiptKey(params: JsonObject | undefined): string {
  return `${str(params?.caller) ?? ""}\u0000${str(params?.clientRequestId) ?? ""}`;
}

/** A recorded parent as `session/list` carries it in `_meta.rustcode.parent`. */
const mockParent = (parent: { id?: string; title?: string }): Json => ({
  ...(parent.id ? { sessionId: parent.id } : {}),
  ...(parent.title ? { title: parent.title } : {}),
});

/**
 * `?scenario=delegated`: a parent with a child whose link is long pruned, and a
 * thread an older release started whose parent is gone. The child is nested
 * under its parent; the orphan has no parent to sit under, so it keeps a row of
 * its own (`state/topLevel.ts`).
 */
function seedDelegatedThreads(): MockThread[] {
  const turn = (id: string, ask: string, say: string): JsonObject[] => [
    update(id, { sessionUpdate: "user_message_chunk", content: text(ask) }),
    update(id, { sessionUpdate: "agent_message_chunk", content: text(say) }),
  ];
  return [
    {
      id: "mock-parent",
      cwd: "/Users/you/code/rustcode",
      title: "Update the desktop sidebar",
      updatedAt: minutes(2),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: turn("mock-parent", "Make the sidebar calmer.", "Done: fewer rows, same information."),
    },
    {
      id: "mock-child",
      cwd: "/Users/you/code/rustcode",
      title: "Research: agents querying the orchestrator",
      updatedAt: minutes(4),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "low",
      parent: { id: "mock-parent", title: "Update the desktop sidebar" },
      checkpoints: [],
      history: turn(
        "mock-child",
        "From thread “Update the desktop sidebar” (agent):\n\nRead how agents query the orchestrator.",
        "Agents reach the data store through the app's rules.",
      ),
    },
    {
      id: "mock-orphan",
      cwd: "/Users/you/code/rustcode",
      title: "Review fixes: chat noise + sidebar children",
      updatedAt: minutes(3),
      harness: "pi",
      model: "deepseek/flash",
      effort: "high",
      // Only the title survived: the parent thread and its link are gone.
      parent: { title: "my orchestrator app im working on" },
      checkpoints: [],
      history: turn(
        "mock-orphan",
        "From thread “my orchestrator app im working on” (agent):\n\nFix the reported review items.",
        "Fixed: the notices fold into one line.",
      ),
    },
    {
      id: "mock-plain",
      cwd: "/Users/you/code/rustcode",
      title: "Pick a name for the new command",
      updatedAt: minutes(8),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: turn("mock-plain", "What should the new command be called?", "`rustcode data query`."),
    },
  ];
}

/** The threads of `?scenario=delegates`: ids and titles, shared by the threads and the links. */
const DELEGATES = {
  root: "mock-delegate-root",
  research: "mock-delegate-research",
  tests: "mock-delegate-tests",
  docs: "mock-delegate-docs",
  review: "mock-delegate-review",
  wait: "mock-delegate-wait",
  stop: "mock-delegate-stop",
} as const;

const DELEGATE_TITLES: Record<string, string> = {
  [DELEGATES.root]: "Ship the delegate tree",
  [DELEGATES.research]: "Research how delegates are recorded",
  [DELEGATES.tests]: "Write the rollup unit tests",
  [DELEGATES.docs]: "Document the delegate tree",
  [DELEGATES.review]: "Review the delegate tree diff",
  [DELEGATES.wait]: "Confirm the tree's status words",
  [DELEGATES.stop]: "Try the tree in the app",
};

/**
 * `?scenario=delegates`: a thread that delegated two levels deep, so the details
 * tree has something to nest: a branch still working, one waiting on the user,
 * and branches that finished, failed and were stopped. The links carry the times
 * the tree counts (a working branch from its turn's start, a waiting one from
 * the approval it waits on); the threads' own status is what makes a branch live
 * once the app has marked the stored links interrupted at connect
 * (`state/host.ts` `syncLinkBook`).
 */
function seedDelegateTreeThreads(): MockThread[] {
  const thread = (
    id: string,
    harness: string,
    model: string,
    effort: string,
    updatedAt: number,
    parent: string | undefined,
    ask: string,
    say: string,
    status?: string,
  ): MockThread => ({
    id,
    cwd: "/Users/you/code/rustcode",
    title: DELEGATE_TITLES[id],
    updatedAt,
    harness,
    model,
    effort,
    ...(status ? { status } : {}),
    ...(parent ? { parent: { id: parent, title: DELEGATE_TITLES[parent] } } : {}),
    checkpoints: [],
    history: [
      update(id, { sessionUpdate: "user_message_chunk", content: text(ask) }),
      update(id, { sessionUpdate: "agent_message_chunk", content: text(say) }),
    ],
  });
  return [
    thread(
      DELEGATES.root,
      "rustcode",
      "chatgpt/gpt-6-luna",
      "high",
      minutes(2),
      undefined,
      "Draw the delegates as a tree in the details panel.",
      "I will hand the research, the tests and the docs to threads and keep the wiring here.",
    ),
    // The live branch: a working researcher with a finished and a failed thread under it.
    thread(
      DELEGATES.research,
      "codex",
      "gpt-6-luna",
      "low",
      minutes(1),
      DELEGATES.root,
      "From thread “Ship the delegate tree” (agent):\n\nMap how a delegated thread records its parent and its turns.",
      "The link carries the start and the settle; the thread's own status tells what it does now.",
      "running",
    ),
    thread(
      DELEGATES.tests,
      "codex",
      "gpt-6-luna",
      "low",
      minutes(6),
      DELEGATES.research,
      "From thread “Research how delegates are recorded” (agent):\n\nCover the roll-up and the paused clock.",
      "Added 14 tests: the branch sum, the clock that freezes on an approval, and an unknown cost.",
    ),
    thread(
      DELEGATES.docs,
      "claude-code",
      "claude-sonnet-6",
      "medium",
      minutes(7),
      DELEGATES.research,
      "From thread “Research how delegates are recorded” (agent):\n\nWrite the delegate tree down in docs/design.md.",
      "The docs build failed on an unknown token name.",
    ),
    thread(
      DELEGATES.review,
      "pi",
      "deepseek/flash",
      "high",
      minutes(5),
      DELEGATES.root,
      "From thread “Ship the delegate tree” (agent):\n\nReview the tree's rules before this goes out.",
      "Two notes: fold the finished branch, and keep the cost unknown when no turn reported one.",
    ),
    thread(
      DELEGATES.wait,
      "claude-code",
      "sonnet",
      "medium",
      minutes(3),
      DELEGATES.root,
      "From thread “Ship the delegate tree” (agent):\n\nCheck the state words against docs/terminology.md.",
      "“Needs you” reads right for an approval; may I rename the stopped branch too?",
      "needs_input",
    ),
    thread(
      DELEGATES.stop,
      "rustcode",
      "chatgpt/gpt-6-luna",
      "medium",
      minutes(9),
      DELEGATES.root,
      "From thread “Ship the delegate tree” (agent):\n\nClick through the tree in the app.",
      "Stopped before the check ran.",
    ),
  ];
}

/** The links of `?scenario=delegates`: who started whom, how it went and for how long. */
function seedDelegateLinks(): JsonObject[] {
  const link = (
    orchestrator: string,
    n: number,
    target: string,
    state: string,
    createdAt: number,
    extra: JsonObject,
  ): JsonObject => ({
    id: `${orchestrator}/${n}`,
    orchestrator,
    target,
    orchestratorTitle: DELEGATE_TITLES[orchestrator]!,
    targetTitle: DELEGATE_TITLES[target]!,
    message: `From thread “${DELEGATE_TITLES[orchestrator]}” (agent):\n\n${DELEGATE_TITLES[target]}.`,
    state,
    started: true,
    createdAt,
    ...extra,
  });
  return [
    // Two levels: the root started the researcher, the researcher started two threads of its own.
    link(DELEGATES.root, 1, DELEGATES.research, "working", minutes(3), {
      harness: "codex",
      harnessName: "Codex",
      startedAt: minutes(2),
    }),
    link(DELEGATES.research, 1, DELEGATES.tests, "done", minutes(8), {
      harness: "codex",
      harnessName: "Codex",
      settledAt: minutes(6),
      answer: "Added 14 tests: the branch sum, the clock that freezes on an approval, and an unknown cost.",
      filesChanged: 1,
    }),
    link(DELEGATES.research, 2, DELEGATES.docs, "failed", minutes(7), {
      harness: "claude-code",
      harnessName: "Claude Code",
      settledAt: minutes(6),
      answer: "The docs build failed on an unknown token name.",
    }),
    link(DELEGATES.root, 2, DELEGATES.review, "done", minutes(9), {
      harness: "pi",
      harnessName: "Pi",
      settledAt: minutes(5),
      answer: "Two notes: fold the finished branch, and keep the cost unknown when no turn reported one.",
      filesChanged: 2,
    }),
    // Waiting on the user: the clock stopped when the question was asked.
    link(DELEGATES.root, 3, DELEGATES.wait, "needs_user", minutes(4), {
      harness: "claude-code",
      harnessName: "Claude Code",
      startedAt: minutes(3),
      waitingSince: minutes(2),
    }),
    link(DELEGATES.root, 4, DELEGATES.stop, "cancelled", minutes(10), {
      harness: "rustcode",
      harnessName: "RustCode",
      settledAt: minutes(9),
    }),
  ];
}

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
          toolCallId: "call-r0",
          title: "read src/ui/Composer.tsx",
          kind: "read",
          rawInput: { path: "src/ui/Composer.tsx" },
        }),
        update(a, { sessionUpdate: "tool_call_update", toolCallId: "call-r0", status: "completed" }),
        update(a, {
          sessionUpdate: "tool_call",
          toolCallId: "call-r1",
          title: "read src/ui/App.tsx",
          kind: "read",
          rawInput: { path: "src/ui/App.tsx" },
        }),
        update(a, { sessionUpdate: "tool_call_update", toolCallId: "call-r1", status: "completed" }),
        update(a, {
          sessionUpdate: "tool_call",
          toolCallId: "call-r2",
          title: "read src/state/store.ts",
          kind: "read",
          rawInput: { path: "src/state/store.ts" },
        }),
        update(a, { sessionUpdate: "tool_call_update", toolCallId: "call-r2", status: "completed" }),
        update(a, {
          sessionUpdate: "tool_call",
          toolCallId: "call-t",
          title: "bash",
          kind: "execute",
          rawInput: { command: "bun test src/ui", summary: "Run the composer tests" },
        }),
        update(a, { sessionUpdate: "tool_call_update", toolCallId: "call-t", status: "completed" }),
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
      // A delegate, so Statistics can show a parent's rolled-up tokens and cost.
      parent: { id: a, title: "Fix the composer focus ring" },
      checkpoints: [],
      history: [
        update(b, {
          sessionUpdate: "user_message_chunk",
          content: text("How does the sidecar restart backoff work?"),
        }),
        update(b, {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "It doubles from 0.5 s up to 30 s, and resets after the engine stays up for a minute. See https://github.com/LHagfoss/rustcode/pull/2788 and the timer in https://docs.rs/tokio/latest/tokio/time/fn.sleep.html.\n\n```mermaid\nflowchart LR\n  A[Engine exits] --> B{Up for a minute?}\n  B -- yes --> C[Reset to 0.5 s]\n  B -- no --> D[Double the wait, max 30 s]\n  C --> E[Restart]\n  D --> E\n```\n\n![The backoff over time](docs/backoff.png)",
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
      // Not in pricing.ts's table: Statistics shows this row as unknown, not $0.
      model: "claude-sonnet-6",
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
      // A thread on a named account: its plan numbers are that account's own,
      // read when the thread is opened (`_rustcode/usage/plans` with its id).
      id: "mock-thread-claude",
      cwd: "/Users/you/code/website",
      title: "Tidy the pricing page copy",
      updatedAt: minutes(12),
      harness: "claude-code",
      account: "paral-2",
      model: "sonnet",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-thread-claude", {
          sessionUpdate: "user_message_chunk",
          content: text("Tighten the pricing page: three tiers, no jargon."),
        }),
        update("mock-thread-claude", {
          sessionUpdate: "agent_message_chunk",
          content: text("Rewrote the three tier blurbs; each is two lines now."),
          ...turn(22_000),
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
        base_ref: "origin/main",
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
          content: text(
            "I wrote `LoginForm`, but the typecheck failed: the form's submit handler expects a `FormEvent` and the field is `unknown`.",
          ),
          // The last turn failed, so the inbox has a failed row to offer Retry for.
          _meta: {
            rustcode: {
              turn: {
                id: "t1",
                elapsedMs: 52_000,
                outcome: "failed",
                endedAt: new Date(minutes(20)).toISOString(),
              },
            },
          },
        }),
      ],
    },
  ];
}

/** What a mock thread spent, as the data store would record it. */
interface MockSpend {
  input: number;
  output: number;
  cached: number;
  cacheWrite: number | null;
  cost: number | null;
  turns: number;
  /** Turns with neither input nor output reported. */
  withoutTokens: number;
  /** Turns missing one of the four token counts. */
  withoutAny: number;
  withoutInput: number;
  withoutOutput: number;
  withoutCached: number;
  withoutCacheWrite: number;
  withoutCost: number;
}

/** Known threads get numbers that make the missing ones visible; the rest get a small canned spend. */
const MOCK_SPEND: Record<string, MockSpend> = {
  "mock-thread-1": {
    input: 1_240_000,
    output: 32_100,
    cached: 980_000,
    cacheWrite: null,
    cost: null,
    turns: 8,
    withoutTokens: 0,
    withoutAny: 8,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 8,
    withoutCacheWrite: 8,
    withoutCost: 8,
  },
  "mock-thread-2": {
    input: 248_000,
    output: 9_100,
    cached: 201_000,
    cacheWrite: null,
    cost: 3.42,
    turns: 12,
    withoutTokens: 0,
    withoutAny: 12,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 12,
    withoutCost: 0,
  },
  "mock-thread-3": {
    input: 88_000,
    output: 4_200,
    cached: 60_000,
    cacheWrite: 12_000,
    cost: null,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 4,
  },
  "mock-thread-4": {
    input: 512_000,
    output: 22_000,
    cached: 400_000,
    cacheWrite: 0,
    cost: 5.1,
    turns: 9,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-thread-claude": {
    input: 112,
    output: 72_297,
    cached: 14_757_930,
    cacheWrite: 633_078,
    cost: 77.22,
    turns: 9,
    withoutTokens: 7,
    withoutAny: 7,
    withoutInput: 7,
    withoutOutput: 7,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-parent": {
    input: 820_000,
    output: 30_000,
    cached: 700_000,
    cacheWrite: null,
    cost: null,
    turns: 6,
    withoutTokens: 0,
    withoutAny: 6,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 6,
    withoutCost: 6,
  },
  "mock-child": {
    input: 4_500_000,
    output: 180_000,
    cached: 3_900_000,
    cacheWrite: 420_000,
    cost: 8.4,
    turns: 14,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-orphan": {
    input: 1_200_000,
    output: 40_000,
    cached: 900_000,
    cacheWrite: null,
    cost: 4.1,
    turns: 7,
    withoutTokens: 0,
    withoutAny: 7,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 7,
    withoutCost: 0,
  },
  "mock-plain": {
    input: 310_000,
    output: 12_000,
    cached: 250_000,
    cacheWrite: null,
    cost: null,
    turns: 5,
    withoutTokens: 0,
    withoutAny: 5,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 5,
    withoutCost: 5,
  },
  // `?scenario=delegates`: a branch with reported costs, and one thread whose
  // model has no price, so the tree shows `—` rather than a made-up number.
  "mock-delegate-root": {
    input: 640_000,
    output: 24_000,
    cached: 520_000,
    cacheWrite: null,
    cost: 1.9,
    turns: 5,
    withoutTokens: 0,
    withoutAny: 5,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 5,
    withoutCost: 0,
  },
  "mock-delegate-research": {
    input: 1_450_000,
    output: 58_000,
    cached: 1_200_000,
    cacheWrite: null,
    cost: 2.4,
    turns: 9,
    withoutTokens: 0,
    withoutAny: 9,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 9,
    withoutCost: 0,
  },
  "mock-delegate-tests": {
    input: 520_000,
    output: 26_000,
    cached: 410_000,
    cacheWrite: null,
    cost: 0.85,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 4,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 4,
    withoutCost: 0,
  },
  "mock-delegate-docs": {
    input: 190_000,
    output: 8_000,
    cached: 150_000,
    cacheWrite: 21_000,
    cost: null,
    turns: 3,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 3,
  },
  "mock-delegate-review": {
    input: 2_100_000,
    output: 61_000,
    cached: 1_800_000,
    cacheWrite: null,
    cost: 3.1,
    turns: 11,
    withoutTokens: 0,
    withoutAny: 11,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 11,
    withoutCost: 0,
  },
  "mock-delegate-wait": {
    input: 380_000,
    output: 15_000,
    cached: 300_000,
    cacheWrite: 48_000,
    cost: 1.05,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 4,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-delegate-stop": {
    input: 60_000,
    output: 2_000,
    cached: 40_000,
    cacheWrite: null,
    cost: 0.2,
    turns: 1,
    withoutTokens: 0,
    withoutAny: 1,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 1,
    withoutCost: 0,
  },
};

function spendOf(thread: MockThread, index: number): MockSpend {
  const known = MOCK_SPEND[thread.id];
  if (known) return known;
  const input = 40_000 + (index % 7) * 12_000;
  return {
    input,
    output: Math.round(input / 10),
    cached: Math.round(input / 3),
    cacheWrite: null,
    cost: null,
    turns: 1,
    withoutTokens: 0,
    withoutAny: 1,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 1,
    withoutCost: 1,
  };
}

const sumKnown = (values: (number | null)[]): number | null => {
  const known = values.filter((value): value is number => value !== null);
  return known.length ? known.reduce((sum, value) => sum + value, 0) : null;
};

/** A day in the last 30 for a mock thread, so the daily chart has gaps and variation. */
function mockUsageDay(index: number): string {
  const offset = [0, 1, 1, 3, 6, 11, 18, 24][index % 8] ?? 0;
  return new Date(Date.now() - offset * 86_400_000).toISOString().slice(0, 10);
}

/** `_rustcode/data/usage`, grouped like the engine groups the store (`engine/src/acp/data.rs`). */
function mockUsage(threads: MockThread[], groupBy: string): JsonObject {
  const groups = new Map<string, { keys: JsonObject; spends: MockSpend[] }>();
  for (const [index, thread] of threads.entries()) {
    const spend = spendOf(thread, index);
    let key: string;
    let keys: JsonObject;
    switch (groupBy) {
      case "model":
        key = `${thread.harness}\u0000${thread.model}`;
        keys = { harness: thread.harness, model: thread.model };
        break;
      case "day": {
        const day = mockUsageDay(index);
        key = day;
        keys = { day };
        break;
      }
      case "project":
        key = thread.cwd;
        keys = { project: thread.cwd };
        break;
      case "thread":
        key = thread.id;
        keys = {
          thread: thread.id,
          title: thread.title ?? "",
          project: thread.cwd,
          harness: thread.harness,
          model: thread.model,
          // The mock's turns all run the thread's own harness and model.
          model_harness: thread.harness,
          parent_id: thread.parent?.id ?? null,
          parent_title: thread.parent?.title ?? null,
        };
        break;
      default:
        key = thread.harness;
        keys = { harness: thread.harness };
    }
    const group = groups.get(key) ?? { keys, spends: [] };
    group.spends.push(spend);
    groups.set(key, group);
  }
  const rows = [...groups.values()]
    .map(({ keys, spends }) => ({
      ...keys,
      input_tokens: sumKnown(spends.map((spend) => spend.input)),
      output_tokens: sumKnown(spends.map((spend) => spend.output)),
      cached_tokens: sumKnown(spends.map((spend) => spend.cached)),
      cache_write_tokens: sumKnown(spends.map((spend) => spend.cacheWrite)),
      cost_usd: sumKnown(spends.map((spend) => spend.cost)),
      turns: spends.reduce((sum, spend) => sum + spend.turns, 0),
      turns_without_tokens: spends.reduce((sum, spend) => sum + spend.withoutTokens, 0),
      turns_without_any_tokens: spends.reduce((sum, spend) => sum + spend.withoutAny, 0),
      turns_without_input_tokens: spends.reduce((sum, spend) => sum + spend.withoutInput, 0),
      turns_without_output_tokens: spends.reduce((sum, spend) => sum + spend.withoutOutput, 0),
      turns_without_cached_tokens: spends.reduce((sum, spend) => sum + spend.withoutCached, 0),
      turns_without_cache_write_tokens: spends.reduce((sum, spend) => sum + spend.withoutCacheWrite, 0),
      turns_without_cost: spends.reduce((sum, spend) => sum + spend.withoutCost, 0),
    }))
    .sort(
      (one, other) =>
        Number(other.input_tokens ?? 0) +
          Number(other.output_tokens ?? 0) -
          (Number(one.input_tokens ?? 0) + Number(one.output_tokens ?? 0)) || 0,
    );
  const total = (key: string) =>
    sumKnown(
      rows.map((row) =>
        typeof row[key as keyof typeof row] === "number" ? (row[key as keyof typeof row] as number) : null,
      ),
    );
  const turns = rows.reduce((sum, row) => sum + Number(row.turns ?? 0), 0);
  return {
    group_by: groupBy,
    since: null,
    project: null,
    scope: "every project",
    rows,
    totals: {
      input_tokens: total("input_tokens"),
      output_tokens: total("output_tokens"),
      cached_tokens: total("cached_tokens"),
      cache_write_tokens: total("cache_write_tokens"),
      cost_usd: total("cost_usd"),
      turns,
      turns_without_tokens: rows.reduce((sum, row) => sum + Number(row.turns_without_tokens ?? 0), 0),
      turns_without_any_tokens: rows.reduce((sum, row) => sum + Number(row.turns_without_any_tokens ?? 0), 0),
      turns_without_input_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_input_tokens ?? 0),
        0,
      ),
      turns_without_output_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_output_tokens ?? 0),
        0,
      ),
      turns_without_cached_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_cached_tokens ?? 0),
        0,
      ),
      turns_without_cache_write_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_cache_write_tokens ?? 0),
        0,
      ),
      turns_without_cost: rows.reduce((sum, row) => sum + Number(row.turns_without_cost ?? 0), 0),
    },
    truncated: false,
    note: "tokens are what each harness reported; cost is the USD a harness reported, never a price RustCode computed",
    version: 1,
  };
}

/** What the mock's report commands answer, in the chunks they stream in. */
const MOCK_REPORTS: Record<string, string[]> = {
  "/status": [
    "**Status**\n\nModel: `gpt-6-luna`\nEffort: medium\nHarness: RustCode\n",
    "\n### Session\n- Messages: 14\n- Context: 18k of 272k\n- Folder: ~/code/rustcode\n",
  ],
  "/context": [
    "**Context**\n\n18k of 272k tokens in use. Most of it is tool output from this turn; ",
    "`/compact` would free about 9k.",
  ],
};

const COMMANDS: Json[] = [
  { name: "compact", description: "Summarize the conversation to free context" },
  {
    name: "status",
    description: "Show the thread's model, harness and session",
    _meta: { rustcode: { report: true } },
  },
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
  {
    name: "model",
    description: "Switch the model",
    input: { hint: "model id" },
    _meta: { rustcode: { action: "model_picker" } },
  },
  { name: "skills", description: "List the skills this thread can load" },
  { name: "clear", description: "Start a new thread", _meta: { rustcode: { action: "new_session" } } },
  { name: "copy", description: "Copy the last reply", _meta: { rustcode: { action: "copy_response" } } },
  {
    name: "resume",
    description: "Switch to another thread",
    _meta: { rustcode: { action: "session_picker" } },
  },
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
  /** The inbox seed runs once, after the list the app first reads. */
  private needsYouSeeded = false;
  /** The client's `_meta.rustcode.orchestration` at initialize. */
  private orchestration = 0;
  /** Discord Rich Presence (docs/discord-rich-presence.md), as `discord_rpc_enabled` holds it. */
  private presence = true;
  /** The payload-capture switches (docs/data-store.md › Captured payloads), off as `config.toml` defaults them. */
  private payloadCapture = { providerPayloads: false, harnessRpcPayloads: false };
  private removedWorktrees = new Set<string>();
  /** The link book the app keeps in the data store: none until it saves one, and kept in `options` as the engine's disk does. */
  private get linkBook(): JsonObject | undefined {
    return this.options.links;
  }
  private set linkBook(book: JsonObject | undefined) {
    this.options.links = book;
  }
  /** What the store remembers of each orchestration call made with a retry key. */
  private get receipts(): JsonObject {
    this.options.receipts ??= {};
    return this.options.receipts;
  }
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
  /** MCP servers the mock restarted through `_rustcode/capabilities/mcp/restart`. */
  private mcpRestarted = new Set<string>();

  private themesSnapshot(): JsonObject {
    return { active: this.activeTheme, themes: this.themes as unknown as Json };
  }

  /** The MCP rows `_rustcode/capabilities` lists; `fagbrev` starts failed until it is restarted. */
  private mcpServers(): JsonObject[] {
    const up = this.mcpRestarted.has("fagbrev");
    return [
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
      up
        ? {
            name: "fagbrev",
            transport: "stdio",
            target: "npx -y fagbrev-mcp",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: 3,
            error: null,
          }
        : {
            name: "fagbrev",
            transport: "stdio",
            target: "npx -y fagbrev-mcp",
            enabled: true,
            scope: "user",
            connection: "failed",
            tools: 0,
            error: "MCP server 'fagbrev' failed to start: spawn npx ENOENT",
          },
    ];
  }

  constructor(
    private readonly emit: (line: string) => void,
    private readonly options: MockOptions = {},
  ) {
    const scenario = options.scenario ?? "default";
    this.threads =
      scenario === "default"
        ? seedThreads()
        : scenario === "delegated"
          ? seedDelegatedThreads()
          : scenario === "delegates"
            ? seedDelegateTreeThreads()
            : [];
    // The stored book is the only one the app reads at connect; a scenario brings its own.
    if (scenario === "delegates") {
      this.options.links ??= {
        links: seedDelegateLinks() as unknown as Json,
        grants: {},
        counters: {},
        fanOuts: [],
      };
    }
    this.harnesses = withLast(initialHarnesses(scenario !== "onboarding"));
  }

  private get delay() {
    return this.options.delayMs ?? 40;
  }

  /**
   * The inbox's demo: a question, an approval and a failed turn on threads the
   * app has not opened, so the list has something to show on first open. Each
   * request is asked like a live one and answered by the app's own cards.
   */
  private seedNeedsYou() {
    const has = (id: string) => this.threads.some((t) => t.id === id);
    if (has("mock-thread-1")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-1",
        toolCall: {
          toolCallId: "seed-approval-1",
          title: "bash",
          kind: "execute",
          rawInput: { command: "bun test src/ui", summary: "Run the composer tests" },
        },
        options: [
          { optionId: "allow", name: "Allow", kind: "allow_once" },
          { optionId: "always", name: "Always allow commands", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
    }
    if (has("mock-thread-claude")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-claude",
        toolCall: {
          toolCallId: "seed-question-1",
          title: "Which tone?",
          rawInput: {
            header: "Pricing copy",
            question: "Which tone should the pricing blurbs use?",
            options: ["Plain and short", "Warmer, more human", "Keep the current voice"],
            descriptions: [
              "Three lines, no adjectives",
              "Talks to one reader at a time",
              "Only fix the jargon",
            ],
            multiple: false,
          },
        },
        options: [
          { optionId: "tone-plain", name: "Plain and short", kind: "allow_once" },
          { optionId: "tone-warm", name: "Warmer, more human", kind: "allow_once" },
          { optionId: "tone-keep", name: "Keep the current voice", kind: "allow_once" },
        ],
        _meta: {
          rustcode: {
            kind: "question",
            version: 2,
            questionId: "seed-question-1",
            answerKinds: ["options", "text"],
          },
        },
      });
    }
    if (has("mock-thread-4")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-4",
        toolCall: {
          toolCallId: "seed-approval-4",
          title: "edit src/ui/LoginForm.tsx",
          kind: "edit",
          rawInput: { path: "src/ui/LoginForm.tsx" },
        },
        options: [
          { optionId: "allow", name: "Allow", kind: "allow_once" },
          { optionId: "always", name: "Always allow edits", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
    }
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
            _meta: {
              rustcode: { steer: 1, agents: 2, rewind: 1, pullRequests: 3, sessionMove: 1, sessionRepair: 1 },
            },
          },
          authMethods: [],
        };
      }
      // The catalog, for a thread that has no session yet to send it on.
      case "_rustcode/commands":
        return { sessionUpdate: "available_commands_update", availableCommands: COMMANDS };
      case "session/list": {
        if (this.options.needsYou && !this.needsYouSeeded) {
          this.needsYouSeeded = true;
          setTimeout(() => this.seedNeedsYou(), 500);
        }
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
                  harness: { id: t.harness },
                  ...(t.account ? { account: t.account } : {}),
                  ...(t.status ? { status: t.status } : {}),
                  ...(t.pinned ? { pinned: true } : {}),
                  ...(t.parent ? { parent: mockParent(t.parent) } : {}),
                  ...(t.worktree ? { worktree: t.worktree as unknown as Json } : {}),
                },
              },
            })),
        };
      }
      case "session/new": {
        const meta = obj(obj(params._meta)?.rustcode);
        const harness = str(meta?.harness) ?? this.harnesses.default.harness;
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const account = str(meta?.account);
        if (account && !(h.accounts ?? []).some((a) => a.id === account))
          throw Object.assign(new Error(`unknown ${harness} account '${account}'`), { code: -32602 });
        const wanted = obj(meta?.worktree);
        let worktree: MockWorktree | undefined;
        if (wanted) {
          const repo = str(wanted.repo) ?? str(params.cwd) ?? "/";
          worktree = this.makeWorktree(repo, str(wanted.branch), str(wanted.title));
        }
        // The thread an agent starts this one from: like the engine, keep it so
        // pruning the link book cannot make the child a top-level thread.
        const parent = str(meta?.parent);
        const parentTitle = this.threads.find((t) => t.id === parent)?.title;
        const thread: MockThread = {
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          cwd: worktree?.path ?? str(params.cwd) ?? "/",
          worktree,
          checkpoints: [],
          updatedAt: Date.now(),
          harness,
          ...(account ? { account } : {}),
          ...(parent ? { parent: { id: parent, title: parentTitle } } : {}),
          model:
            str(meta?.model) ??
            h.last?.model ??
            (harness === this.harnesses.default.harness ? this.harnesses.default.model : null) ??
            (harness === "rustcode" ? "chatgpt/gpt-6-luna" : (HARNESS_MODELS[harness]?.[0]?.id ?? "default")),
          // Like the real engine, the built-in harness takes its effort from config, not from `session/new`.
          effort:
            (harness === "rustcode" ? undefined : str(meta?.effort)) ??
            h.last?.effort ??
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
              ...(account ? { account: { id: account } } : {}),
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
            cost: { amount: 0.0123, currency: "USD" },
            _meta: {
              rustcode: {
                estimated: true,
                inputTokens: 90_000,
                outputTokens: 3_200,
                cachedTokens: 61_000,
                context: {
                  model: "gpt-6-luna",
                  window: 272_000,
                  used: 18_400,
                  free: 253_600,
                  categories: [
                    { name: "User messages", tokens: 1_900 },
                    { name: "Agent responses", tokens: 3_400 },
                    { name: "Tool calls", tokens: 5_100 },
                    { name: "Compacted history", tokens: 0 },
                    { name: "System prompt", tokens: 4_200 },
                    { name: "System tools", tokens: 2_900 },
                    { name: "Skills", tokens: 900 },
                    { name: "Subagents", tokens: 0 },
                  ],
                },
              },
            },
          }),
        );
        const h = this.harnesses.harnesses.find((x) => x.id === thread.harness);
        // Like the engine before a harness ever listed its models: a restored external thread knows
        // only its own model, until `_rustcode/harnesses/models` ran.
        const unlisted = !!h && !h.builtin && !h.models.length;
        return {
          configOptions: unlisted
            ? [
                {
                  id: "model",
                  name: "Model",
                  category: "model",
                  type: "select",
                  currentValue: thread.model,
                  options: [{ value: thread.model, name: thread.model }],
                },
              ]
            : configOptions(thread, this.harnesses),
          _meta: {
            rustcode: {
              harness: { id: thread.harness, capabilities: (h?.capabilities ?? {}) as Json },
              ...(thread.account ? { account: { id: thread.account } } : {}),
            },
          },
        };
      }
      case "session/close":
        return {};
      case "_rustcode/session/move": {
        const thread = this.thread(params.sessionId);
        const cwd = str(params.cwd);
        if (!cwd?.startsWith("/")) throw new Error("cwd must be absolute");
        thread.cwd = cwd;
        return { cwd };
      }
      case "_rustcode/session/repair": {
        const thread = this.thread(params.sessionId);
        return { cwd: thread.cwd, fixed: [], problems: [] };
      }
      case "session/set_config_option": {
        const thread = this.thread(params.sessionId);
        const value = str(params.value);
        const before = { model: thread.model, effort: thread.effort };
        if (params.configId === "model" && value) thread.model = value;
        if (params.configId === "effort" && value) thread.effort = value;
        // Like the engine: a change after the first message marks the place.
        if (thread.history.length && (before.model !== thread.model || before.effort !== thread.effort)) {
          const last = thread.history.at(-1) as
            | {
                update?: {
                  _meta?: { rustcode?: { kind?: string; model_from?: string; effort_from?: string } };
                };
              }
            | undefined;
          const pending = last?.update?._meta?.rustcode;
          const from =
            pending?.kind === "switch" ? { model: pending.model_from, effort: pending.effort_from } : before;
          if (pending?.kind === "switch") thread.history.pop();
          const divider = update(thread.id, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "Changed the model or effort" },
            _meta: {
              rustcode: {
                kind: "switch",
                model_from: from.model ?? null,
                model_to: thread.model,
                effort_from: from.effort ?? null,
                effort_to: thread.effort,
              },
            },
          });
          if (from.model !== thread.model || from.effort !== thread.effort) thread.history.push(divider);
          queueMicrotask(() => this.notify(divider));
        }
        // Like the engine: a thread's pick is also its harness's next new-thread choice.
        const picked = this.harnesses.harnesses.find((x) => x.id === thread.harness);
        if (picked && (params.configId === "model" || params.configId === "effort")) {
          picked.last = { model: thread.model, effort: thread.effort };
        }
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
      case "_rustcode/data/usage": {
        // The same shape engine/src/acp/data.rs answers with.
        const by = (params as { groupBy?: string } | undefined)?.groupBy ?? "harness";
        return mockUsage(this.threads, by);
      }
      case "_rustcode/data/query":
        return {
          columns: ["tool_name", "failures", "calls"],
          rows: [
            { tool_name: "run_command", failures: 19, calls: 240 },
            { tool_name: "replace_file_content", failures: 6, calls: 310 },
          ],
          truncated: false,
        };
      case "_rustcode/data/activity": {
        // The same shape engine/src/acp/data.rs answers with: summaries only.
        const at = new Date(Date.now() - 60_000).toISOString();
        return {
          session_id: str(params.sessionId) ?? "",
          items: [
            { source: "event", at, type: "turn.start", fields: { tool_rounds: 0 } },
            {
              source: "provider_request",
              at,
              provider: "openai",
              model: "gpt-6-luna",
              status_code: 200,
              latency_ms: 412,
              input_tokens: 1200,
              output_tokens: 80,
              attempt: 1,
            },
          ],
          next: null,
          has_more: false,
        };
      }
      case "_rustcode/data/links/load":
        // The same shape `_rustcode/data/links/load` answers with (engine/src/acp/data.rs).
        return {
          stored: this.linkBook !== undefined,
          book: this.linkBook ?? { links: [], grants: {}, counters: {}, fanOuts: [] },
        };
      case "_rustcode/data/receipts/get": {
        // The same shape engine/src/acp/data.rs answers with.
        const key = receiptKey(params);
        const stored = obj(this.receipts[key]);
        if (!stored) return { state: "none" };
        if (stored.action !== str(obj(params)?.action) || stored.argsHash !== str(obj(params)?.argsHash)) {
          return { state: "conflict" };
        }
        return { state: "replayed", result: stored.result ?? {}, created_at: stored.createdAt ?? "" };
      }
      case "_rustcode/data/receipts/put": {
        const key = receiptKey(params);
        const result = obj(obj(params)?.result);
        if (!result) throw Object.assign(new Error("result must be a JSON object"), { code: -32602 });
        // The first receipt under a key stays, as the store keeps it.
        this.receipts[key] ??= {
          action: str(obj(params)?.action) ?? "",
          argsHash: str(obj(params)?.argsHash) ?? "",
          result,
          createdAt: new Date().toISOString(),
        };
        return { saved: true };
      }
      case "_rustcode/data/links/save": {
        const book = obj(params)?.book;
        if (!book || typeof book !== "object" || Array.isArray(book)) {
          throw Object.assign(new Error("book is required"), { code: -32602 });
        }
        this.linkBook = book as JsonObject;
        return { saved: true };
      }
      case "_rustcode/usage/plans": {
        const at = (h: number) => Math.floor(Date.now() / 1000) + h * 3600;
        const p = params as
          | { claude?: boolean; claudeAccounts?: boolean; claudeAccount?: string; force?: boolean }
          | undefined;
        // `?claudeUsage=expired|refused|denied` stages a sign-in RustCode could
        // not read, so Settings › Usage's expired and denied rows can be seen
        // (and its Refresh tried) without a Keychain. A forced read answers the
        // same: the real engine re-reads and only then knows it is still broken.
        const failure = this.options.claudeUsage;
        const failed = (reason: NonNullable<MockOptions["claudeUsage"]>) => ({
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          status: "unavailable",
          reason,
          error:
            reason === "expired"
              ? "Claude Code's sign-in has expired. Open Claude Code to renew it, then Refresh."
              : reason === "refused"
                ? "Claude Code's sign-in was refused. Sign in again in Claude Code, then Refresh."
                : reason === "forbidden"
                  ? "Anthropic does not allow this sign-in to read plan usage. Press Refresh to try once more."
                  : "macOS did not allow reading Claude Code's sign-in from the Keychain.",
          windows: [],
        });
        // One row per login, as the engine answers it: the default login when
        // asked for, and each account the client names (all of them on an
        // explicit look in Settings › Usage, else the one the app asks for).
        const accounts = [
          {
            id: "paral-2",
            name: "Paral 2",
            windows: [
              { label: "5-hour", usedPercent: 4, windowMinutes: 300, resetsAt: at(4) },
              { label: "Weekly", usedPercent: 12, windowMinutes: 10080, resetsAt: at(90) },
            ],
          },
        ].filter((a) => p?.claude && (p?.claudeAccounts || a.id === p?.claudeAccount));
        return {
          plans: [
            p?.claude
              ? failure
                ? failed(failure)
                : {
                    id: "claude",
                    name: "Claude",
                    via: "Claude Code",
                    status: "ok",
                    plan: "max",
                    windows: [
                      { label: "5-hour", usedPercent: 18, windowMinutes: 300, resetsAt: at(3) },
                      { label: "Weekly", usedPercent: 46, windowMinutes: 10080, resetsAt: at(80) },
                      { label: "Weekly · Opus", usedPercent: 83, windowMinutes: 10080, resetsAt: at(80) },
                    ],
                  }
              : { id: "claude", name: "Claude", via: "Claude Code", status: "notAsked", windows: [] },
            ...accounts.map((a) => ({
              id: `claude:${a.id}`,
              name: a.name,
              via: "Claude Code",
              harness: "claude-code",
              account: a.id,
              status: "ok",
              plan: "max",
              windows: a.windows,
            })),
            {
              id: "codex",
              name: "ChatGPT",
              via: "Codex CLI",
              status: "ok",
              windows: [
                { label: "5-hour", usedPercent: 42, windowMinutes: 300, resetsAt: at(2) },
                { label: "Weekly", usedPercent: 71, windowMinutes: 10080, resetsAt: at(72) },
              ],
            },
          ],
        };
      }
      case "_rustcode/usage/limits": {
        // A thread's own windows, as the engine answers them: Claude Code's are
        // null (only its sign-in's plan read has them), so the composer meter
        // reads the account's row from `_rustcode/usage/plans`.
        const sessionId = str(obj(params)?.sessionId);
        const thread = this.threads.find((t) => t.id === sessionId);
        if (thread?.harness === "claude-code") return null;
        return {
          source: "codex",
          primary: { usedPercent: 42, windowMinutes: 300, resetsAt: Math.floor(Date.now() / 1000) + 7200 },
          secondary: {
            usedPercent: 71,
            windowMinutes: 10080,
            resetsAt: Math.floor(Date.now() / 1000) + 3 * 86400,
          },
        };
      }
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
      case "_rustcode/config/telemetry":
        // The same shape `_rustcode/config/telemetry` answers with (engine/src/acp/telemetry.rs).
        return { ...this.payloadCapture };
      case "_rustcode/config/telemetry/set": {
        const asked = obj(params);
        if (typeof asked?.providerPayloads !== "boolean" || typeof asked?.harnessRpcPayloads !== "boolean") {
          throw Object.assign(new Error("both switches are required"), { code: -32602 });
        }
        this.payloadCapture = {
          providerPayloads: asked.providerPayloads,
          harnessRpcPayloads: asked.harnessRpcPayloads,
        };
        return { ...this.payloadCapture };
      }
      case "_rustcode/presence":
        return { enabled: this.presence, discordDetected: true };
      case "_rustcode/presence/set":
        this.presence = params.enabled === true;
        return { enabled: this.presence, discordDetected: true };
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
            accounts: [],
            models: [],
            error: null,
          };
          this.harnesses.harnesses.push(h);
        }
        // As the engine: RustCode turns off only once another harness is the default.
        if (h.builtin && params.enabled === false && this.harnesses.default.harness === h.id)
          throw new Error("choose another default harness before turning RustCode off");
        if (typeof params.enabled === "boolean") h.enabled = params.enabled;
        if (Array.isArray(params.command)) h.command = params.command.map(String);
        return this.snapshot();
      }
      case "_rustcode/harnesses/remove": {
        this.harnesses.harnesses = this.harnesses.harnesses.filter(
          (h) =>
            h.id !== params.id ||
            ["rustcode", "codex", "claude-code", "gemini", "opencode", "pi"].includes(h.id),
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
      case "_rustcode/harnesses/accounts/add": {
        const harness = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const id = str(params.id) ?? "";
        if (!id || !/^[a-z0-9_.-]+$/.test(id))
          throw Object.assign(new Error(`invalid account id '${id}'`), { code: -32602 });
        if ((h.accounts ?? []).some((a) => a.id === id))
          throw Object.assign(new Error(`there is already a ${harness} account '${id}'`), {
            code: -32602,
          });
        const prefix = h.account_env === "CODEX_HOME" ? "~/.codex-" : "~/.claude-";
        const config_dir = str(params.config_dir) ?? `${prefix}${id}`;
        h.accounts = [
          ...(h.accounts ?? []),
          {
            id,
            name: str(params.name) ?? id,
            config_dir,
            auth: "unknown",
          },
        ];
        // A login found on this Mac is no longer offered as a discovery.
        h.discovered_accounts = (h.discovered_accounts ?? []).filter(
          (found) => found.config_dir !== config_dir,
        );
        return this.snapshot();
      }
      case "_rustcode/harnesses/accounts/remove": {
        const harness = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const id = str(params.id) ?? "";
        if ((h.accounts ?? []).some((a) => a.id === id && this.threads.some((t) => t.account === id)))
          throw Object.assign(new Error(`a thread is running on the account '${id}'`), {
            code: -32602,
          });
        h.accounts = (h.accounts ?? []).filter((a) => a.id !== id);
        return this.snapshot();
      }
      case "_rustcode/harnesses/accounts/check":
        return this.snapshot();
      case "_rustcode/session/harness": {
        const thread = this.thread(params.sessionId);
        const id = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h || h.builtin) throw Object.assign(new Error(`cannot switch to '${id}'`), { code: -32602 });
        const account = str(params.account) ?? null;
        if (account && !(h.accounts ?? []).some((a) => a.id === account))
          throw Object.assign(new Error(`unknown ${id} account '${account}'`), { code: -32602 });
        const from = thread.harness;
        const fromAccount = (this.harnesses.harnesses.find((x) => x.id === from)?.accounts ?? []).find(
          (a) => a.id === thread.account,
        )?.name;
        thread.harness = id;
        thread.account = account ?? undefined;
        thread.model = str(params.model) ?? h.models[0]?.id ?? thread.model;
        const toAccount = (h.accounts ?? []).find((a) => a.id === account)?.name;
        const label = (name: string, login?: string) => (login ? `${name} · ${login}` : name);
        const divider = update(thread.id, {
          sessionUpdate: "agent_message_chunk",
          content: {
            type: "text",
            text: `Switched from ${label(from, fromAccount)} to ${label(id, toAccount)}`,
          },
          _meta: {
            rustcode: {
              kind: "handoff",
              from,
              to: id,
              from_account: fromAccount ?? null,
              to_account: toAccount ?? null,
              model: thread.model,
            },
          },
        });
        thread.history.push(divider);
        queueMicrotask(() => this.notify(divider));
        return {
          configOptions: configOptions(thread, this.harnesses),
          _meta: {
            rustcode: {
              harness: { id, capabilities: h.capabilities as unknown as Json },
              ...(account ? { account: { id: account } } : {}),
            },
          },
        };
      }
      case "_rustcode/harnesses/remember": {
        const id = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) throw Object.assign(new Error(`unknown harness '${id}'`), { code: -32602 });
        const model = str(params.model) ?? null;
        const effort = str(params.effort) ?? null;
        h.last = model || effort ? { model, effort } : null;
        return { harness: id, last: (h.last ?? null) as unknown as Json };
      }
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
          mcpServers: this.mcpServers(),
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
      case "_rustcode/capabilities/mcp/restart": {
        const name = str(params.name) ?? "";
        if (name !== "github" && name !== "sqlite" && name !== "fagbrev")
          throw Object.assign(new Error(`MCP server '${name}' is no longer configured`), {
            code: -32602,
          });
        await this.sleep(400);
        this.mcpRestarted.add(name);
        return this.handle("_rustcode/capabilities", {});
      }
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
      base_ref: "origin/main",
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
    if (/manage (this|the) thread/i.test(prompt) && this.orchestration >= 3) {
      const call = async (name: string, args: JsonObject) => {
        const callId = `orch-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: callId,
          title: name,
          kind: "other",
          status: "in_progress",
          rawInput: args,
        });
        const result = await this.orchestrate(sid, name, args);
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: obj(result.error) ? "failed" : "completed",
          rawOutput: { content: JSON.stringify(result) },
        });
      };
      await call("list_harnesses", {});
      await call("search_sessions", { query: "login" });
      await call("list_worktrees", {});
      await call("list_projects", {});
      await call("list_pull_requests", { state: "open" });
      await call("update_session", { title: "Managed by its agent", pinned: true });
      await call("configure_session", { effort: "high" });
      return "Renamed and pinned this thread, and raised its effort.";
    }
    const tool = /fan.?out/i.test(prompt)
      ? "fan_out"
      : /start (a |another )?thread/i.test(prompt)
        ? "start_session"
        : /(ask|message) (the )?(other|another) thread/i.test(prompt)
          ? "message_session"
          : null;
    if (!tool) return null;
    if (this.orchestration < (tool === "message_session" ? 1 : 2)) {
      return "I can't reach other threads here: turn on Settings › Agents › Threads can start and message other threads, then restart the engine.";
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

  /**
   * "Task agents": Claude Code's subagents, as its adapter streams them. Each
   * `Agent` call's own steps carry `_meta.claudeCode.parentToolUseId`.
   */
  private async claudeAgents(push: (body: JsonObject) => void, run: { cancelled: boolean }) {
    const agents: [string, string, string, [string, string, JsonObject][]][] = [
      [
        "Explore",
        "Map the composer's paste handling",
        "explore",
        [
          ["Grep", "search", { pattern: "onPaste", path: "src/ui" }],
          ["Read Composer.tsx", "read", { file_path: "src/ui/Composer.tsx" }],
          ["Read compose.ts", "read", { file_path: "src/state/compose.ts" }],
        ],
      ],
      [
        "general-purpose",
        "Write tests for link chips",
        "worker",
        [
          ["Read Markdown.tsx", "read", { file_path: "src/ui/Markdown.tsx" }],
          ["Write Markdown.test.tsx", "edit", { file_path: "src/ui/Markdown.test.tsx" }],
          ["bun test src/ui", "execute", { command: "bun test src/ui" }],
        ],
      ],
    ];
    const ids = agents.map(([type, description]) => {
      const id = `agent-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: id,
        title: description,
        kind: "think",
        status: "in_progress",
        rawInput: { description, prompt: `${description}.`, subagent_type: type },
        _meta: { claudeCode: { toolName: "Agent" } },
      });
      return id;
    });
    const longest = Math.max(...agents.map((a) => a[3].length));
    for (let step = 0; step < longest; step++) {
      for (const [i, agent] of agents.entries()) {
        const s = agent[3][step];
        if (!s || run.cancelled) continue;
        const id = `${ids[i]}-${step}`;
        const meta = { claudeCode: { toolName: s[0].split(" ")[0] ?? s[0], parentToolUseId: ids[i] ?? "" } };
        push({
          sessionUpdate: "tool_call",
          toolCallId: id,
          title: s[0],
          kind: s[1],
          status: "in_progress",
          rawInput: s[2],
          _meta: meta,
        });
        await this.sleep(this.delay * 8);
        push({ sessionUpdate: "tool_call_update", toolCallId: id, status: "completed", _meta: meta });
      }
    }
    for (const [i, agent] of agents.entries()) {
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: ids[i]!,
        status: "completed",
        rawOutput: { content: `Done: ${agent[1].toLowerCase()}. Three files touched, all tests pass.` },
      });
      await this.sleep(this.delay * 4);
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
      // Report commands answer like the engine's: marked as a report, kept out of the transcript.
      const report = MOCK_REPORTS[prompt.trim()];
      if (report) {
        for (const chunk of report) {
          this.notify(
            update(sid, {
              sessionUpdate: "agent_message_chunk",
              content: text(chunk),
              _meta: { rustcode: { report: true } },
            }),
          );
          await this.sleep();
        }
        return done("end_turn");
      }
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
      // Claude Code's own words when a plan's limit stops a turn: a reply, not a failure.
      if (/\bhit the limit\b/i.test(prompt)) {
        push({
          sessionUpdate: "agent_message_chunk",
          content: text("You've hit your 5-hour limit · resets 11:59pm"),
        });
        return done("end_turn");
      }
      if (/\btask agents\b/i.test(prompt)) await this.claudeAgents(push, run);
      else if (/\bagents\b|subagents?/i.test(prompt)) await this.spawnAgents(sid, push, run);
      push({
        sessionUpdate: "plan",
        entries: [
          { content: "Find the code", status: "completed", priority: "medium" },
          { content: "Make the change", status: "in_progress", priority: "medium" },
          { content: "Run the checks", status: "pending", priority: "low" },
        ],
      });
      // A prompt about a plan stops there, with the plan still open: the card at the foot of the
      // transcript has something left to do and offers Implement.
      if (/\bplan\b/i.test(prompt)) {
        push({
          sessionUpdate: "agent_message_chunk",
          content: text("Here's how I'd do it. Say the word and I'll start."),
        });
        push({
          sessionUpdate: "plan",
          entries: [
            { content: "Read the composer and the transcript", status: "completed", priority: "medium" },
            { content: "Move the approval to its tool row", status: "pending", priority: "high" },
            { content: "Keep a resolved question in the transcript", status: "pending", priority: "medium" },
            { content: "Draw the plan card with Expand and Copy", status: "pending", priority: "medium" },
            { content: "Persist drafts across restarts", status: "pending", priority: "low" },
            { content: "Write the design.md paragraphs", status: "pending", priority: "low" },
            { content: "Add a mock state for each of the three", status: "pending", priority: "low" },
            { content: "Run the checks", status: "pending", priority: "high" },
          ],
        });
        return done("end_turn");
      }
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
      // One usage update per round, as the engine sends it: the session's
      // running totals. The app's Trace subtracts them to price each call, so a
      // mock turn shows where its money went too.
      const round = (input: number, output: number, cost: number) =>
        push({
          sessionUpdate: "usage_update",
          used: 24_000 + Math.round((Date.now() - started) / 10),
          size: 272_000,
          cost: { amount: cost, currency: "USD" },
          _meta: {
            rustcode: {
              estimated: false,
              inputTokens: input,
              outputTokens: output,
              cachedTokens: Math.round(input * 0.55),
            },
          },
        });
      round(120_000, 4_100, 0.0276);
      // A live call's id never reuses one the replayed history already has: an
      // update keyed the same would replace an old row instead of adding one.
      const callId = `live-${++this.counter}`;
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
      round(147_000, 4_900, 0.0341);
      // "trace" asks for the whole picture: a few more calls and one delegate,
      // so the Trace chip under the turn draws every kind of row.
      const forTrace = /\b(trace|waterfall|delegate)\b/i.test(prompt);
      if (forTrace) {
        const readId = `live-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: readId,
          title: "read src/ui/Composer.tsx",
          kind: "read",
          status: "in_progress",
          rawInput: { path: "src/ui/Composer.tsx", summary: "Read the composer" },
        });
        await this.sleep(this.delay * 3);
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: readId,
          status: "completed",
          rawOutput: {
            content: "export function Composer() {\n  const [draft, setDraft] = useState('');\n  ...\n}\n",
          },
        });
        round(166_000, 5_400, 0.0402);
      }
      const editId = `live-${++this.counter}`;
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
      if (forTrace) {
        round(184_000, 6_100, 0.0493);
        // A delegate: one call folded into the trace's single dotted row. Its
        // agent is recorded like `spawnAgents` records one, so its row and its
        // steps answer from the engine rather than from nothing.
        const task = "Check the composer tests for the focus fix";
        const agents = this.agents.get(sid) ?? [];
        this.agents.set(sid, agents);
        const agentId = agents.length + 1;
        agents.push({
          id: agentId,
          name: `agent-${agentId}`,
          task,
          model: "gpt-6-luna",
          agentType: "explorer",
          status: "running",
          activeTurn: true,
          depth: 1,
          startedAtMs: Date.now(),
          lastActivityMs: Date.now(),
          toolCalls: 1,
          requests: 1,
          inputTokens: 12_000,
          outputTokens: 600,
          cachedInputTokens: 8_000,
        });
        const delegateId = `spawn-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: delegateId,
          title: "spawn_agent",
          kind: "other",
          status: "in_progress",
          rawInput: { task, agent_type: "explorer" },
        });
        await this.sleep(this.delay * 10);
        const agent = agents.find((a) => a.id === agentId);
        if (agent) {
          agent.status = "completed";
          agent.activeTurn = false;
          agent.finishedAtMs = Date.now();
          agent.completion = "The composer tests pass; nothing to change.";
        }
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: delegateId,
          status: "completed",
          rawOutput: {
            content: JSON.stringify({ agent_id: agentId, name: `agent-${agentId}`, status: "completed" }),
          },
        });
      }
      // A message the turn read mid-step (`_rustcode/steer`): it says so where it read it, and the
      // plan moves on, so the app has a step that was active when the note landed and a next step
      // whose clock starts (`state/steer.ts`). A steer that arrives after this is still answered at
      // the foot of the turn, below.
      if (run.steers.length) {
        const read = run.steers.splice(0);
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`Noted ${read.map((steer) => `“${steer}”`).join(", ")}.\n\n`),
        });
        push({
          sessionUpdate: "plan",
          entries: [
            { content: "Find the code", status: "completed", priority: "medium" },
            { content: "Make the change", status: "completed", priority: "medium" },
            { content: "Run the checks", status: "in_progress", priority: "low" },
          ],
        });
      }
      const reply = /\bkit\b/i.test(prompt)
        ? MOCK_KIT
        : /\bview\b/i.test(prompt)
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
      round(214_000, 7_100, 0.0612);
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

/** A Smart UI reply built from the page's component kit: no script and next to no CSS of its own. */
export const MOCK_KIT = [
  "The release is blocked by one failing suite; everything else is green.\n\n",
  "```rustcode-ui v1\n",
  "<title>Release 1.11 readiness</title>\n",
  '<div class="rc-stats">\n',
  '  <div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Tests run</span><span class="rc-stat-note">+12 this week</span></div>\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span><span class="rc-stat-note">4 on Thursday</span></div>\n',
  '  <div class="rc-stat rc-ok"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span><span class="rc-stat-note">6 s faster</span></div>\n',
  "</div>\n",
  '<div class="rc-callout rc-warn">The auth suite fails on a stale fixture <span class="rc-tip" tabindex="0">1<span>tests/auth/fixtures/token.json expired on 2026-11-01, so every refresh test fails at setup.</span></span>, not on a code change.</div>\n',
  '<div class="rc-tabs">\n',
  '  <div role="tablist" aria-label="Sections"><button role="tab" data-rc-tab="suites">Suites</button><button role="tab" data-rc-tab="plan">Plan</button><button role="tab" data-rc-tab="log">Log</button></div>\n',
  '  <section data-rc-panel="suites">\n',
  '    <div class="rc-bars">\n',
  '      <div class="rc-bar"><span>engine</span><i style="width:100%"></i><b>42 s</b></div>\n',
  '      <div class="rc-bar"><span>desktop</span><i style="width:44%"></i><b>18.5 s</b></div>\n',
  '      <div class="rc-bar rc-bad"><span>auth</span><i style="width:25%"></i><b>10.5 s</b></div>\n',
  '      <div class="rc-bar"><span>core</span><i style="width:7%"></i><b>3 s</b></div>\n',
  "    </div>\n",
  '    <table class="rc-table"><thead><tr><th>Suite</th><th>Status</th><th class="rc-num">Passed</th><th class="rc-num">Failed</th><th class="rc-num">Time</th></tr></thead><tbody>\n',
  '      <tr><td>engine</td><td><span class="rc-badge rc-ok">Passing</span></td><td class="rc-num">64</td><td class="rc-num">0</td><td class="rc-num">42 s</td></tr>\n',
  '      <tr><td>desktop</td><td><span class="rc-badge rc-ok">Passing</span></td><td class="rc-num">38</td><td class="rc-num">0</td><td class="rc-num">18.5 s</td></tr>\n',
  '      <tr><td>auth</td><td><span class="rc-badge rc-bad">Failing</span></td><td class="rc-num">12</td><td class="rc-num">7</td><td class="rc-num">10.5 s</td></tr>\n',
  '      <tr><td>core</td><td><span class="rc-badge rc-info">Skipped 1</span></td><td class="rc-num">7</td><td class="rc-num">0</td><td class="rc-num">3 s</td></tr>\n',
  "    </tbody></table>\n",
  "  </section>\n",
  '  <section data-rc-panel="plan">\n',
  '    <div class="rc-row rc-gap-2" style="margin-bottom:12px"><span class="rc-label">2 of 4 steps</span><div class="rc-progress" style="flex:1"><span style="width:50%"></span></div></div>\n',
  '    <ol class="rc-steps">\n',
  '      <li class="rc-step rc-done">Bump the version <div class="rc-label">Done by the release script</div></li>\n',
  '      <li class="rc-step rc-done">Build the macOS app</li>\n',
  '      <li class="rc-step rc-failed">Run the auth suite <div class="rc-label">7 failures, one cause</div></li>\n',
  '      <li class="rc-step rc-active">Refresh the fixture</li>\n',
  '      <li class="rc-step rc-todo">Publish the beta</li>\n',
  "    </ol>\n",
  "  </section>\n",
  '  <section data-rc-panel="log">\n',
  '    <div class="rc-term"><div>$ cargo test -p auth</div><div class="rc-ok">test refresh::keeps_session ... ok</div><div class="rc-bad">test refresh::rotates_token ... FAILED</div><div>test result: 12 passed; 7 failed</div></div>\n',
  "  </section>\n",
  "</div>\n",
  "<div>\n",
  '  <details class="rc-details"><summary>Owners <span class="rc-label">3</span></summary><ul class="rc-list">\n',
  '    <li class="rc-item"><span>Engine</span><span class="rc-badge rc-ok">Ready</span></li>\n',
  '    <li class="rc-item"><span>Desktop</span><span class="rc-badge rc-ok">Ready</span></li>\n',
  '    <li class="rc-item"><span>Auth</span><span class="rc-badge rc-warn">Waiting</span></li>\n',
  "  </ul></details>\n",
  '  <details class="rc-details rc-tree"><summary>Changed files <span class="rc-label">4</span></summary><div>\n',
  '    <details class="rc-details" open><summary>tests/auth</summary><div>\n',
  '      <div class="rc-file"><span>fixtures/token.json</span><span>+2 −2</span></div>\n',
  '      <div class="rc-file"><span>refresh.rs</span><span>+14 −3</span></div>\n',
  "    </div></details>\n",
  '    <div class="rc-file"><span>Cargo.lock</span><span>+1 −1</span></div>\n',
  '    <div class="rc-file"><span>CHANGELOG.md</span><span>+6</span></div>\n',
  "  </div></details>\n",
  "</div>\n",
  '<div class="rc-row rc-gap-2" style="margin-top:12px">\n',
  '  <button class="rc-chip" data-rc-send="Refresh the auth fixture">Refresh the fixture</button>\n',
  '  <button class="rc-chip" data-rc-send="Show the failing tests">Show the failures</button>\n',
  '  <button class="rc-chip" data-rc-copy="128 run, 7 failed, 41 s median">Copy numbers</button>\n',
  "</div>\n",
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
    if (command === "project_files") return MOCK_PROJECT_FILES;
    if (command === "git_changes")
      return mockGitChanges(String(args.cwd), args.base ? String(args.base) : undefined);
    if (
      command === "reveal_path" ||
      command === "open_terminal_at" ||
      command === "open_path" ||
      command === "open_in_editor"
    )
      return null;
    if (command === "thread_image") {
      // A drawn stand-in for any picture inside the thread's folder; the shell's rules decide the rest.
      const path = String(args.path);
      if (path.startsWith("/") && !path.startsWith(String((args.roots as string[])[0] ?? "")))
        throw new Error("Pictures outside the thread's folder are not shown.");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="200" viewBox="0 0 480 200"><rect width="480" height="200" rx="12" fill="#18181a"/><polyline fill="none" stroke="#5b8def" stroke-width="3" points="20,180 80,170 140,150 200,110 260,40 320,40 380,40 440,40"/><text x="20" y="28" fill="#a1a1a1" font-family="-apple-system" font-size="14">${path}</text></svg>`;
      return { name: path.split("/").pop(), mimeType: "image/svg+xml", data: btoa(svg) };
    }
    if (command === "skill_create") return `~/.config/rustcode/skills/${String(args.name)}/SKILL.md`;
    throw new Error(`The mock does not run ${command}.`);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }
}

const MOCK_PROJECT_FILES = [
  "README.md",
  "apps/desktop/src/main.tsx",
  "apps/desktop/src/ui/Composer.tsx",
  "apps/desktop/src/ui/Sidebar.tsx",
  "apps/desktop/src/state/store.ts",
  "docs/design.md",
  "docs/my notes.md",
  "rustcode/engine/src/acp.rs",
];

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
