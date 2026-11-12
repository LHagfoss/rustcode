// design-lint: allow-file (fake link colours in canned data)
// A fake `rustcode --acp` engine with canned traffic, for tests and for
// running the UI without an engine (`bun run dev:mock`, or `?transport=mock`).
// It speaks the same JSON-RPC lines as the real engine, including the
// harness contract in docs/harnesses.md, so the UI works before that lands.

import { BUILTIN_THEMES } from "../lib/themes";
import { arr, type Json, type JsonObject, num, obj, str } from "./jsonrpc";
import type { Transport, TransportEvent } from "./transport";
import { BUILTIN_CAPABILITIES, type HarnessCapabilities, type HarnessSnapshot } from "./types";

/** The local mock's skills; their text is served only by `_rustcode/skills/get`. */
const MOCK_SKILLS: JsonObject[] = [
  {
    name: "release-automation",
    description: "Cut a release with scripts/release.sh",
    path: "~/.agents/skills/release-automation",
    source: "universal",
    readOnly: false,
    enabled: true,
    content:
      "---\nname: release-automation\ndescription: Cut a release with scripts/release.sh\n---\n\nRun scripts/release.sh --patch.\n",
  },
  {
    name: "frontend-design",
    description: "Design guidance for web UIs",
    path: ".rustcode/skills/frontend-design",
    source: "workspace",
    readOnly: false,
    enabled: false,
    content:
      "---\nname: frontend-design\ndescription: Design guidance for web UIs\n---\n\nRead docs/design.md before shaping a page.\n",
  },
  {
    name: "cloudflare-email-service",
    description: "Send mail from a Worker",
    path: "~/.claude/skills/cloudflare-email-service",
    source: "claude-user",
    readOnly: true,
    enabled: true,
    content:
      "---\nname: cloudflare-email-service\ndescription: Send mail from a Worker\n---\n\nUse the Email Sending binding.\n",
  },
];

function withoutSkillContent(skill: JsonObject): JsonObject {
  const row = { ...skill };
  delete row.content;
  return row;
}

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
   * the user declined, `forbidden` Anthropic refusing this sign-in access, and
   * `recorded` a lapsed token answered with the numbers RustCode last read for
   * that login ("as of" then). Named accounts stay readable, as their own rows.
   */
  claudeUsage?: "expired" | "refused" | "forbidden" | "denied" | "recorded";
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
  /**
   * Remote access the mock starts in (`?remote=`): `disconnected` shares with
   * the gateway unreachable, `expired` issues a pairing code that has already
   * run out, `connect` starts on with a phone asking to connect, `refused`
   * starts on with recent refusals (an app version this Mac does not speak, an
   * unknown pairing, an unexpected app origin). The default starts idle.
   */
  remote?: "disconnected" | "expired" | "connect" | "refused";
  /**
   * The machines this app is paired with (`?machines=`): `paired` is a machine
   * saved just now, `connect` starts with one saved machine on an older
   * RustCode reached over NetBird so the Add sheet has found machines to list
   * (one of them needing an update) and the server not installed, `offline` a
   * machine the shell keeps retrying, and `revoked` one whose pairing the
   * machine ended. The default is the paired, connected machine the happy path
   * shows.
   */
  machines?: "paired" | "connect" | "offline" | "revoked";
  /**
  /**
   * `?notices=repeat`: the default thread ends on six identical quiet notices, the shape an
   * access mode that lets calls through without asking leaves (one notice per call), so the
   * transcript's count (`state/fold.ts` `collapseNotices`) is visible without a real run.
   */
  notices?: "repeat";
  /**
   * A tool row's hard cases on their own thread (`?tools=background`): a command the harness sent to
   * the background, its answer fenced in Markdown, and the call repeated with its description in
   * `content`. It is a thread of the default scenario, so the rest of the mock is still there.
   */
  tools?: "background";
}

interface MockConnectRequest {
  id: string;
  deviceName: string;
  deviceKind: string;
  platform: string;
  remoteAddr: string;
  createdAt: number;
  expiresAt: number;
  state: "pending" | "approved";
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
  /** The thread this one came from and why, as the engine records it. */
  parent?: { id?: string; title?: string; relationship?: string };
  checkpoints: MockCheckpoint[];
  /** How many times `/compact` ran here, so the card can compare before and after. */
  compactions?: number;
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
  /** The state a rewind abandoned, kept for "go back to this attempt": no turn of its own. */
  attempt?: boolean;
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

const SETTINGS_SEARCH_PATCH = `@@ -18,6 +18,17 @@
 export function SettingsPanel({ groups }: { groups: SettingsGroup[] }) {
+  const [query, setQuery] = useState("");
+  const shown = query.trim() ? groups.filter((group) => matches(group, query)) : groups;
   return (
     <div className="flex flex-col gap-4">
+      <input
+        type="search"
+        value={query}
+        placeholder="Search settings"
+        onChange={(e) => setQuery(e.target.value)}
+        onKeyDown={(e) => e.key === "Escape" && setQuery("")}
+      />
-      {groups.map((group) => (
+      {shown.map((group) => (
         <SettingsGroup key={group.id} group={group} />
       ))}`;

const SETTINGS_FILTER_PATCH = `@@ -1,4 +1,4 @@
-export function matches(group: SettingsGroup, query: string) {
-  return group.title.toLowerCase().includes(query.toLowerCase());
+export function matches(group: SettingsGroup, query: string) {
+  const needle = query.trim().toLowerCase();
+  if (!needle) return true;
+  if (group.title.toLowerCase().includes(needle)) return true;
+  return group.rows.some((row) => isOpen(group, row) && matchesRow(row, needle));`;

const SETTINGS_HELPER_PATCH = `@@ -0,0 +1,10 @@
+import type { SettingsGroup, SettingsRow } from "./Settings";
+
+export function matchesRow(row: SettingsRow, needle: string): boolean {
+  return row.label.toLowerCase().includes(needle) || row.description.toLowerCase().includes(needle);
+}
+
+/** A group whose title answered keeps its rows: the reader is looking for the group itself. */
+export function isOpen(group: SettingsGroup, _row: SettingsRow): boolean {
+  return group.rows.length > 0;
+}`;

const SETTINGS_KEYWORDS_PATCH = `@@ -2,2 +2,7 @@
 export function matchesRow(row: SettingsRow, needle: string): boolean {
-  return row.label.toLowerCase().includes(needle) || row.description.toLowerCase().includes(needle);
+  const words = [row.label, row.description, ...(row.keywords ?? [])].join(" ").toLowerCase();
+  return words.includes(needle);
 }
+
+/** Keywords a row answers to besides its own words, such as "dark" for Appearance. */
+export const KEYWORDS: Record<string, string[]> = { appearance: ["dark", "theme"] };`;

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
        known: true,
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
        known: true,
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
        known: true,
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
        // Settings › Harnesses' "Found on this Mac" can be seen and tried.
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
        known: true,
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
        known: true,
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
      // A standard harness that is not installed: it still lists, with its
      // install hint and a switch that cannot be turned on.
      {
        id: "pi",
        name: "Pi",
        kind: "acp",
        builtin: false,
        known: true,
        enabled: false,
        command: ["npx", "-y", "pi-acp"],
        installed: false,
        auth: "unknown",
        login_command: ["pi"],
        install_hint: "npm install -g @earendil-works/pi-coding-agent",
        capabilities: ACP_CAPS,
        accounts: [],
        models: [],
        error: null,
      },
      // A harness the user wrote into config.toml by hand.
      {
        id: "local-agent",
        name: "Local Agent",
        kind: "acp",
        builtin: false,
        known: false,
        enabled: true,
        command: ["local-agent", "--acp"],
        installed: true,
        version: "0.3.1",
        auth: "unknown",
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

/** What a hand-added command lists once it answers ACP. */
const MOCK_CUSTOM_MODELS = [
  { id: "fast", name: "Fast", efforts: [] },
  { id: "deep", name: "Deep", efforts: [] },
];

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
const mockParent = (parent: { id?: string; title?: string; relationship?: string }): Json => ({
  ...(parent.id ? { sessionId: parent.id } : {}),
  ...(parent.title ? { title: parent.title } : {}),
  relationship: parent.relationship ?? "delegate",
});

/**
 * `?scenario=branches`: a thread with a fork and a side chat opened from it, as the engine records
 * them (`_meta.rustcode.parent.relationship`). The fork and the side chat nest under the parent
 * with a glyph, the parent's transcript ends on "Forked to" and "Side chat" lines, and the fork's
 * header links back.
 */
function seedBranchThreads(): MockThread[] {
  const turn = (id: string, ask: string, say: string): JsonObject[] => [
    update(id, { sessionUpdate: "user_message_chunk", content: text(ask) }),
    update(id, { sessionUpdate: "agent_message_chunk", content: text(say) }),
  ];
  const base = {
    cwd: "/Users/you/code/rustcode",
    harness: "rustcode",
    model: "chatgpt/gpt-6-luna",
    effort: "medium",
    checkpoints: [],
  };
  return [
    {
      ...base,
      id: "mock-branch-root",
      title: "Rework the sidebar rows",
      updatedAt: minutes(2),
      history: turn(
        "mock-branch-root",
        "Rework the sidebar rows so a thread's state reads at a glance.",
        "Done: each row now leads with its state, and the project mark moved to the second line.",
      ),
    },
    {
      ...base,
      id: "mock-branch-fork",
      title: "Rework the sidebar rows (fork)",
      updatedAt: minutes(5),
      parent: { id: "mock-branch-root", title: "Rework the sidebar rows", relationship: "fork" },
      history: turn(
        "mock-branch-fork",
        "Rework the sidebar rows so a thread's state reads at a glance.",
        "Done: each row now leads with its state, and the project mark moved to the second line.",
      ),
    },
    {
      ...base,
      id: "mock-branch-side",
      title: "Side chat: Rework the sidebar rows",
      updatedAt: minutes(4),
      parent: { id: "mock-branch-root", title: "Rework the sidebar rows", relationship: "side_chat" },
      history: turn(
        "mock-branch-side",
        "> Done: each row now leads with its state.\n\nWhy the state first?",
        "Because the state is what decides whether you open the row at all.",
      ),
    },
    {
      ...base,
      id: "mock-branch-plain",
      title: "Pick a name for the new command",
      updatedAt: minutes(9),
      history: turn("mock-branch-plain", "What should the new command be called?", "`rustcode data query`."),
    },
  ];
}

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
      events: [
        { kind: "started", at: minutes(3), detail: "Codex · gpt-6-luna" },
        { kind: "working", at: minutes(2) },
      ],
    }),
    link(DELEGATES.research, 1, DELEGATES.tests, "done", minutes(8), {
      harness: "codex",
      harnessName: "Codex",
      settledAt: minutes(6),
      answer: "Added 14 tests: the branch sum, the clock that freezes on an approval, and an unknown cost.",
      filesChanged: 1,
      events: [
        { kind: "started", at: minutes(8), detail: "Codex · gpt-6-luna" },
        { kind: "working", at: minutes(8) },
        { kind: "answered", at: minutes(6), detail: "worked 2m" },
      ],
    }),
    link(DELEGATES.research, 2, DELEGATES.docs, "failed", minutes(7), {
      harness: "claude-code",
      harnessName: "Claude Code",
      settledAt: minutes(6),
      answer: "The docs build failed on an unknown token name.",
      events: [
        { kind: "started", at: minutes(7), detail: "Claude Code · claude-sonnet-6" },
        { kind: "working", at: minutes(7) },
        { kind: "failed", at: minutes(6) },
      ],
    }),
    link(DELEGATES.root, 2, DELEGATES.review, "done", minutes(9), {
      harness: "pi",
      harnessName: "Pi",
      settledAt: minutes(5),
      answer: "Two notes: fold the finished branch, and keep the cost unknown when no turn reported one.",
      filesChanged: 2,
      events: [
        { kind: "started", at: minutes(9), detail: "Pi · deepseek/flash" },
        { kind: "working", at: minutes(9) },
        { kind: "answered", at: minutes(5), detail: "worked 4m" },
      ],
    }),
    // Waiting on the user: the clock stopped when the question was asked.
    link(DELEGATES.root, 3, DELEGATES.wait, "needs_user", minutes(4), {
      harness: "claude-code",
      harnessName: "Claude Code",
      startedAt: minutes(3),
      waitingSince: minutes(2),
      events: [
        { kind: "started", at: minutes(4), detail: "Claude Code · sonnet" },
        { kind: "working", at: minutes(3) },
        { kind: "asking", at: minutes(2) },
      ],
    }),
    link(DELEGATES.root, 4, DELEGATES.stop, "cancelled", minutes(10), {
      harness: "rustcode",
      harnessName: "RustCode",
      settledAt: minutes(9),
      events: [
        { kind: "started", at: minutes(10), detail: "RustCode · chatgpt/gpt-6-luna" },
        { kind: "working", at: minutes(10) },
        { kind: "stopped", at: minutes(9) },
      ],
    }),
  ];
}

/** The quiet notice the app appends for a call its thread's access mode let through (`state/host.ts`). */
const ALLOWED_NOTICE_LINE = "Cleaned up a delegate's worktree · allowed by Full access";

/**
 * `?notices=repeat`: a turn that ended on the same quiet notice six times, as a run of calls its
 * thread's access mode let through leaves it (one notice per call, each an app-side row the engine
 * never sent). The only engine line that draws that quiet row is a finished compaction, so each
 * notice replays as one; the transcript then draws a single row, with its count.
 */
function seedRepeatedNotices(threads: MockThread[]): void {
  const thread = threads.find((t) => t.id === "mock-thread-1");
  if (!thread) return;
  for (let i = 0; i < 6; i++)
    thread.history.push(
      update(thread.id, {
        sessionUpdate: "agent_message_chunk",
        content: text(ALLOWED_NOTICE_LINE),
        _meta: { rustcode: { kind: "compaction", phase: "completed" } },
      }),
    );
}

/**
 * `?tools=background`: a Claude Code turn whose commands went to the background, as its adapter
 * sends them — the call again with the description in `content`, then the task's id inside a
 * ```console fence — so the row can be looked at, and beside it a command fenced the same way.
 */
function seedToolRowsThread(): MockThread {
  const id = "mock-tool-rows";
  const call = (toolCallId: string, title: string, rawInput: JsonObject) => [
    update(id, {
      sessionUpdate: "tool_call",
      toolCallId,
      title: "Terminal",
      kind: "execute",
      status: "in_progress",
      content: [],
      _meta: { claudeCode: { toolName: "Bash" } },
    }),
    update(id, { sessionUpdate: "tool_call_update", toolCallId, title }),
    // The call again, with the description it was given: never the output.
    update(id, {
      sessionUpdate: "tool_call_update",
      toolCallId,
      rawInput,
      content: [{ type: "content", content: { type: "text", text: String(rawInput.description ?? "") } }],
    }),
  ];
  return {
    id,
    cwd: "/Users/you/code/rustcode-mobile",
    title: "Install the phone build",
    updatedAt: minutes(1),
    harness: "claude-code",
    model: "sonnet",
    effort: "medium",
    checkpoints: [],
    history: [
      update(id, {
        sessionUpdate: "user_message_chunk",
        content: text("Build the Release app on my iPhone, and run the checks while it builds."),
      }),
      ...call("toolu_bg", "cd ~/code/rustcode-mobile && CI=1 bunx expo run:ios --configuration Release", {
        command: "cd ~/code/rustcode-mobile && CI=1 bunx expo run:ios --configuration Release",
        description: "Build the Release app and install it on the iPhone",
        run_in_background: true,
      }),
      update(id, {
        sessionUpdate: "tool_call_update",
        toolCallId: "toolu_bg",
        status: "completed",
        content: [
          {
            type: "content",
            content: {
              type: "text",
              text: "```console\nCommand running in background with ID: bc82qcgu4. Output is being written to: /private/tmp/claude-501/-Users-you-code-rustcode-mobile/tasks/bc82qcgu4.output. You will be notified when it completes. To check interim output, use Read on that file path.\n```",
            },
          },
        ],
        _meta: { jetbrains: { air: { asyncTasks: { backgrounded: true } } } },
      }),
      ...call("toolu_test", "bun run check", { command: "bun run check", description: "Run the checks" }),
      update(id, {
        sessionUpdate: "tool_call_update",
        toolCallId: "toolu_test",
        status: "completed",
        rawOutput: {
          content:
            "```console\n$ bun run check\n$ tsc --noEmit && biome check src\nChecked 214 files in 380ms. No fixes applied.\n```",
          exitCode: 0,
        },
      }),
      update(id, {
        sessionUpdate: "agent_message_chunk",
        content: text(
          "The build runs in the background and the checks pass. I'll report when the app is on the phone.",
        ),
        _meta: {
          rustcode: {
            turn: {
              id: "t1",
              elapsedMs: 12_000,
              outcome: "completed",
              endedAt: new Date(minutes(1)).toISOString(),
            },
          },
        },
      }),
    ],
  };
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
      // Three turns, one checkpoint each: rewinding to one of them keeps the later turns as an
      // earlier attempt (state/attempts.ts), so the mock can show the whole of it.
      id: "mock-thread-attempts",
      cwd: "/Users/you/code/rustcode",
      title: "Give the settings panel a search field",
      updatedAt: minutes(18),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      checkpoints: [
        {
          turn: 1,
          prompt: "Add a search field to the settings panel",
          created_at: new Date(minutes(24)).toISOString(),
          files: [
            {
              path: "src/ui/Settings.tsx",
              status: "modified",
              insertions: 12,
              deletions: 1,
              patch: SETTINGS_SEARCH_PATCH,
            },
          ],
        },
        {
          turn: 2,
          prompt: "Match the section names too, and keep the list open",
          created_at: new Date(minutes(22)).toISOString(),
          files: [
            {
              path: "src/ui/Settings.tsx",
              status: "modified",
              insertions: 6,
              deletions: 4,
              patch: SETTINGS_FILTER_PATCH,
            },
            {
              path: "src/ui/SettingsSearch.ts",
              status: "added",
              insertions: 34,
              deletions: 0,
              patch: SETTINGS_HELPER_PATCH,
            },
          ],
        },
        {
          turn: 3,
          prompt: "Also search what each setting does, not only its name",
          created_at: new Date(minutes(20)).toISOString(),
          files: [
            {
              path: "src/ui/SettingsSearch.ts",
              status: "modified",
              insertions: 9,
              deletions: 2,
              patch: SETTINGS_KEYWORDS_PATCH,
            },
          ],
        },
      ],
      history: [
        update("mock-thread-attempts", {
          sessionUpdate: "user_message_chunk",
          content: text("Add a search field to the settings panel"),
        }),
        update("mock-thread-attempts", {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "Added a field above the panel that filters the sections as you type, and clears with Escape.",
          ),
          _meta: {
            rustcode: {
              turn: {
                id: "t1",
                elapsedMs: 41_000,
                outcome: "completed",
                endedAt: new Date(minutes(23)).toISOString(),
              },
            },
          },
        }),
        update("mock-thread-attempts", {
          sessionUpdate: "user_message_chunk",
          content: text("Match the section names too, and keep the list open"),
        }),
        update("mock-thread-attempts", {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "It now matches section headings as well, and a group stays open when one of its rows answers the query.",
          ),
          _meta: {
            rustcode: {
              turn: {
                id: "t2",
                elapsedMs: 63_000,
                outcome: "completed",
                endedAt: new Date(minutes(21)).toISOString(),
              },
            },
          },
        }),
        update("mock-thread-attempts", {
          sessionUpdate: "user_message_chunk",
          content: text("Also search what each setting does, not only its name"),
        }),
        update("mock-thread-attempts", {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "Each setting carries a few keywords now, so `dark`, `notify` and `token` all find their row.",
          ),
          _meta: {
            rustcode: {
              turn: {
                id: "t3",
                elapsedMs: 28_000,
                outcome: "completed",
                endedAt: new Date(minutes(19)).toISOString(),
              },
            },
          },
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
    {
      id: "mock-agent-images",
      cwd: "/Users/you/code/rustcode",
      title: "Review agent screenshots",
      model: "gpt-6-luna",
      effort: "low",
      updatedAt: minutes(90),
      harness: "codex",
      checkpoints: [],
      history: [
        update("mock-agent-images", {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "![Temp screenshot](/tmp/rustcode/mock-agent-images/screenshot.png)\n\n![Blocked screenshot](/Users/you/Private/screenshot.png)",
          ),
          ...turn(2000),
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

/** The days in the last month a mock thread worked, so the daily chart has gaps and variation. */
const MOCK_DAY_OFFSETS = [0, 1, 1, 3, 6, 11, 18, 24];

function mockUsageDays(index: number): string[] {
  const count = 1 + (index % 3);
  const days = new Set<string>();
  for (let step = 0; step < count; step++) {
    const offset = MOCK_DAY_OFFSETS[(index + step * 3) % MOCK_DAY_OFFSETS.length] ?? 0;
    days.add(new Date(Date.now() - offset * 86_400_000).toISOString().slice(0, 10));
  }
  return [...days];
}

/** A thread's spend split over the days it worked, so a window of days keeps what it holds. */
function mockUsageParts(spend: MockSpend, index: number): { day: string; spend: MockSpend }[] {
  const days = mockUsageDays(index);
  const parts = days.length;
  // Counts are whole; the last part takes the remainder so the parts add up to the thread.
  const count = (value: number | null, at: number): number | null =>
    value === null
      ? null
      : at === parts - 1
        ? value - Math.floor(value / parts) * (parts - 1)
        : Math.floor(value / parts);
  // A cost is a fraction of a dollar: share it evenly.
  const usd = (value: number | null): number | null => (value === null ? null : value / parts);
  return days.map((day, at) => ({
    day,
    spend: {
      input: count(spend.input, at)!,
      output: count(spend.output, at)!,
      cached: count(spend.cached, at)!,
      cacheWrite: count(spend.cacheWrite, at),
      cost: usd(spend.cost),
      turns: count(spend.turns, at)!,
      withoutTokens: count(spend.withoutTokens, at)!,
      withoutAny: count(spend.withoutAny, at)!,
      withoutInput: count(spend.withoutInput, at)!,
      withoutOutput: count(spend.withoutOutput, at)!,
      withoutCached: count(spend.withoutCached, at)!,
      withoutCacheWrite: count(spend.withoutCacheWrite, at)!,
      withoutCost: count(spend.withoutCost, at)!,
    },
  }));
}

/**
 * `_rustcode/data/usage`, grouped like the engine groups the store
 * (`engine/src/acp/data.rs`). `since` keeps only the days it names, as the
 * engine's own comparison does, so the period switch has something to switch.
 */
function mockUsage(threads: MockThread[], groupBy: string, since?: string): JsonObject {
  const groups = new Map<string, { keys: JsonObject; spends: MockSpend[] }>();
  for (const [index, thread] of threads.entries()) {
    const parts = mockUsageParts(spendOf(thread, index), index).filter((part) => !since || part.day >= since);
    if (!parts.length) continue;
    if (groupBy === "day") {
      for (const part of parts) {
        const group = groups.get(part.day) ?? { keys: { day: part.day }, spends: [] };
        group.spends.push(part.spend);
        groups.set(part.day, group);
      }
      continue;
    }
    let key: string;
    let keys: JsonObject;
    switch (groupBy) {
      case "model":
        key = `${thread.harness}\u0000${thread.model}`;
        keys = { harness: thread.harness, model: thread.model };
        break;
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
    for (const part of parts) group.spends.push(part.spend);
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

/**
 * What the mock says fills its own context: enough per category, and enough per item, to show the
 * card's tile map and its item list the way the engine's breakdown does. `used` scales the canned
 * categories so the bar, the tiles and the headline agree.
 */
const MOCK_CONTEXT: Json[] = [
  {
    name: "User messages",
    tokens: 1_900,
    items: [
      { name: "Make the context card show what fills it", tokens: 1_200 },
      { name: "Add a tile map and a compact comparison", tokens: 700 },
    ],
  },
  {
    name: "Agent responses",
    tokens: 3_400,
    items: [
      { name: "I will trace the context breakdown through the engine", tokens: 2_100 },
      { name: "The mock now reports per-category items as well", tokens: 1_300 },
    ],
  },
  {
    name: "Tool calls",
    tokens: 5_100,
    items: [
      { name: "read src/ui/ComposerUsage.tsx", tokens: 2_200 },
      { name: "replace_file_content src/ui/ComposerUsage.tsx", tokens: 1_600 },
      { name: "run_command cargo test", tokens: 800 },
      { name: "read src/lib/treemap.ts", tokens: 500 },
    ],
  },
  { name: "Compacted history", tokens: 0, items: [] },
  {
    name: "System prompt",
    tokens: 4_200,
    items: [{ name: "Instructions and hints", tokens: 4_200 }],
  },
  {
    name: "System tools",
    tokens: 2_900,
    items: [
      { name: "replace_file_content", tokens: 1_100 },
      { name: "run_command", tokens: 800 },
      { name: "view_file", tokens: 600 },
      { name: "Tool protocol and examples", tokens: 400 },
    ],
  },
  {
    name: "Skills",
    tokens: 900,
    items: [
      { name: "impeccable", tokens: 500 },
      { name: "Skill headings", tokens: 400 },
    ],
  },
  { name: "Subagents", tokens: 0, items: [] },
];

/** The same categories after a compaction: tool output is gone, a summary took its place. */
const MOCK_CONTEXT_COMPACTED: Json[] = [
  {
    name: "User messages",
    tokens: 1_200,
    items: [{ name: "Make the context card show what fills it", tokens: 1_200 }],
  },
  {
    name: "Agent responses",
    tokens: 1_500,
    items: [{ name: "I will trace the context breakdown through the engine", tokens: 1_500 }],
  },
  {
    name: "Tool calls",
    tokens: 1_800,
    items: [
      { name: "read src/ui/ComposerUsage.tsx", tokens: 1_100 },
      { name: "run_command cargo test", tokens: 700 },
    ],
  },
  {
    name: "Compacted history",
    tokens: 3_000,
    items: [{ name: "Compaction summary", tokens: 3_000 }],
  },
  {
    name: "System prompt",
    tokens: 4_200,
    items: [{ name: "Instructions and hints", tokens: 4_200 }],
  },
  {
    name: "System tools",
    tokens: 2_900,
    items: [{ name: "replace_file_content", tokens: 2_900 }],
  },
  { name: "Skills", tokens: 900, items: [{ name: "impeccable", tokens: 900 }] },
  { name: "Subagents", tokens: 0, items: [] },
];

/** The context block the engine puts on `usage_update`, scaled to the headline `used`. */
function mockContext(
  used: number,
  compaction?: { count: number; reclaimed: number; before: number },
): JsonObject {
  const base = compaction ? MOCK_CONTEXT_COMPACTED : MOCK_CONTEXT;
  const total = base.reduce<number>((sum, category) => sum + Number(obj(category)?.tokens ?? 0), 0);
  const scale = total > 0 ? used / total : 1;
  const categories = base.map((category) => {
    const fields = obj(category) ?? {};
    return {
      ...fields,
      tokens: Math.round(Number(fields.tokens ?? 0) * scale),
      items: arr(fields.items).map((item) => {
        const entry = obj(item) ?? {};
        return { ...entry, tokens: Math.round(Number(entry.tokens ?? 0) * scale) };
      }),
    };
  });
  return {
    model: "gpt-6-luna",
    window: 272_000,
    used,
    free: 272_000 - used,
    compactAt: { tokens: 199_000 },
    categories,
    ...(compaction
      ? {
          compaction: {
            count: compaction.count,
            reclaimedTokens: compaction.reclaimed,
            beforeTokens: compaction.before,
            afterTokens: used,
          },
        }
      : {}),
  };
}

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
  private pullEventDelivered = false;
  /** The client's `_meta.rustcode.orchestration` at initialize. */
  private orchestration = 0;
  /** Discord Rich Presence (docs/discord-rich-presence.md), as `discord_rpc_enabled` holds it. */
  private presence = true;
  /** The server settings of this machine (`_rustcode/settings`, docs/configuration.md). */
  private serverSettings: JsonObject = {
    "default-permission-mode": "ask",
    "resume-after-restart": true,
    "continue-after-limit": true,
    "auto-compact-mode": "size",
    "auto-compact-threshold": 150_000,
    orchestration: true,
    "delegates-total": 8,
    "delegates-per-thread": 4,
    "read-across-projects": true,
  };
  /** The payload-capture switches (docs/data-store.md › Captured payloads), off as `config.toml` defaults them. */
  private payloadCapture = { providerPayloads: false, harnessRpcPayloads: false };
  /**
  /**
   * Remote access (docs/remote.md), as the engine's gateway reports it: a
   * machine-wide switch and gateway, the threads this app shared (whatever
   * harness runs them), one paired phone, and the pairing offer while it
   * lasts. `refused` holds the threads the host turns down and why, which the
   * machine's answer counts as `notShared` and a thread's answer names.
   */
  private remote = {
    enabled: false,
    shared: new Set<string>(),
    gateway: null as JsonObject | null,
    devices: [
      {
        id: "7c1f3a2b",
        name: "Lars's iPhone",
        kind: "phone",
        platform: "ios",
        lastSeenAt: new Date(Date.now() - 2 * 3_600_000).toISOString(),
      },
    ] as JsonObject[],
    offer: null as JsonObject | null,
    pairingDue: false,
    pairingError: null as string | null,
    refused: new Map<string, string>(),
    /** Connections the gateway turned away, newest first (`refusedConnections`). */
    refusedConnections: [] as JsonObject[],
    /** Phones asking to connect, as the gateway holds them (`remote_gateway/connect.rs`). */
    connect: [] as MockConnectRequest[],
    /** Requests that ended, oldest first, as the gateway keeps its last eight. */
    endedConnect: [] as { id: string; deviceName: string; end: string; deviceId?: string; at: number }[],
    /** The open connect code for another computer, apart from the challenge (`pairing.rs`). */
    machineOffer: null as { serial: number; expiresAt: number } | null,
    machineSerial: 0,
  };
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
  /**
   * The timed approvals `_rustcode/permissions` lists: one seeded hour so the
   * details panel has "Allowed for 42m · Revoke" to show, and whatever the user
   * granted while the mock ran.
   */
  private grants: JsonObject[] = [
    {
      id: "prefix-v1:bun test",
      prefix: "bun test",
      expiresAt: Math.floor(Date.now() / 1000) + 42 * 60,
      remainingSecs: 42 * 60,
    },
  ];

  /** A timed approval the user just gave in the mock: listed and revocable like the engine's. */
  private addGrant(prefix: string) {
    const remainingSecs = 60 * 60;
    const id = `prefix-v1:${prefix}`;
    this.grants = [
      { id, prefix, expiresAt: Math.floor(Date.now() / 1000) + remainingSecs, remainingSecs },
      ...this.grants.filter((grant) => str(grant.id) !== id),
    ];
  }

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
            : scenario === "branches"
              ? seedBranchThreads()
              : [];
    // The stored book is the only one the app reads at connect; a scenario brings its own.
    if (options.notices === "repeat") seedRepeatedNotices(this.threads);
    if (scenario === "delegates") {
      this.options.links ??= {
        links: seedDelegateLinks() as unknown as Json,
        grants: {},
        counters: {},
        fanOuts: [],
      };
    }
    // A thread of its own for a tool row's hard cases, in front of the scenario's own.
    if (options.tools === "background") this.threads = [seedToolRowsThread(), ...this.threads];
    this.harnesses = withLast(initialHarnesses(scenario !== "onboarding"));
    if (options.remote === "connect") {
      this.remote.enabled = true;
      this.remote.gateway = this.remoteGateway();
      for (const open of this.threads) this.remote.shared.add(open.id);
      this.askToConnect("Lars's Pixel", "android", "192.168.1.31");
    }
    if (options.remote === "refused") {
      this.remote.enabled = true;
      this.remote.gateway = this.remoteGateway();
      for (const open of this.threads) this.remote.shared.add(open.id);
      // One of each refusal the owner can act on, newest first, as the gateway
      // keeps them (`remote_gateway/gateway.rs`).
      this.remote.refusedConnections = [
        {
          reason: "unsupported_version",
          attempts: 3,
          deviceName: "Lars's iPhone",
          deviceKind: "phone",
          platform: "ios",
          // The phone build from #3301: a protocol version this Mac does not speak.
          protocolVersion: 0,
          supportedVersions: [1],
          remoteAddr: "192.168.1.31",
          refusedSecsAgo: 40,
        },
        {
          reason: "pairing_failed",
          attempts: 1,
          deviceName: "Pixel 8",
          deviceKind: "phone",
          platform: "android",
          supportedVersions: [],
          remoteAddr: "192.168.1.72",
          refusedSecsAgo: 190,
        },
        {
          reason: "unauthorized",
          attempts: 1,
          deviceKind: "phone",
          platform: "android",
          supportedVersions: [],
          remoteAddr: "192.168.1.44",
          refusedSecsAgo: 320,
        },
        {
          reason: "unexpected_origin",
          attempts: 2,
          deviceKind: "phone",
          platform: "unknown",
          supportedVersions: [],
          remoteAddr: "192.168.1.72",
          refusedSecsAgo: 500,
        },
      ];
    }
  }

  /** A phone (or a desktop app) on the network asks to connect, as `connect_request` does at the gateway. */
  private askToConnect(
    deviceName: string,
    platform: string,
    remoteAddr: string,
    deviceKind = "phone",
  ): string {
    const id = Math.random().toString(16).slice(2, 18).padEnd(16, "0");
    const now = Date.now();
    this.remote.connect.push({
      id,
      deviceName,
      deviceKind,
      platform,
      remoteAddr,
      createdAt: now,
      expiresAt: now + 120_000,
      state: "pending",
    });
    return id;
  }

  private endConnect(id: string, end: string, deviceId?: string) {
    const request = this.remote.connect.find((entry) => entry.id === id);
    if (!request) return;
    this.remote.connect = this.remote.connect.filter((entry) => entry !== request);
    this.remote.endedConnect.push({ id, deviceName: request.deviceName, end, deviceId, at: Date.now() });
    this.remote.endedConnect = this.remote.endedConnect.slice(-8);
  }

  /** The phone enters the code the app showed: it pairs, as a new device. */
  private enterConnectCode(): boolean {
    const request = this.remote.connect.find((entry) => entry.state === "approved");
    if (!request) return false;
    const deviceId = Math.random().toString(16).slice(2, 10).padEnd(8, "0");
    this.remote.devices.push({
      id: deviceId,
      name: request.deviceName,
      kind: request.deviceKind,
      platform: request.platform,
      lastSeenAt: new Date().toISOString(),
    });
    this.endConnect(request.id, "paired", deviceId);
    return true;
  }

  private connectRequestsJson(): JsonObject {
    const now = Date.now();
    for (const request of [...this.remote.connect]) {
      if (request.expiresAt <= now) this.endConnect(request.id, "expired");
    }
    return {
      connectRequests: this.remote.connect.map((request) => ({
        id: request.id,
        deviceName: request.deviceName,
        deviceKind: request.deviceKind,
        platform: request.platform,
        remoteAddr: request.remoteAddr,
        ageSecs: Math.floor((now - request.createdAt) / 1000),
        expiresInSecs: Math.max(0, Math.floor((request.expiresAt - now) / 1000)),
        state: request.state,
      })),
      endedConnectRequests: this.remote.endedConnect.map((ended) => {
        const entry: JsonObject = {
          id: ended.id,
          deviceName: ended.deviceName,
          end: ended.end,
          endedSecsAgo: Math.floor((now - ended.at) / 1000),
        };
        if (ended.deviceId) entry.deviceId = ended.deviceId;
        return entry;
      }),
    };
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
    // One of each risk level, so the card's tint and preview are visible in the mock:
    // a read-only call, an edit with a diff, and a command that would destroy a tree.
    if (has("mock-thread-1")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-1",
        toolCall: {
          toolCallId: "seed-approval-1",
          title: "read src/ui/Composer.tsx",
          kind: "read",
          rawInput: {
            path: "src/ui/Composer.tsx",
            summary: "Read the composer before changing it",
          },
        },
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          {
            optionId: "allow_hour",
            name: "Allow for 1 hour `cat src/ui/Composer.tsx`",
            kind: "allow_always",
          },
          { optionId: "always", name: "Always allow `cat src/ui/Composer.tsx`", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      }).then((answer) => {
        // An hour granted here is listed and revocable, like the engine's own.
        if (obj(answer.outcome)?.optionId === "allow_hour") this.addGrant("cat src/ui/Composer.tsx");
      });
    }
    if (has("mock-thread-4")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-4",
        toolCall: {
          toolCallId: "seed-approval-destructive",
          title: "bash",
          kind: "execute",
          rawInput: {
            command: "rm -rf node_modules && bun install",
            cwd: "/Users/you/.config/rustcode/worktrees/rustcode/feat-login-form",
            summary: "Start the dependencies over",
          },
        },
        // A harness that offers grants for a destructive command: the app must
        // show only Allow once and Reject (lib/approval.ts `grantAllowed`).
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "allow_hour", name: "Allow for 1 hour `rm -rf node_modules`", kind: "allow_always" },
          { optionId: "always", name: "Always allow `rm -rf node_modules`", kind: "allow_always" },
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
          content: [
            {
              type: "diff",
              path: "src/ui/LoginForm.tsx",
              oldText: '  <button type="submit">Sign in</button>',
              newText: '  <button type="submit" disabled={!email}>Sign in</button>',
              _meta: { rustcode: { oldStart: 8, newStart: 8 } },
            },
          ],
          rawInput: { path: "src/ui/LoginForm.tsx", summary: "Disable sign-in until the email is typed" },
        },
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
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
      (error: Error & { code?: number; data?: Json }) => {
        // `data` travels as the engine sends it: the refusals an orchestration
        // tool maps (`{ reason, message }`, `error.data.reason`) are read there.
        if (id !== undefined)
          this.send({
            id,
            error: {
              code: error.code ?? -32603,
              message: error.message,
              ...(error.data === undefined ? {} : { data: error.data }),
            },
          });
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

  /**
   * The gateway as the engine names it: the phones connected to it and the
   * ones attached to the thread being read, both by name (arrays, as
   * `acp/remote.rs` sends them), and whether anything is connected at all.
   */
  private remoteGateway(): JsonObject {
    const connected = this.options.remote !== "disconnected";
    const phones = connected ? this.remote.devices.map((device) => str(device.name) ?? "phone") : [];
    return {
      connected,
      instanceId: "mock-gateway-instance-1",
      advertisedAddress: "192.168.1.24:17879",
      listenAddress: "192.168.1.24:17879",
      loopbackOnly: false,
      attachedDevices: phones,
      connectedDevices: phones,
      pairedDevices: this.remote.devices.length,
    };
  }

  /** The pairing challenge as the engine's offer: the rows are drawn, the payload is not scannable in a mock. */
  private remoteOffer(): JsonObject | null {
    const stored = obj(this.remote.offer);
    if (!stored) return null;
    const expiresAt = num(stored.expiresAt) ?? 0;
    if (expiresAt <= Date.now()) return null;
    const offer: JsonObject = {
      qrPayload: str(stored.qrPayload) ?? "",
      qrRows: mockQrRows(),
      code: str(stored.code) ?? "",
      address: str(stored.address) ?? "",
      alternates: arr(stored.alternates),
      expiresInSecs: Math.max(0, Math.ceil((expiresAt - Date.now()) / 1000)),
      loopbackOnly: stored.loopbackOnly === true,
      gatewayId: str(stored.gatewayId) ?? "",
      instanceId: str(stored.instanceId) ?? "mock-instance",
      challenge: num(stored.challenge) ?? 0,
    };
    const lockedFor = num(stored.lockedForSecs);
    if (lockedFor !== undefined) offer.lockedForSecs = lockedFor;
    return offer;
  }

  /** The machine's answer: the switch, the gateway, the open challenge and the paired devices. */
  private remoteHost(): JsonObject {
    const stored = obj(this.remote.offer);
    const expiresAt = num(stored?.expiresAt) ?? 0;
    const offer = this.remoteOffer();
    const host: JsonObject = {
      available: true,
      enabled: this.remote.enabled,
      shared: false,
      pairingPending: this.remote.pairingDue,
      pairingExpired: stored !== undefined && expiresAt <= Date.now(),
      pairingError: this.remote.pairingError,
      notShared: this.remote.refused.size,
      pairedDevices: this.remote.devices.length,
      refusedConnections: this.remote.refusedConnections,
      // The machine's answer says which paired devices are connected; the mock's gateway has them all.
      devices: this.remote.devices.map((device) => ({
        ...device,
        connected: this.remote.gateway !== null && this.options.remote !== "disconnected",
      })),
      ...this.connectRequestsJson(),
    };
    if (this.remote.gateway) {
      const gateway: JsonObject = {
        ...this.remote.gateway,
        instanceId: "mock-instance",
        pairedDevices: this.remote.devices.length,
        pairingOpen: offer !== null,
        sessions: this.remote.shared.size,
      };
      const machineOffer = this.remote.machineOffer;
      gateway.machineOffer = {
        open:
          machineOffer && machineOffer.expiresAt > Date.now()
            ? {
                serial: machineOffer.serial,
                expiresInSecs: Math.ceil((machineOffer.expiresAt - Date.now()) / 1000),
                attemptsLeft: 5,
              }
            : null,
        lastEnded: null,
      };
      if (offer) {
        gateway.pairing = {
          expiresInSecs: num(offer.expiresInSecs) ?? 0,
          attemptsLeft: num(stored?.attemptsLeft) ?? 5,
          challenge: num(offer.challenge) ?? 0,
        };
      }
      host.gateway = gateway;
    }
    return host;
  }

  /**
   * The answer `_rustcode/remote` gives: with no session, the machine's; with
   * one it shares, that thread's; with one it does not, the machine's plus the
   * thread and why the host holds it back (`notShared`), as the engine does.
   */
  private remoteStatus(sessionId: string): Json {
    const shared = sessionId !== "" && this.remote.shared.has(sessionId);
    if (!shared) {
      const host = this.remoteHost();
      if (!sessionId) return host;
      const held: JsonObject = { ...host, sessionId };
      const reason = this.remote.refused.get(sessionId);
      if (reason !== undefined) held.notShared = reason;
      return held as Json;
    }
    const stored = obj(this.remote.offer);
    const expiresAt = num(stored?.expiresAt) ?? 0;
    const share: JsonObject = {
      available: true,
      enabled: this.remote.enabled,
      shared: true,
      sessionId,
      target: this.thread(sessionId).harness,
      registrationEpoch: 1,
      offer: this.remoteOffer(),
      pairingPending: this.remote.pairingDue,
      pairingExpired: stored !== undefined && expiresAt <= Date.now(),
      pairedDevices: this.remote.devices.length,
      devices: this.remote.devices,
    };
    if (this.remote.gateway) share.gateway = this.remoteGateway();
    if (this.remote.pairingError) share.pairingError = this.remote.pairingError;
    return share as Json;
  }

  /** A fresh pairing challenge: the QR payload carries the credential, the code is typed by hand. */
  private issueRemoteOffer() {
    const address = "192.168.1.24:17879";
    const alternates = ["100.90.16.237:17879"];
    this.remote.gateway = this.remoteGateway();
    this.remote.offer = {
      qrPayload: JSON.stringify({
        protocol_version: 1,
        address,
        gateway_id: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
        credential: "jH8sK2mQ9xT4vLpR6nB1yC3fW7gZ0aE5uI2oP8sD4qX",
        host_name: "Mock MacBook Pro",
        alternates,
      }),
      code: "4927-3318",
      address,
      alternates,
      expiresAt: Date.now() + (this.options.remote === "expired" ? -1_000 : 120_000),
      loopbackOnly: false,
      gatewayId: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
      challenge: 17,
      attemptsLeft: 5,
    };
    this.remote.pairingDue = false;
    this.remote.pairingError = null;
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
              rustcode: {
                steer: 1,
                agents: 2,
                rewind: 1,
                pullRequests: 3,
                sessionMove: 1,
                sessionRepair: 1,
                checkpoints: 2,
                lineage: 1,
              },
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
                  // The project's git remote, when its machine reports one: what finds the same
                  // project on another computer (state/projects.ts).
                  ...(mockRemote(t.cwd) ? { remote: mockRemote(t.cwd) } : {}),
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
        const relationship = str(meta?.relationship) ?? "delegate";
        const thread: MockThread = {
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          cwd: worktree?.path ?? str(params.cwd) ?? "/",
          worktree,
          checkpoints: [],
          updatedAt: Date.now(),
          harness,
          ...(account ? { account } : {}),
          ...(parent ? { parent: { id: parent, title: parentTitle, relationship } } : {}),
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
                context: mockContext(18_400),
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
        const ask = params as { groupBy?: string; since?: string } | undefined;
        return mockUsage(this.threads, ask?.groupBy ?? "harness", ask?.since);
      }
      case "_rustcode/data/query":
        if (String(obj(params)?.sql ?? "").includes("operational_events")) {
          const at = (ago: number) => new Date(Date.now() - ago).toISOString();
          return {
            columns: ["at", "event_type", "thread_id", "fields_json"],
            rows: [
              {
                at: at(40_000),
                event_type: "turn.summary",
                thread_id: "t-login",
                fields_json: '{"rounds":6,"tool_calls":11}',
              },
              {
                at: at(95_000),
                event_type: "tools.batch.finish",
                thread_id: "t-login",
                fields_json: '{"calls":3,"failed":1}',
              },
              {
                at: at(120_000),
                event_type: "turn.start",
                thread_id: "t-login",
                fields_json: '{"wakeup":false}',
              },
            ],
            truncated: false,
          };
        }
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
        const items: JsonObject[] = [
          { source: "event", at, type: "turn.start", fields: { tool_rounds: 0 } },
          {
            source: "provider_request",
            at,
            provider: "openai",
            model: "gpt-6-luna",
            status_code: 429,
            error_kind: "rate_limit",
            latency_ms: 412,
            input_tokens: 1200,
            output_tokens: 80,
            attempt: 1,
          },
        ];
        if (params.order === "newest") items.reverse();
        return {
          session_id: str(params.sessionId) ?? "",
          items,
          next: null,
          has_more: false,
          last_provider_error: { at, status_code: 429, error_kind: "rate_limit" },
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
        // A lapsed access token that RustCode answered with the numbers it last
        // read for the login: the row draws them, marked with when.
        const recorded: JsonObject =
          failure === "recorded" ? { asOf: Math.floor(Date.now() / 1000) - 3 * 3600 } : {};
        const failed = (reason: Exclude<NonNullable<MockOptions["claudeUsage"]>, "recorded">) => ({
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          status: "unavailable",
          reason,
          error:
            reason === "expired"
              ? "Run a Claude Code thread on this account to refresh its usage."
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
        // Naming an account is independent of `claude`, so a background read of
        // one account leaves the default login unread (engine `acp/usage.rs`).
        const accounts = [
          {
            id: "paral-2",
            name: "Paral 2",
            windows: [
              { label: "5-hour", usedPercent: 4, windowMinutes: 300, resetsAt: at(4) },
              { label: "Weekly", usedPercent: 12, windowMinutes: 10080, resetsAt: at(90) },
            ],
          },
        ].filter((a) => (p?.claude && p?.claudeAccounts) || a.id === p?.claudeAccount);
        return {
          plans: [
            p?.claude
              ? failure && failure !== "recorded"
                ? failed(failure)
                : {
                    id: "claude",
                    name: "Claude",
                    via: "Claude Code",
                    status: "ok",
                    plan: "max",
                    ...recorded,
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
              ...recorded,
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
      case "_rustcode/pulls/watches": {
        const thread = this.threads[0];
        if (!thread || typeof location === "undefined" || !location.search.includes("pullWatchPreview"))
          return { watches: [] };
        const watch = { sessionId: thread.id, number: 3375, link: true };
        if (obj(params)?.take) {
          const events = this.pullEventDelivered
            ? []
            : [{ message: "CI checks failed", kind: "checks_failed" }];
          this.pullEventDelivered = true;
          return { watch, events };
        }
        return { watches: [watch] };
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
              {
                number: 2795,
                title: "feat(desktop): add the login form",
                author: "LHagfoss",
                state: "open",
                draft: false,
                review: "changes_requested",
                checks: { passed: 4, failed: 1, pending: 0, skipped: 0 },
                headRef: "feat/login-form",
                baseRef: "main",
                headSha: "58961fc5aa",
                updatedAt: new Date(minutes(30)).toISOString(),
                url: "https://github.com/LHagfoss/rustcode/pull/2795",
                additions: 210,
                deletions: 4,
                changedFiles: 5,
              },
            ],
          },
        };
      case "_rustcode/project/read": {
        // The engine's `_rustcode/project/read`: a folder's git facts.
        const path = str(obj(params)?.path) || "/Users/me/code/rustcode";
        return {
          path,
          name: path.split("/").filter(Boolean).pop() ?? path,
          is_git: true,
          remote: "https://github.com/LHagfoss/rustcode.git",
          branch: "main",
          default_branch: "main",
        };
      }
      case "_rustcode/project/create": {
        const path = str(obj(params)?.path) || "/Users/me/code/new-project";
        const init = obj(params)?.init !== false;
        return {
          path,
          name: path.split("/").filter(Boolean).pop() ?? path,
          is_git: init,
          remote: null,
          branch: init ? "main" : null,
          default_branch: init ? "main" : null,
          created: true,
          initialized: init,
        };
      }
      case "_rustcode/project/clone": {
        const ask = obj(params) ?? {};
        const parent = str(ask.parent) || "/Users/me/code";
        const name =
          str(ask.name) ||
          String(str(ask.url) ?? "cloned")
            .replace(/\/+$/, "")
            .split(/[/:]/)
            .pop()!
            .replace(/\.git$/, "");
        return {
          path: `${parent}/${name}`,
          name,
          is_git: true,
          remote: str(ask.url) ?? null,
          branch: "main",
          default_branch: "main",
          created: true,
        };
      }
      case "_rustcode/scheduled/list":
        return {
          daemon_running: true,
          tasks: [
            {
              id: "task-morning",
              name: "Morning build check",
              paused: false,
              schedule: { kind: "cron", expression: "0 9 * * 1-5", timezone: "Europe/Oslo" },
              workspace: "/Users/me/code/rustcode",
              next_due_at: new Date(new Date().setHours(9, 0, 0, 0) + 86_400_000).toISOString(),
            },
          ],
        };
      case "_rustcode/scheduled/create": {
        const ask = obj(params) ?? {};
        const prompt = str(ask.prompt) ?? "Scheduled work";
        return {
          daemon_running: true,
          task: {
            id: `task-${this.counter}`,
            name: str(ask.name) || prompt.split("\n")[0]!.slice(0, 60),
            paused: false,
            schedule: ask.schedule ?? { kind: "cron", expression: "0 9 * * *" },
            workspace: str(ask.workspace) ?? "",
            next_due_at: new Date(Date.now() + 86_400_000).toISOString(),
          },
        };
      }
      case "_rustcode/scheduled/delete":
        return { id: str(obj(params)?.id) ?? "", deleted: true };
      case "_rustcode/scheduled/run":
        return { id: str(obj(params)?.id) ?? "", state: "claimed" };
      case "_rustcode/pulls/state": {
        // A project whose remote is on another forge: `gh` knows no pull requests there.
        if (str(obj(params)?.cwd)?.includes("/gitlab-")) {
          return {
            error: {
              code: "no_provider",
              message: "none of the git remotes configured for this repository point to a known GitHub host",
            },
          };
        }
        const branch = str(obj(params)?.branch) || "feat/delegate-pull";
        return {
          branch,
          pull: {
            number: 2790,
            title: "feat(mcp): report and finish delegate pull requests",
            state: "open",
            merged: false,
            draft: false,
            url: "https://github.com/LHagfoss/rustcode/pull/2790",
            branch,
            headSha: "58961fc5aa",
            ci: "pending",
            failing: [],
            pending: 1,
            passed: 5,
          },
        };
      }
      case "_rustcode/pulls/merge/cancel":
        return { cancelled: false };
      case "_rustcode/pulls/merge":
        return {
          merge: {
            number: num(obj(params)?.number) ?? 2790,
            state: "merged",
            merged: true,
            failing: [],
            url: "https://github.com/LHagfoss/rustcode/pull/2790",
            waitedSecs: 42,
            message: "squash-merged and the branch was deleted",
          },
        };
      case "_rustcode/worktrees/cleanup":
        return {
          cleanup: {
            removed: true,
            branch: str(obj(params)?.branch) ?? "feat/delegate-pull",
            branch_removed: true,
            base: "main",
            commit_count: 0,
          },
        };
      case "_rustcode/worktrees/state": {
        const paths = Array.isArray(params.paths) ? params.paths : [];
        return {
          worktrees: paths.map((path) => {
            const t = this.threads.find((x) => x.worktree?.path === path);
            if (!t?.worktree) return { path, exists: false } as unknown as Json;
            return {
              path,
              exists: true,
              branch: t.worktree.branch,
              against: "origin/main",
              ahead: t.worktree.ahead ?? 1,
              behind: t.worktree.behind ?? 0,
              dirty: t.worktree.dirty ? 1 : 0,
              last_commit: new Date(Date.now() - 12 * 60_000).toISOString(),
              last_commit_subject: "feat(desktop): the delegate's last commit",
            } as unknown as Json;
          }),
        };
      }
      case "_rustcode/worktrees/handoff": {
        const sessionId = str(params.sessionId ?? params.session_id);
        const t = this.threads.find((x) => x.id === sessionId);
        if (!t) throw Object.assign(new Error(`unknown session: ${sessionId}`), { code: -32602 });
        const to = str(params.to) ?? "checkout";
        if (to === "checkout") {
          if (!t.worktree) {
            throw Object.assign(new Error("this thread does not run in a worktree"), {
              code: -32000,
              data: { reason: "no_worktree" },
            });
          }
          if (t.worktree.dirty) {
            throw Object.assign(new Error("the worktree has uncommitted changes"), {
              code: -32000,
              data: { reason: "dirty" },
            });
          }
          const repo = t.worktree.repo;
          const branch = t.worktree.branch;
          const path = t.worktree.path;
          this.removedWorktrees.add(path);
          t.worktree = undefined;
          t.cwd = repo;
          return {
            session_id: sessionId,
            to,
            moved: true,
            worktree_removed: true,
            switched: true,
            branch,
            path: repo,
          } as unknown as Json;
        }
        if (t.worktree) {
          throw Object.assign(new Error("this thread already runs in a worktree"), {
            code: -32000,
            data: { reason: "has_worktree" },
          });
        }
        const worktree = this.makeWorktree(t.cwd, undefined, t.title);
        t.worktree = worktree;
        t.cwd = worktree.path;
        return {
          session_id: sessionId,
          to,
          moved: true,
          path: worktree.path,
          branch: worktree.branch,
          repo: worktree.repo,
          base: worktree.base,
          checkout_branch: "main",
        } as unknown as Json;
      }
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
      case "_rustcode/pulls/diff":
        return {
          diff: [
            "diff --git a/apps/desktop/src/ui/Composer.tsx b/apps/desktop/src/ui/Composer.tsx",
            "--- a/apps/desktop/src/ui/Composer.tsx",
            "+++ b/apps/desktop/src/ui/Composer.tsx",
            "@@ -1,4 +1,5 @@",
            ' import { useApp } from "../state/store";',
            '+import { compose } from "../state/compose";',
            " ",
            "-export const Composer = () => null;",
            "+export const Composer = () => <div className={compose()} />;",
          ].join("\n"),
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
      case "_rustcode/remote": {
        const sessionId = str(params.sessionId) ?? "";
        return this.remoteStatus(sessionId);
      }
      case "_rustcode/remote/start": {
        const sessionId = str(params.sessionId) ?? "";
        // Settings' switch names no thread: remote access is on for the
        // machine and every open thread is shared. A thread's own start
        // shares that thread, as the engine's auto-share pass does.
        await this.sleep(this.delay * 2);
        if (!sessionId) {
          this.remote.enabled = true;
          this.remote.gateway = this.remoteGateway();
          for (const open of this.threads) this.remote.shared.add(open.id);
          this.remote.refused.clear();
          if (params.pair === true) this.issueRemoteOffer();
          return this.remoteStatus("");
        }
        const thread = this.thread(sessionId);
        this.remote.enabled = true;
        this.remote.gateway = this.remoteGateway();
        this.remote.shared.add(thread.id);
        if (params.pair === true) this.issueRemoteOffer();
        return this.remoteStatus(thread.id);
      }
      case "_rustcode/remote/pair": {
        const sessionId = str(params.sessionId) ?? "";
        // A challenge is the gateway's, so a thread the app does not share
        // still pairs the machine (acp/remote.rs › `pair`).
        await this.sleep(this.delay * 2);
        this.remote.enabled = true;
        if (!this.remote.gateway) this.remote.gateway = this.remoteGateway();
        this.issueRemoteOffer();
        return { ...obj(this.remoteStatus(sessionId)), offer: this.remoteOffer() } as Json;
      }
      case "_rustcode/remote/connect_code": {
        await this.sleep(this.delay * 2);
        if (params.cancel === true) {
          const wasOpen = this.remote.machineOffer !== null;
          this.remote.machineOffer = null;
          return { ...this.remoteHost(), connectCodeCancelled: wasOpen };
        }
        // A connect code lives beside the QR challenge; creating one replaces only the last code.
        this.remote.enabled = true;
        if (!this.remote.gateway) this.remote.gateway = this.remoteGateway();
        this.remote.machineSerial += 1;
        this.remote.machineOffer = { serial: this.remote.machineSerial, expiresAt: Date.now() + 600_000 };
        return {
          ...this.remoteHost(),
          connectCode: {
            code: "K7MQ-29XH-TP4W-NRD6",
            link: "rustcode://connect/eyJ2IjoxLCJnYXRld2F5X2lkIjoiOWYyYzFkMGI0YTZlOGM3ZCIsImhvc3RfbmFtZSI6Ik1vY2sgTWFjQm9vayBQcm8iLCJyb3V0ZXMiOlt7ImFkZHJlc3MiOiIxMDAuOTAuMTYuMjM3OjE3ODc5Iiwia2luZCI6Im92ZXJsYXkifV0sImNyZWRlbnRpYWwiOiJtb2NrIn0",
            address: "100.90.16.237:17879",
            addressInCode: true,
            gatewayId: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
            instanceId: "mock-instance",
            hostName: "Mock MacBook Pro",
            routes: [{ address: "100.90.16.237:17879", kind: "overlay", provider: "netbird" }],
            loopbackOnly: false,
            expiresInSecs: 600,
            serial: this.remote.machineSerial,
          },
        };
      }
      case "_rustcode/remote/answer": {
        const id = str(params.id) ?? "";
        const accept = params.accept === true;
        await this.sleep(this.delay * 2);
        const request = this.remote.connect.find((entry) => entry.id === id);
        if (!request)
          return { ...this.remoteHost(), connectError: "that connection request is no longer open" };
        if (!accept) {
          this.endConnect(id, "denied");
          return {
            ...this.remoteHost(),
            connectAnswer: { id, deviceName: request.deviceName, accepted: false },
          };
        }
        if (request.state !== "pending")
          return { ...this.remoteHost(), connectError: "that connection request was already accepted" };
        // One challenge at a time: the code replaces the QR code's and any other accepted request's.
        for (const other of this.remote.connect.filter((entry) => entry.state === "approved"))
          this.endConnect(other.id, "replaced");
        this.remote.offer = null;
        request.state = "approved";
        request.expiresAt = Date.now() + 120_000;
        // In the browser the phone types the code a few seconds later; tests enter it themselves.
        if (this.delay > 0) setTimeout(() => this.enterConnectCode(), 6_000);
        return {
          ...this.remoteHost(),
          connectAnswer: {
            id,
            deviceName: request.deviceName,
            instanceId: "mock-gateway-instance-1",
            accepted: true,
            code: "4821-9034",
            expiresInSecs: 120,
          },
        };
      }
      case "_rustcode/remote/revoke": {
        const deviceId = str(params.deviceId) ?? "";
        await this.sleep(this.delay * 2);
        const device = this.remote.devices.find((entry) => entry.id === deviceId);
        if (!device) return { ...this.remoteHost(), revokeError: `no paired device matches ${deviceId}` };
        this.remote.devices = this.remote.devices.filter((entry) => entry !== device);
        return {
          ...this.remoteHost(),
          revoked: { deviceId: str(device.id) ?? deviceId, deviceName: str(device.name) ?? deviceId },
        };
      }
      // Test and mock-window hooks: a phone asks to connect, or enters the code it was shown.
      case "_mock/remote/askToConnect":
        return {
          id: this.askToConnect(
            str(params.deviceName) ?? "Lars's iPhone",
            str(params.platform) ?? "ios",
            str(params.remoteAddr) ?? "192.168.1.42",
            str(params.deviceKind) ?? "phone",
          ),
        };
      case "_mock/remote/enterCode":
        return { paired: this.enterConnectCode() };
      case "_rustcode/remote/stop": {
        const sessionId = str(params.sessionId) ?? "";
        await this.sleep(this.delay * 2);
        this.remote.enabled = false;
        this.remote.shared.clear();
        this.remote.gateway = null;
        this.remote.offer = null;
        this.remote.pairingDue = false;
        this.remote.pairingError = null;
        this.remote.refused.clear();
        // The gateway is gone, and with it every phone waiting on it.
        this.remote.connect = [];
        return this.remoteStatus(sessionId);
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
            ...(c.attempt ? { attempt: true } : {}),
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
        const turn = Number(params.turn);
        const at = thread.checkpoints.findIndex((x) => x.turn === turn);
        if (at < 0) throw Object.assign(new Error("No such checkpoint"), { code: -32602 });
        // A kept attempt can be gone back to as well: it restores the state it left, and the
        // conversation stays where it is (the engine has no rewind forward either).
        const attempt = thread.checkpoints[at]!.attempt === true;
        // The engine keeps every checkpoint: a rewind never deletes the turns it leaves.
        let kept: { turn: number; ref: string } | undefined;
        if (params.keep === true) {
          const next = (thread.checkpoints.at(-1)?.turn ?? 0) + 1;
          const abandoned = thread.checkpoints.filter((c) => c.turn >= turn && !c.attempt);
          thread.checkpoints.push({
            turn: next,
            prompt: "",
            created_at: new Date().toISOString(),
            files: abandoned.flatMap((c) => c.files),
            attempt: true,
          });
          kept = { turn: next, ref: `refs/rustcode/checkpoints/${thread.id}/${next}` };
        }
        const canRewind = thread.harness === "rustcode" && !attempt;
        if (params.rewind === true && canRewind) {
          // Its own turn's message, in the history the conversation still has.
          const own = thread.checkpoints.filter((c) => !c.attempt).findIndex((c) => c.turn === turn);
          let n = 0;
          const cut = thread.history.findIndex(
            (u) => str(obj(u.update)?.sessionUpdate) === "user_message_chunk" && ++n === own + 1,
          );
          if (cut >= 0) thread.history.splice(cut);
        }
        return {
          restored: true,
          conversation: params.rewind === true && canRewind ? "rewound" : "kept",
          ...(kept ? { kept } : {}),
        };
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
      case "_rustcode/settings/get": {
        const keys = arr(params.keys).flatMap((key) => (typeof key === "string" ? [key] : []));
        const values = keys.length
          ? Object.fromEntries(
              keys.filter((key) => key in this.serverSettings).map((key) => [key, this.serverSettings[key]!]),
            )
          : this.serverSettings;
        return { values, stored: Object.keys(this.serverSettings) };
      }
      case "_rustcode/settings/set": {
        const values = obj(params.values);
        if (values) this.serverSettings = { ...this.serverSettings, ...values };
        return { values: this.serverSettings, stored: Object.keys(this.serverSettings) };
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
        // A hand-added command named "broken" starts but never answers ACP.
        if (!h.known && h.command[0] === "broken")
          throw new Error(`${h.name} did not answer the ACP handshake (initialize timed out after 30 s).`);
        h.models = h.builtin ? h.models : (HARNESS_MODELS[id] ?? (h.known ? [] : MOCK_CUSTOM_MODELS));
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
            known: false,
            enabled: true,
            command: arr(params.command).map(String),
            // A command named "missing" is not on PATH.
            installed: arr(params.command).map(String)[0] !== "missing",
            env_names: Object.keys(obj(params.env) ?? {}),
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
        this.harnesses.harnesses = this.harnesses.harnesses.filter((h) => h.id !== params.id || h.known);
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
          skills: MOCK_SKILLS.map(withoutSkillContent),
          mcpServers: this.mcpServers(),
        };
      case "_rustcode/skills/get": {
        const name = str(params.name);
        const path = str(params.path);
        const skill = MOCK_SKILLS.find((entry) => str(entry.name) === name && str(entry.path) === path);
        if (!skill || typeof skill.content !== "string")
          throw Object.assign(new Error(`Could not read ${name ?? "that"}'s SKILL.md`), {
            code: -32602,
          });
        return { content: skill.content };
      }
      case "_rustcode/permissions": {
        // The same shape `_rustcode/permissions` answers with (engine/src/acp/permissions.rs).
        const now = Math.floor(Date.now() / 1000);
        return {
          grants: this.grants.filter((grant) => num(grant.expiresAt)! > now),
        };
      }
      case "_rustcode/permissions/revoke": {
        const id = str(params.id);
        this.grants = this.grants.filter((grant) => str(grant.id) !== id);
        return { grants: this.grants };
      }
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
      await call("organize_threads", { session_ids: [sid], pinned: true, unread: true });
      await call("configure_session", { effort: "high" });
      return "Renamed and pinned this thread, marked it unread, and raised its effort.";
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
      return "I can't reach other threads here: turn on Settings › Harnesses › Threads can start and message other threads, then restart the engine.";
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
      if (prompt.trim() === "/fork") {
        // Like the engine: a copy of the chat in a new thread that records this one as its parent.
        const title = `${thread.title ?? "Forked chat"} (fork)`;
        this.threads.unshift({
          ...thread,
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          title,
          updatedAt: Date.now(),
          parent: { id: thread.id, title: thread.title, relationship: "fork" },
          history: [...thread.history],
          checkpoints: [],
        });
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`**Fork**\n\nForked this chat into **${title}**. Open it from the chat list.`),
        });
        return done("end_turn");
      }
      // `/compact` says it is compacting while it runs, then replaces that line with its card. Two
      // variants stage the other states: `/compact hold` keeps compacting until stopped, and
      // `/compact fail` ends without compacting, with the reason. Neither line is history: a
      // reopened thread replays the card only.
      const compact = /^\/compact(?:\s+(hold|fail))?$/.exec(prompt.trim());
      if (compact) {
        const compacting = (phase: string, line: string) =>
          this.notify(
            update(sid, {
              sessionUpdate: "agent_message_chunk",
              content: text(line),
              _meta: { rustcode: { kind: "compaction", phase } },
            }),
          );
        compacting("started", "Compacting context…");
        if (compact[1] === "hold") {
          for (let i = 0; i < 600 && !run.cancelled; i++) await this.sleep(1_000);
          return done(run.cancelled ? "cancelled" : "end_turn");
        }
        if (compact[1] === "fail") {
          await this.sleep();
          compacting("failed", "Context compaction failed: the summary request timed out.");
          return done("end_turn");
        }
        thread.compactions = (thread.compactions ?? 0) + 1;
        const before = 18_400;
        const after = 15_500;
        // A harness compacts with a call of its own (Claude Code's `Compact conversation`) and then
        // says so; the app keeps the call out of a turn it started itself, so one notice is the
        // record of it and the answer above stays where the reader left it (`state/thread.ts`).
        const compactId = `compact-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: compactId,
          title: "Compact conversation",
          kind: "think",
          status: "in_progress",
          rawInput: { summary: "Summarize the conversation" },
        });
        await this.sleep();
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: compactId,
          status: "completed",
          rawOutput: { content: "Summarized the conversation." },
        });
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`Context compacted: reduced from ${before / 1_000}k to ${after / 1_000}k tokens.`),
          _meta: { rustcode: { kind: "compaction" } },
        });
        this.notify(
          update(sid, {
            sessionUpdate: "usage_update",
            used: after,
            size: 272_000,
            _meta: {
              rustcode: {
                estimated: false,
                inputTokens: 90_000,
                outputTokens: 3_200,
                cachedTokens: 61_000,
                context: mockContext(after, {
                  count: thread.compactions,
                  reclaimed: before - after,
                  before,
                }),
              },
            },
          }),
        );
        return done("end_turn");
      }
      if (!thread.title) {
        thread.title = prompt.split("\n")[0]?.slice(0, 60) || "New thread";
        this.notify(update(sid, { sessionUpdate: "session_info_update", title: thread.title }));
      }
      // Two row states that only exist while something happens, so the sidebar's status chip can be
      // seen without a model: "keep working" leaves a turn running for two minutes (`Working 0:12`
      // with its clock, then `Waiting`), and "fail this turn" ends it with an error (`Failed` until
      // the row is opened). `?scenario=delegates` covers `Waiting`, `Needs you` and the stack.
      if (/\bfail this turn\b/i.test(prompt)) throw new Error("The mock failed this turn, as asked.");
      if (/\bkeep working\b/i.test(prompt)) {
        const callId = `live-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: callId,
          title: "bash",
          kind: "execute",
          status: "in_progress",
          rawInput: { command: "bun test --watch", summary: "Keep the turn running" },
        });
        for (let i = 0; i < 60 && !run.cancelled; i++) {
          await this.sleep(2_000);
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: callId,
            _meta: { rustcode: { outputDelta: `✓ ${i + 1} tests still passing\n` } },
          });
        }
        if (run.cancelled) return done("cancelled");
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: "completed",
          rawOutput: { content: "✓ 60 pass, 0 fail\n", exitCode: 0 },
        });
        return done("end_turn");
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
      const round = (input: number, output: number, cost: number) => {
        const used = 24_000 + Math.round((Date.now() - started) / 10);
        push({
          sessionUpdate: "usage_update",
          used,
          size: 272_000,
          cost: { amount: cost, currency: "USD" },
          _meta: {
            rustcode: {
              estimated: false,
              inputTokens: input,
              outputTokens: output,
              cachedTokens: Math.round(input * 0.55),
              context: mockContext(used),
            },
          },
        });
      };
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
        : /\b(hover|trend|graph)\b/i.test(prompt)
          ? MOCK_CHARTS
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
  '<div class="rc-stats">\n',
  '  <div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span></div>\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span></div>\n',
  "</div>\n",
  '<div class="rc-chart"><canvas id="c" role="img" aria-label="Runs and failures per day"></canvas></div>\n',
  '<div class="rc-row rc-gap-2">\n',
  '  <button class="primary" id="ask">Explain Thursday\'s failures</button>\n',
  '  <button class="rc-chip" id="copy">Copy numbers</button>\n',
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
  '    <div class="rc-row rc-gap-2"><span class="rc-label">2 of 4 steps</span><div class="rc-progress" style="flex:1"><span style="width:50%"></span></div></div>\n',
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
  '    <li class="rc-item"><span class="rc-badge">Worth reviewing</span><strong>docs/superpowers/</strong><div>Ignored by the repo. It contains 26 design specs and plans, including provider login and remote turn timing.</div></li>\n',
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
  '<div class="rc-row rc-gap-2">\n',
  '  <button class="rc-chip" data-rc-send="Refresh the auth fixture">Refresh the fixture</button>\n',
  '  <button class="rc-chip" data-rc-send="Show the failing tests">Show the failures</button>\n',
  '  <button class="rc-chip" data-rc-copy="128 run, 7 failed, 41 s median">Copy numbers</button>\n',
  "</div>\n",
  "```\n",
];

/**
 * A Smart UI reply built from the kit's charts and hover pieces: a themed line
 * chart, a donut with its total in the middle, bars with hover text, an inline
 * sparkline and a table that sorts on a heading click. No CSS of its own.
 */
export const MOCK_CHARTS = [
  "Failures fell to zero after Thursday's fixture fix.\n\n",
  "```rustcode-ui v1\n",
  "<title>Failures this week</title>\n",
  '<p class="rc-lead">Failures fell to zero after Thursday\'s fixture fix.</p>\n',
  '<div class="rc-stats">\n',
  '  <div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span><span class="rc-stat-note">4 on Thursday</span></div>\n',
  '  <div class="rc-stat rc-ok"><span class="rc-stat-value">0</span><span class="rc-stat-label">Failed since</span><span class="rc-stat-note">3 days clean</span></div>\n',
  '  <div class="rc-stat"><span class="rc-stat-value">41 s</span><span class="rc-stat-label">Median run</span><span class="rc-stat-note">6 s faster</span></div>\n',
  "</div>\n",
  '<div class="rc-chart"><canvas role="img" aria-label="Runs and failures per day"></canvas></div>\n',
  '<div class="rc-row rc-gap-3">\n',
  '  <div class="rc-chart" data-size="sm" style="width:140px"><canvas role="img" aria-label="Runs by suite"></canvas>',
  '<div class="rc-donut-total"><b>128</b><span>runs</span></div></div>\n',
  '  <div class="rc-bars" style="flex:1">\n',
  '    <div class="rc-bar" data-rc-tip="64 passed, 0 failed, 42 s"><span>engine</span><i style="width:100%"></i><b>42 s</b></div>\n',
  '    <div class="rc-bar" data-rc-tip="38 passed, 0 failed, 18.5 s"><span>desktop</span><i style="width:44%"></i><b>18.5 s</b></div>\n',
  '    <div class="rc-bar rc-bad" data-rc-tip="12 passed, 7 failed, 10.5 s"><span>auth</span><i style="width:25%"></i><b>10.5 s</b></div>\n',
  "  </div>\n",
  "</div>\n",
  '<div class="rc-row rc-gap-2"><span class="rc-label">Duration, last 8 runs</span>',
  '<span class="rc-spark" data-values="86,81,79,74,72,63,55,48" style="width:120px"></span>',
  '<span class="rc-num">48 s</span></div>\n',
  '<table class="rc-table" data-rc-sort><thead><tr><th>Suite</th><th class="rc-num">Time</th><th class="rc-num">Failed</th></tr></thead><tbody>\n',
  '  <tr><td>engine</td><td class="rc-num">42 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>desktop</td><td class="rc-num">18.5 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>auth</td><td class="rc-num">10.5 s</td><td class="rc-num">7</td></tr>\n',
  '  <tr><td>core</td><td class="rc-num">3 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>e2e</td><td class="rc-num">64 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>cli</td><td class="rc-num">2 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>mcp</td><td class="rc-num">1 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>sandbox</td><td class="rc-num">9 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>store</td><td class="rc-num">4 s</td><td class="rc-num">0</td></tr>\n',
  '  <tr><td>tui</td><td class="rc-num">7 s</td><td class="rc-num">0</td></tr>\n',
  "</tbody></table>\n",
  '<script src="rustcode-lib:chart.js"></script>\n',
  "<script>\n",
  "const canvases = document.querySelectorAll('.rc-chart canvas');\n",
  "new Chart(canvases[0], { type: 'line', data: { labels: ['Mon','Tue','Wed','Thu','Fri'],\n",
  "  datasets: [{ label: 'Runs', data: [25,27,24,23,29] }, { label: 'Failed', data: [1,0,2,4,0] }] } });\n",
  "new Chart(canvases[1], { type: 'doughnut', data: { labels: ['engine','desktop','auth','core'],\n",
  "  datasets: [{ label: 'Runs', data: [64,38,19,7] }] }, options: { plugins: { legend: { display: false } } } });\n",
  "</script>\n",
  '<details class="rc-details rc-sources"><summary>Sources</summary><ol>\n',
  '  <li><a href="https://github.com/LHagfoss/rustcode/actions">CI runs, this week</a></li>\n',
  '  <li><a href="https://www.example.com/flaky-tests">Flaky test triage notes</a></li>\n',
  "</ol></details>\n",
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

// ---------------------------------------------------------------- machines

/**
 * The git remote of the mock's projects, as a machine's `session/list` reports it
 * (`_meta.rustcode.remote`): one repository checked out in two places, which is what **Run on**
 * finds again (state/projects.ts).
 */
function mockRemote(cwd: string): string | undefined {
  if (cwd.startsWith("/Users/you/code/rustcode") || cwd.startsWith("/Users/lars/code/rustcode"))
    return "git@github.com:LHagfoss/rustcode.git";
  if (cwd.startsWith("/Users/you/code/website")) return "git@github.com:LHagfoss/rustcode-website.git";
  return undefined;
}

/**
 * The machine this app is paired with in the mock: a Linux box with three
 * threads, which is what `Lars's Linux box` is on the machine network plan.
 */
const MOCK_MACHINE = {
  id: "gw-7c1d9e4f2a8b6d30",
  name: "Lars's Linux box",
  platform: "linux",
  host: "ubuntu-1.local",
  route: "192.168.1.31:17879",
  overlay: "100.90.16.237:17879",
  threads: 3,
};

/** The NetBird address the saved machine's handshake reported (#3402). */
const MOCK_OVERLAY_ROUTE = {
  kind: "vpn",
  address: MOCK_MACHINE.overlay,
  provider: "netbird",
  encrypted: true,
};

/**
 * One saved machine as `machines_list` reports it (`src-tauri/src/machines.rs`).
 * `?machines=connect` asks for the overlay route and an older version, the
 * state the Add sheet has to explain.
 */
function mockMachine(
  state: "connected" | "offline" | "needsPairing",
  addedAt: number,
  extra: { overlay?: boolean; version?: string; hint?: string | null } = {},
): JsonObject {
  const route = { kind: "lan", address: MOCK_MACHINE.route, encrypted: false };
  const routes = extra.overlay ? [route, MOCK_OVERLAY_ROUTE] : [route];
  return {
    id: MOCK_MACHINE.id,
    name: MOCK_MACHINE.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_MACHINE.platform,
    version: extra.version ?? "1.28.0",
    routes,
    route,
    state,
    detail:
      state === "offline"
        ? "failed to connect to 192.168.1.31:17879: Connection timed out (os error 60)"
        : state === "needsPairing"
          ? "Lars's Linux box ended this computer's pairing. Pair it again to see its threads."
          : null,
    hint: extra.hint ?? null,
    retryInSecs: state === "offline" ? 4 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: state === "connected" ? new Date().toISOString() : new Date(addedAt).toISOString(),
  };
}

/** A second computer on the same network, with threads of its own (`?machines=paired`). */
const MOCK_STUDIO = {
  id: "gw-8e1f4b7a5c2d4e6f",
  name: "Studio",
  platform: "macos",
  route: "192.168.1.20:17879",
  version: "1.28.0",
  // Two threads in one project and one in another, so **Run on** shows a choice of folders.
  threads: 3,
};

/** A Windows computer on the same network, one RustCode behind (`?machines=paired`). */
const MOCK_WINDOWS = {
  id: "gw-4a2c8e5f1b7d9c30",
  name: "Lars's Windows box",
  platform: "windows",
  route: "192.168.1.44:17879",
  version: "1.27.0",
  threads: 2,
};

/** The Studio row: a Mac on the LAN, connected (or not) like the Linux box. */
function mockStudio(state: "connected" | "offline", addedAt: number): JsonObject {
  return {
    id: MOCK_STUDIO.id,
    name: MOCK_STUDIO.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_STUDIO.platform,
    version: MOCK_STUDIO.version,
    routes: [{ kind: "lan", address: MOCK_STUDIO.route, encrypted: false }],
    route: { kind: "lan", address: MOCK_STUDIO.route, encrypted: false },
    state,
    detail: state === "offline" ? "Not connected. Retrying in 8 s." : null,
    retryInSecs: state === "offline" ? 8 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: new Date(addedAt).toISOString(),
  };
}

/** The Windows row: a PC on the LAN, one RustCode behind this app (the version sentence). */
function mockWindows(state: "connected" | "offline", addedAt: number): JsonObject {
  return {
    id: MOCK_WINDOWS.id,
    name: MOCK_WINDOWS.name,
    displayName: null,
    icon: null,
    iconTint: null,
    paused: false,
    platform: MOCK_WINDOWS.platform,
    version: MOCK_WINDOWS.version,
    routes: [{ kind: "lan", address: MOCK_WINDOWS.route, encrypted: false }],
    route: { kind: "lan", address: MOCK_WINDOWS.route, encrypted: false },
    state,
    detail: state === "offline" ? "Not connected. Retrying in 8 s." : null,
    retryInSecs: state === "offline" ? 8 : null,
    addedAt: new Date(addedAt).toISOString(),
    lastConnectedAt: new Date(addedAt).toISOString(),
  };
}

/** The saved machines `?machines=` starts with. */
function mockMachineList(option: MockOptions["machines"]): JsonObject[] {
  const savedAt = Date.now() - 1000 * 60 * 60 * 24 * 6;
  switch (option) {
    case "connect":
      // Saved once, on an older RustCode, and reached over NetBird: the Add
      // sheet lists it, says to update it, and the server is not installed.
      return [
        mockMachine("offline", savedAt, {
          overlay: true,
          version: "0.96.0",
          hint: "Not reachable over NetBird (100.90.16.237:17879). On that machine, check that NetBird allows inbound connections and that an access policy allows TCP 17879 between the two machines.",
        }),
      ];
    case "offline":
      return [mockMachine("offline", savedAt)];
    case "revoked":
      return [mockMachine("needsPairing", savedAt)];
    case "paired":
      // The sidebar's own happy path: this Mac, a Windows PC with threads, and a machine
      // that is away, saved during this session so the Mac and the PC read as just saved.
      return [
        mockStudio("connected", Date.now()),
        mockWindows("connected", savedAt),
        mockMachine("offline", savedAt),
      ];
    default:
      return [mockMachine("connected", savedAt)];
  }
}

/**
 * Whether an address is on the public internet, by the same rule
 * `rustcode-client` applies (`registry::classify_address`): a private, loopback
 * or link-local address is LAN, CGNAT and `.ts.net` are an overlay, a bare name
 * or `.local` is LAN, and anything else is public. The client refuses a public
 * address because the gateway has no TLS.
 */
function mockIsPublic(address: string): boolean {
  const withoutScheme = address.trim().replace(/^[a-z][a-z0-9+.-]*:\/\//i, "");
  const endpoint = withoutScheme.split("/")[0] ?? withoutScheme;
  const host = (endpoint.split(":")[0] ?? endpoint).replace(/^\[|\]$/g, "").toLowerCase();
  if (!host) return false;
  if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {
    const [a, b] = host.split(".").map(Number) as [number, number];
    if (a === 10 || a === 127) return false;
    if (a === 192 && b === 168) return false;
    if (a === 172 && b >= 16 && b <= 31) return false;
    if (a === 169 && b === 254) return false;
    // 100.64.0.0/10 is CGNAT, which Tailscale and other overlays hand out.
    if (a === 100 && b >= 64 && b <= 127) return false;
    return true;
  }
  // IPv6: loopback, unique-local and link-local are private; the rest are not.
  if (host.includes(":")) return !(host === "::1" || host.startsWith("fd") || host.startsWith("fe80"));
  if (host.endsWith(".local") || host.endsWith(".ts.net")) return false;
  return host.includes(".");
}

/**
 * The machines `machines_discover` lists in the mock, as the shell answers:
 * Bonjour on this network, plus the saved machine's own NetBird address, which
 * the page merges by host id into one row per machine. The local search
 * always works here, so the notice is null.
 */
function mockDiscovery(saved: JsonObject[]): JsonObject {
  return { rows: mockDiscovered(saved), notice: null };
}

function mockDiscovered(saved: JsonObject[]): JsonObject[] {
  const machine = saved.find((row) => row.id === MOCK_MACHINE.id);
  const version = machine ? String(machine.version ?? "") || null : null;
  return [
    {
      name: MOCK_MACHINE.name,
      source: "bonjour",
      address: MOCK_MACHINE.route,
      addresses: ["192.168.1.31"],
      gatewayId: MOCK_MACHINE.id,
      platform: MOCK_MACHINE.platform,
      version,
      routeKind: "lan",
      hint: null,
      saved: Boolean(machine),
    },
    {
      name: "Studio",
      source: "bonjour",
      address: "192.168.1.20:17879",
      addresses: ["192.168.1.20"],
      gatewayId: "gw-8e1f4b7a5c2d4e6f",
      platform: "macos",
      version: "1.28.0",
      routeKind: "lan",
      hint: null,
      saved: saved.some((row) => row.id === "gw-8e1f4b7a5c2d4e6f"),
    },
    ...(machine
      ? [
          {
            name: MOCK_MACHINE.name,
            source: "overlay",
            address: MOCK_MACHINE.overlay,
            addresses: [MOCK_MACHINE.overlay],
            gatewayId: MOCK_MACHINE.id,
            platform: MOCK_MACHINE.platform,
            version,
            routeKind: "netbird",
            hint: machine.hint ?? null,
            saved: true,
          },
        ]
      : []),
  ];
}

/** The RustCode server as a background service, as `src-tauri/src/service.rs` answers. */
function mockServerService(installed: boolean): JsonObject {
  return {
    installed,
    running: installed,
    version: installed ? "1.28.0" : null,
    cliVersion: "1.28.0",
    gateway: {
      remoteEnabled: installed,
      running: installed,
      address: installed ? "192.168.1.20:17879" : null,
      advertised: installed ? "192.168.1.20:17879" : null,
      owner: installed ? "server" : null,
      version: installed ? "1.28.0" : null,
      pid: installed ? 97702 : null,
      pairedDevices: 1,
      port: 17879,
    },
    pathRustcode: null,
    problems: installed
      ? []
      : [{ code: "not-installed", message: "The RustCode server service is not installed." }],
    error: null,
  };
}

/**
 * One saved machine's ACP, answered in process for `?machines=`: the app's own
 * shell dials the real machine over its gateway, so this exists only where
 * there is no shell to dial with. It answers `initialize`, `session/list`,
 * `_rustcode/harnesses` and `session/new`, which is what the app reads and does
 * of a connected machine (docs/desktop.md › Several machines).
 */
export class MockMachine {
  /** The threads this window started on the machine, newest first (`session/new`). */
  private readonly created: {
    sessionId: string;
    cwd: string;
    title: string;
    harness: string;
    updatedAt: number;
  }[] = [];
  private createdCount = 0;
  /** The server settings of this machine, differing from this app's own. */
  private settings: JsonObject;
  /** A harness change made here, over the machine's seeded snapshot. */
  private harnessOverride: HarnessSnapshot | null = null;
  /** What this client advertised: only one that lists threads hears about the ones made here. */
  private showsThreads = false;
  /** Its skills, with the SKILL.md so a copy can be seen. */
  private skills: JsonObject[];
  /** Its MCP servers, with key names but no secret values. */
  private mcpServers: JsonObject[];

  constructor(
    private readonly name: string,
    private readonly threads?: number,
  ) {
    const studio = name === MOCK_STUDIO.name;
    this.settings = {
      "default-permission-mode": studio ? "ask" : "edits",
      "resume-after-restart": studio,
      "continue-after-limit": true,
      "auto-compact-mode": studio ? "size" : "cache",
      "auto-compact-threshold": studio ? 150_000 : 200_000,
      orchestration: true,
      "delegates-total": 8,
      "delegates-per-thread": studio ? 4 : 6,
      "read-across-projects": true,
    };
    this.skills = [
      {
        name: studio ? "frontend-design" : "release-automation",
        description: studio ? "Design guidance for web UIs" : "Cut a release with scripts/release.sh",
        path: studio
          ? "/Users/lars/.agents/skills/frontend-design"
          : "/home/lars/.agents/skills/release-automation",
        source: "universal",
        readOnly: false,
        enabled: true,
        content: studio
          ? "---\nname: frontend-design\ndescription: Design guidance for web UIs\n---\n\nRead docs/design.md before shaping a page.\n"
          : "---\nname: release-automation\ndescription: Cut a release with scripts/release.sh\n---\n\nRun scripts/release.sh --patch.\n",
      },
      {
        name: "shared-notes",
        description: "Notes every machine can have",
        path: "/home/lars/.config/rustcode/skills/shared-notes",
        source: "rustcode",
        readOnly: false,
        enabled: false,
        content:
          "---\nname: shared-notes\ndescription: Notes every machine can have\n---\n\nKeep notes short.\n",
      },
    ];
    this.mcpServers = studio
      ? [
          {
            name: "notes",
            transport: "http",
            target: "https://mcp.example.com/notes",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: ["search", "write"],
            error: null,
            envKeys: [],
            headerKeys: ["Authorization"],
            copyTarget: ["https://mcp.example.com/notes"],
          },
        ]
      : [
          {
            name: "github",
            transport: "stdio",
            target: "npx -y github-mcp",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: ["issues", "pulls"],
            error: null,
            envKeys: ["GITHUB_TOKEN"],
            headerKeys: [],
            copyTarget: ["npx", "-y", "github-mcp"],
          },
        ];
  }

  /** The frames this machine sends back for one line the page wrote. */
  receive(line: string): string[] {
    let frame: JsonObject;
    try {
      const parsed = JSON.parse(line) as Json;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return [];
      frame = parsed;
    } catch {
      return [];
    }
    const id = frame.id;
    if (typeof id !== "number") return [];
    if (frame.method === "initialize") {
      const rustcode = obj(obj(obj(frame.params)?.clientCapabilities)?._meta)?.rustcode;
      this.showsThreads = !!num(obj(rustcode)?.threads);
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { protocolVersion: 1, agentCapabilities: {} } })];
    }
    if (frame.method === "session/list")
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { sessions: this.sessions() } })];
    if (frame.method === "_rustcode/harnesses")
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: (this.harnessOverride ?? this.harnesses()) as unknown as Json,
        }),
      ];
    if (frame.method === "session/new") {
      const created = this.newSession(id, frame.params);
      return this.showsThreads
        ? [
            JSON.stringify({
              jsonrpc: "2.0",
              method: "_rustcode/threads/created",
              params: created.notification,
            }),
            created.response,
          ]
        : [created.response];
    }
    if (frame.method === "_rustcode/threads/delete") {
      const sessionId = str(obj(frame.params)?.sessionId);
      const at = sessionId ? this.created.findIndex((entry) => entry.sessionId === sessionId) : -1;
      if (at >= 0) this.created.splice(at, 1);
      const gone = sessionId
        ? [
            JSON.stringify({
              jsonrpc: "2.0",
              method: "_rustcode/threads/deleted",
              params: { sessionId },
            }),
          ]
        : [];
      return [...gone, JSON.stringify({ jsonrpc: "2.0", id, result: {} })];
    }
    if (frame.method === "session/load") return this.load(id, frame.params);
    if (frame.method === "session/prompt") {
      const sessionId = str(obj(frame.params)?.sessionId);
      this.titleFromPrompt(sessionId, frame.params);
      const reply = [
        sessionId
          ? JSON.stringify({
              jsonrpc: "2.0",
              method: "session/update",
              params: {
                sessionId,
                update: {
                  sessionUpdate: "agent_message_chunk",
                  content: { type: "text", text: `Ran it on ${this.name}. The answer stays there.` },
                },
              },
            })
          : null,
        JSON.stringify({ jsonrpc: "2.0", id, result: { stopReason: "end_turn" } }),
      ];
      return reply.filter((x): x is string => x !== null);
    }
    if (frame.method === "_rustcode/settings/get") {
      const keys = arr(obj(frame.params)?.keys).flatMap((key) => (typeof key === "string" ? [key] : []));
      const values = keys.length
        ? Object.fromEntries(
            keys.filter((key) => key in this.settings).map((key) => [key, this.settings[key]!]),
          )
        : this.settings;
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: { values, stored: Object.keys(this.settings) },
        }),
      ];
    }
    if (frame.method === "_rustcode/settings/set") {
      const values = obj(obj(frame.params)?.values);
      if (values) this.settings = { ...this.settings, ...values };
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: { values: this.settings, stored: Object.keys(this.settings) },
        }),
      ];
    }
    if (frame.method === "_rustcode/harnesses/default") {
      const params = obj(frame.params);
      const snapshot = this.harnessOverride ?? this.harnesses();
      this.harnessOverride = {
        ...snapshot,
        default: {
          harness: str(params?.harness) ?? snapshot.default.harness,
          model: str(params?.model) ?? null,
          effort: str(params?.effort) ?? null,
        },
      };
      return [JSON.stringify({ jsonrpc: "2.0", id, result: this.harnessOverride as unknown as Json })];
    }
    if (frame.method === "_rustcode/harnesses/set") {
      const params = obj(frame.params);
      const id = str(params?.id);
      const enabled = params?.enabled;
      const snapshot = this.harnessOverride ?? this.harnesses();
      this.harnessOverride = {
        ...snapshot,
        harnesses: snapshot.harnesses.map((harness) =>
          harness.id === id && typeof enabled === "boolean" ? { ...harness, enabled } : harness,
        ),
      };
      return [JSON.stringify({ jsonrpc: "2.0", id, result: this.harnessOverride as unknown as Json })];
    }
    if (frame.method === "_rustcode/capabilities") {
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: {
            configPath: `/home/lars/.config/rustcode/config.toml`,
            skills: this.skills.map(withoutSkillContent),
            mcpServers: this.mcpServers,
          },
        }),
      ];
    }
    if (frame.method === "_rustcode/capabilities/set") {
      const params = obj(frame.params);
      const name = str(params?.name);
      const enabled = params?.enabled === true;
      if (params?.kind === "skill")
        this.skills = this.skills.map((skill) => (str(skill.name) === name ? { ...skill, enabled } : skill));
      if (params?.kind === "mcp")
        this.mcpServers = this.mcpServers.map((server) =>
          str(server.name) === name ? { ...server, enabled } : server,
        );
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/capabilities/mcp/add") {
      const params = obj(frame.params);
      const target = arr(params?.target).flatMap((part) => (typeof part === "string" ? [part] : []));
      const transport = str(params?.transport) ?? (target[0]?.startsWith("http") ? "http" : "stdio");
      this.mcpServers = [
        ...this.mcpServers.filter((server) => str(server.name) !== str(params?.name)),
        {
          name: str(params?.name) ?? "server",
          transport,
          target: target.join(" "),
          enabled: true,
          scope: "user",
          connection: "connected",
          tools: [],
          error: null,
          envKeys: arr(params?.env).map((line) => String(line).split("=")[0] ?? ""),
          headerKeys: arr(params?.headers).map((line) => String(line).split(":")[0] ?? ""),
          copyTarget: target,
        },
      ];
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/capabilities/mcp/remove") {
      const name = str(obj(frame.params)?.name);
      this.mcpServers = this.mcpServers.filter((server) => str(server.name) !== name);
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/skills/get") {
      const params = obj(frame.params);
      const skill = this.skills.find(
        (entry) => str(entry.name) === str(params?.name) && str(entry.path) === str(params?.path),
      );
      if (!skill || typeof skill.content !== "string") {
        return [
          JSON.stringify({
            jsonrpc: "2.0",
            id,
            error: {
              code: -32602,
              message: `Could not read ${str(params?.name) ?? "that"}'s SKILL.md`,
            },
          }),
        ];
      }
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { content: skill.content } })];
    }
    if (frame.method === "_rustcode/skills/add") {
      const params = obj(frame.params);
      const name = str(params?.name) ?? "skill";
      this.skills = [
        ...this.skills.filter((skill) => str(skill.name) !== name),
        {
          name,
          description: "Copied from another machine",
          path: `/home/lars/.config/rustcode/skills/${name}`,
          source: "rustcode",
          readOnly: false,
          enabled: true,
          ...(str(params?.content) ? { content: str(params?.content) } : {}),
        },
      ];
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: { path: `/home/lars/.config/rustcode/skills/${name}/SKILL.md` },
        }),
      ];
    }
    return [
      JSON.stringify({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `${this.name} does not answer ${String(frame.method)}.`,
        },
      }),
    ];
  }

  /** Opening one of its threads: a two-line history, then the answer. */
  private load(id: number, params: Json | undefined): string[] {
    const sessionId = str(obj(params)?.sessionId);
    const title = this.sessions().find((s) => s.sessionId === sessionId)?.title ?? "A thread";
    const update = (u: JsonObject) =>
      JSON.stringify({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update: u } });
    return [
      update({
        sessionUpdate: "user_message_chunk",
        content: { type: "text", text: `Run "${title}" on ${this.name}.` },
      }),
      update({
        sessionUpdate: "agent_message_chunk",
        content: {
          type: "text",
          text: `This thread lives on ${this.name}. Its workspace, its files and its shell are there; this window reads and drives it over the machine network.`,
        },
      }),
      JSON.stringify({ jsonrpc: "2.0", id, result: { configOptions: [] } }),
    ];
  }

  /** The engine titles a thread from its first prompt; a thread started here does the same. */
  private titleFromPrompt(sessionId: string | undefined, params: Json | undefined): void {
    const session = this.created.find((entry) => entry.sessionId === sessionId);
    if (session?.title !== "New thread") return;
    const text = arr(obj(params)?.prompt)
      .map((block) => str(obj(block)?.text) ?? "")
      .join(" ")
      .trim();
    if (text) session.title = text.length > 60 ? `${text.slice(0, 60)}…` : text;
  }

  /** `session/new` on this machine: a thread in its folder, run by one of its harnesses. */
  private newSession(id: number, params: Json | undefined): { response: string; notification: JsonObject } {
    const studio = this.name === MOCK_STUDIO.name;
    const meta = obj(obj(params)?._meta)?.rustcode;
    this.createdCount += 1;
    const cwd = str(obj(params)?.cwd) || (studio ? "/Users/lars/code/rustcode" : "/home/lars/code/rustcode");
    const sessionId = `${studio ? "mock-studio" : "mock-linux"}-new-${this.createdCount}`;
    const harness = str(obj(meta)?.harness) ?? (studio ? "claude-code" : "codex");
    this.created.unshift({
      sessionId,
      cwd,
      title: "New thread",
      harness,
      updatedAt: Date.now(),
    });
    return {
      response: JSON.stringify({ jsonrpc: "2.0", id, result: { sessionId, configOptions: [] } }),
      // What the engine says to a client that lists threads, as the thread is made.
      notification: { sessionId, cwd, harness },
    };
  }

  /**
   * This machine's own harnesses, models, logins and default: the Studio runs Claude Code,
   * the Linux box Codex, and neither has this Mac's built-in harness list
   * (docs/desktop.md › Several machines).
   */
  private harnesses(): HarnessSnapshot {
    const studio = this.name === MOCK_STUDIO.name;
    const base = initialHarnesses(true);
    const kept = studio ? ["claude-code", "codex"] : ["rustcode", "codex"];
    const models: Record<string, HarnessSnapshot["harnesses"][number]["models"]> = studio
      ? {
          "claude-code": [
            {
              id: "sonnet-5.5",
              name: "Sonnet 5.5",
              efforts: ["low", "medium", "high", "max"],
              default_effort: "high",
            },
            { id: "opus-5.5", name: "Opus 5.5", efforts: ["medium", "high", "max"], default_effort: "high" },
          ],
          codex: [{ id: "gpt-5.5", name: "GPT-5.5", efforts: EFFORTS, default_effort: "medium" }],
        }
      : {
          rustcode: RUSTCODE_MODELS.map((m) => ({
            id: m.value,
            name: m.name,
            efforts: EFFORTS,
            default_effort: "medium",
          })),
          codex: [{ id: "gpt-5.5-codex", name: "GPT-5.5 Codex", efforts: EFFORTS, default_effort: "medium" }],
        };
    return withLast({
      harnesses: base.harnesses
        .filter((harness) => kept.includes(harness.id))
        .map((harness) => ({ ...harness, enabled: true, auth: "ok", models: models[harness.id] ?? [] })),
      default: studio
        ? { harness: "claude-code", model: "sonnet-5.5", effort: "high" }
        : { harness: "codex", model: "gpt-5.5-codex", effort: "medium" },
      onboarded: true,
      remembers: true,
    });
  }

  private sessions(): JsonObject[] {
    const studio = this.name === MOCK_STUDIO.name;
    const now = Date.now();
    const seeded = studio
      ? [
          { title: "Ship the mock window shots", cwd: "/Users/lars/code/rustcode", harness: "claude-code" },
          { title: "Review the machine groups", cwd: "/Users/lars/code/rustcode", harness: "claude-code" },
          { title: "Draft the release notes", cwd: "/Users/lars/code/notes", harness: "codex" },
        ]
      : [
          { title: "Run the tests", cwd: "/home/lars/code/rustcode", harness: "codex" },
          { title: "Fix the flaky CI job", cwd: "/home/lars/code/rustcode", harness: "codex" },
          { title: "Draft the release notes", cwd: "/home/lars/code/rustcode", harness: "rustcode" },
        ];
    const count = this.threads ?? (studio ? MOCK_STUDIO.threads : MOCK_MACHINE.threads);
    // The engine lists a session once it saved history: a just-made thread has none until its
    // first prompt, and reaches a client through `_rustcode/threads/created` until then.
    const listed = [
      ...this.created.filter((session) => session.title !== "New thread"),
      ...seeded.slice(0, count).map((session, index) => ({
        ...session,
        sessionId: `${studio ? "mock-studio" : "mock-linux"}-${index + 1}`,
        updatedAt: now,
      })),
    ];
    return listed.map((session, index) => ({
      sessionId: session.sessionId,
      cwd: session.cwd,
      title: session.title,
      updatedAt: new Date(session.updatedAt - index * 60_000).toISOString(),
      // The shape `session/list` really carries (`acp/types.ts` `parseSessionList`), including
      // the project's git remote, which finds the same project on this computer (state/projects.ts).
      _meta: {
        model: studio ? "sonnet-5.5" : "gpt-5.5-codex",
        rustcode: {
          harness: session.harness,
          ...(mockRemote(session.cwd) ? { remote: mockRemote(session.cwd) } : {}),
        },
      },
    }));
  }
}

export class MockTransport implements Transport {
  readonly name = "mock";
  private engine: MockEngine | null = null;
  private onEvent: ((event: TransportEvent) => void) | null = null;
  /** The machines this app is paired with, as `machines_list` reports them. */
  private machineList: JsonObject[];

  /** Whether the RustCode server is installed as a service (`?machines=connect` starts without it). */
  private serverInstalled: boolean;

  constructor(private readonly options: MockOptions = {}) {
    this.machineList = mockMachineList(options.machines);
    this.serverInstalled = options.machines !== "connect";
  }

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    this.engine = new MockEngine(
      (line) => queueMicrotask(() => onEvent({ type: "line", line })),
      this.options,
    );
    if (this.options.bootMs) await new Promise((r) => setTimeout(r, this.options.bootMs));
    this.startedAt = Date.now();
    for (const line of MOCK_SERVER_LOG) onEvent({ type: "log", line });
    onEvent({ type: "open", engine: "mock" });
    const crash = this.options.crashAfterMs;
    if (crash && !this.crashed) {
      this.crashed = true;
      setTimeout(() => {
        const reason = "The server was stopped by signal 9.";
        this.lastExit = { code: null, signal: 9, atMs: Date.now(), reason, expected: false };
        this.crashRestarts += 1;
        onEvent({ type: "closed", reason });
        void this.start(onEvent);
      }, crash);
    }
  }
  private crashed = false;
  /** What Settings › Diagnostics reads of the process (`server_status`). */
  private startedAt = Date.now();
  private crashRestarts = 0;
  private lastExit: JsonObject | null = null;

  async restart(): Promise<void> {
    const onEvent = this.onEvent;
    if (!onEvent) return;
    const reason = "The server exited (code 0).";
    this.lastExit = { code: 0, signal: null, atMs: Date.now(), reason, expected: true };
    onEvent({ type: "closed", reason });
    await this.start(onEvent);
  }

  /** `server_status`, as src-tauri/src/diagnostics.rs answers it. */
  private serverStatus(): JsonObject {
    return {
      running: this.engine !== null,
      pid: this.engine ? 48213 : null,
      path: "/Applications/RustCode.app/Contents/MacOS/rustcode",
      startedAtMs: this.engine ? this.startedAt : null,
      crashRestarts: this.crashRestarts,
      lastExit: this.lastExit,
      version: "1.21.0",
      configDir: "/Users/lars/.config/rustcode",
      logFile: "/Users/lars/.config/rustcode/debug.log",
    };
  }

  send(line: string): void {
    this.engine?.receive(line);
  }

  async call(command: string, args: Record<string, unknown>): Promise<unknown> {
    if (command === "machines_list") return this.machineList;
    if (command === "machines_discover") return mockDiscovery(this.machineList);
    if (command === "machines_ask") {
      const address = String(args.address ?? "");
      if (!address) throw new Error("Enter a machine address first.");
      if (mockIsPublic(address))
        throw new Error(
          `refusing to dial ${address}: it is a public address and the gateway has no TLS listener, so the pairing code and the device token would travel in the clear. Dial a LAN, Tailscale or VPN address instead; encrypted transport is tracked in #2651`,
        );
      // Asked, then accepted on the machine: the code is what comes next. The
      // ask id is the page's own, and it names the request for `machines_pair`.
      return { askId: String(args.askId ?? ""), codeLength: 8, expiresInSecs: 120 };
    }
    if (command === "machines_cancel") return null;
    if (command === "machines_pair") {
      const code = String(args.code ?? "");
      if (!/^\d{8}$/.test(code))
        throw new Error("A pairing code is exactly 8 digits. Ask the machine again for a new code.");
      const paired = mockMachine("connected", Date.now());
      this.machineList = [...this.machineList.filter((machine) => machine.id !== paired.id), paired];
      return { ...paired, state: "connecting" };
    }
    if (command === "machines_connect_code") {
      const code = String(args.code ?? "").trim();
      const short = code.replace(/[-\s]/g, "").toUpperCase();
      if (!code.toLowerCase().startsWith("rustcode://connect/") && !/^[2-9A-HJ-NP-Z]{16}$/.test(short))
        throw new Error("a connect code has 16 characters, like K7MQ-29XH-TP4W-NRD6");
      const paired = mockMachine("connected", Date.now());
      this.machineList = [...this.machineList.filter((machine) => machine.id !== paired.id), paired];
      return { ...paired, state: "connecting" };
    }
    if (command === "machines_probe") {
      // The LAN answers; an overlay that accepts no inbound connections times out.
      const at = new Date().toISOString();
      this.machineList = this.machineList.map((machine) => {
        if (args.id && machine.id !== String(args.id)) return machine;
        const routes = (machine.routes as JsonObject[]).map((route) => ({
          ...route,
          probe:
            route.kind === "lan"
              ? { outcome: "reachable", latencyMs: 3, detail: null, at }
              : { outcome: "timeout", latencyMs: null, detail: null, at },
        }));
        return { ...machine, routes };
      });
      return this.machineList;
    }
    if (command === "machines_remove") {
      this.machineList = this.machineList.filter((machine) => machine.id !== String(args.id));
      return null;
    }
    if (command === "app_version") return "1.28.0";
    if (command === "server_service_status") return mockServerService(this.serverInstalled);
    if (command === "server_service_install") {
      this.serverInstalled = true;
      return mockServerService(true);
    }
    if (command === "server_link_path") return mockServerService(this.serverInstalled);
    if (command === "machines_set_paused") {
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, paused: args.paused === true } : machine,
      );
      return null;
    }
    if (command === "machines_set_display_name") {
      const name = typeof args.name === "string" && args.name.trim() ? args.name.trim() : null;
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, displayName: name } : machine,
      );
      return null;
    }
    if (command === "machines_set_icon") {
      const icon = typeof args.icon === "string" && args.icon ? args.icon : null;
      const tint = icon && typeof args.tint === "string" && args.tint ? args.tint : null;
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, icon, iconTint: tint } : machine,
      );
      return null;
    }
    // A machine's own channel is opened in process (`MachineTransport` hands it a MockMachine).
    if (command === "machine_connect" || command === "machine_send" || command === "machine_disconnect") {
      const machine = this.machineList.find((saved) => saved.id === String(args.id));
      return { connected: machine?.state !== "offline" && machine?.state !== "needsPairing" };
    }
    if (command === "link_meta") return mockLinkMeta(String(args.url));
    if (command === "server_status") return this.serverStatus();
    if (command === "system_info") return MOCK_SYSTEM_INFO;
    if (command === "resolve_binaries")
      return (args.names as string[]).map((name) =>
        ["codex", "claude-agent-acp", "gemini", "opencode"].includes(name)
          ? `/opt/homebrew/bin/${name}`
          : null,
      );
    if (command === "logs_folder_open") return null;
    if (command === "diagnostics_save") return `/Users/lars/Downloads/${String(args.name)}`;
    if (command === "thread_log_open") {
      const id = String(args.id);
      if (!/^[A-Za-z0-9_-]+$/.test(id)) throw new Error("Not a thread id.");
      return `/Users/lars/.config/rustcode/threads/2026/11/08/${id}/logs/debug.log`;
    }
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
      // Canned fixtures only: the actual filesystem policy lives in native.rs.
      const path = String(args.path);
      if (path === "/Users/you/Private/screenshot.png")
        throw new Error("Outside the thread, project, worktree and temporary folders.");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="200" viewBox="0 0 480 200"><rect width="480" height="200" rx="12" fill="#18181a"/><polyline fill="none" stroke="#93a3e8" stroke-width="3" points="20,180 80,170 140,150 200,110 260,40 320,40 380,40 440,40"/><text x="20" y="28" fill="#a1a1a1" font-family="-apple-system" font-size="14">${path}</text></svg>`;
      return { name: path.split("/").pop(), mimeType: "image/svg+xml", data: btoa(svg) };
    }
    if (command === "attached_files") {
      // The shell's `attached_files` (native.rs), with the same rules: inside the roots,
      // a file, at most ten of them and 5 MB each. A canned size stands in for the file.
      const paths = Array.isArray(args.paths) ? args.paths.map(String) : [];
      const absolute = (path: string) => /^(?:[A-Za-z]:)?[\\/]/.test(path);
      const one = (path: string) => path.replace(/\\/g, "/");
      const roots = Array.isArray(args.roots) ? args.roots.map(String).filter(absolute) : [];
      if (paths.length > 10) throw new Error("A message attaches at most 10 files.");
      if (!roots.length) throw new Error("This thread has no folder to attach a file from.");
      return paths.map((path) => {
        if (!absolute(path)) throw new Error(`${path} is not an absolute path.`);
        const file = one(path);
        const root = roots.find((r) => {
          const base = one(r).replace(/\/+$/, "");
          return file === base || file.startsWith(`${base}/`);
        });
        if (!root) throw new Error(`${path} is outside this thread's project.`);
        return { path, name: file.split("/").pop() ?? path, bytes: 1024 };
      });
    }
    if (command === "skill_create") return `~/.config/rustcode/skills/${String(args.name)}/SKILL.md`;
    // The mock window has no system clipboard to paste from, and no picture to stand in for.
    if (command === "clipboard_image") return null;
    throw new Error(`The mock does not run ${command}.`);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }
}

/** The server's stderr at start, a few levels of it, for Settings › Diagnostics. */
const MOCK_SERVER_LOG = [
  "desktop: started /Applications/RustCode.app/Contents/MacOS/rustcode --acp (pid 48213)",
  "2026-11-08T09:12:01.114Z  INFO rustcode::acp: listening on stdio (protocol 1)",
  "2026-11-08T09:12:01.120Z DEBUG rustcode::config: read /Users/lars/.config/rustcode/config.toml",
  "2026-11-08T09:12:01.402Z  INFO rustcode::harness: found codex 0.50.0, claude-agent-acp 0.9.1",
  "2026-11-08T09:12:01.733Z  WARN rustcode::harness: gemini: not signed in (gemini auth login)",
  '2026-11-08T09:12:02.010Z ERROR rustcode::mcp: server "linear" exited during initialize: token=lin_api_abcdef123456',
  "2026-11-08T09:12:02.011Z  INFO rustcode::store: data.sqlite ready (migrations 14)",
];

/** `system_info`, as the shell answers it on a Mac. */
const MOCK_SYSTEM_INFO: JsonObject = {
  os: "macos",
  osVersion: "15.2 (24C101)",
  kernel: null,
  arch: "aarch64",
  appVersion: "1.21.0",
  tauriVersion: "2.12.5",
  webview: "WKWebView",
  webviewVersion: "20620.1.16.11.8",
  home: "/Users/lars",
  graphicsEnv: [],
};

/**
 * A project's files as the shell lists them (git's own order): more than the `@` drawer shows at
 * once, so its short list and its `Show all` row can be seen in the mock window.
 */
const MOCK_PROJECT_FILES = [
  "Cargo.lock",
  "Cargo.toml",
  "README.md",
  "apps/desktop/package.json",
  "apps/desktop/src/lib/native.ts",
  "apps/desktop/src/main.tsx",
  "apps/desktop/src/state/compose.ts",
  "apps/desktop/src/state/store.ts",
  "apps/desktop/src/ui/Composer.tsx",
  "apps/desktop/src/ui/Sidebar.tsx",
  "docs/design.md",
  "docs/desktop.md",
  "docs/my notes.md",
  "rustcode/engine/src/acp.rs",
  "rustcode/engine/src/tools/mod.rs",
  "scripts/release.sh",
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

/**
 * A QR-shaped module matrix for the mock's pairing dialog (one string of `0`
 * and `1` per row, quiet zone included). It is drawn, not encoded: the mock
 * has no pairing payload worth scanning, and the dialog only needs the shape a
 * real code has. The engine sends the real one (`remote_gateway/qr.rs`).
 */
function mockQrRows(size = 37): string[] {
  let seed = 0x31_85_79;
  const next = () => {
    seed = (seed * 1_103_515_245 + 12_345) & 0x7fff_ffff;
    return seed / 0x7fff_ffff;
  };
  const rows: string[] = [];
  for (let y = 0; y < size; y += 1) {
    let row = "";
    for (let x = 0; x < size; x += 1) {
      const inFinder = (ox: number, oy: number) =>
        x >= ox && x < ox + 7 && y >= oy && y < oy + 7
          ? // A finder pattern: a dark ring with a dark 3x3 centre.
            Math.max(Math.abs(x - ox - 3), Math.abs(y - oy - 3)) !== 2
          : null;
      // A four-module quiet zone, as a scanner expects.
      const left = inFinder(4, 4);
      const right = inFinder(size - 11, 4);
      const bottom = inFinder(4, size - 11);
      const finder = left ?? right ?? bottom;
      row += finder === null ? (next() > 0.5 ? "1" : "0") : finder ? "1" : "0";
    }
    rows.push(row);
  }
  return rows;
}
