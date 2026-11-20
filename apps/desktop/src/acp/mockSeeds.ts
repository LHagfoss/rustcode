// The mock's seeded threads, their patches and the delegate links between them (acp/mock.ts).

import { type Json, type JsonObject, str } from "./jsonrpc";

export interface MockThread {
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

export interface MockWorktree {
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

export interface MockCheckpoint {
  turn: number;
  prompt: string;
  created_at: string;
  files: { path: string; status: string; insertions: number; deletions: number; patch: string }[];
  /** The state a rewind abandoned, kept for "go back to this attempt": no turn of its own. */
  attempt?: boolean;
}

export const COMPOSER_PATCH = `@@ -40,2 +40,3 @@
   setDraft("");
   submit(text);
+  requestAnimationFrame(() => input.current?.focus());`;

export const APP_PATCH = `@@ -1,2 +1,2 @@
-const greeting = "hello";
+const greeting = "hello, world";
 export default greeting;`;

export const LOGIN_PATCH = `@@ -0,0 +1,14 @@
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

export const SETTINGS_SEARCH_PATCH = `@@ -18,6 +18,17 @@
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

export const SETTINGS_FILTER_PATCH = `@@ -1,4 +1,4 @@
-export function matches(group: SettingsGroup, query: string) {
-  return group.title.toLowerCase().includes(query.toLowerCase());
+export function matches(group: SettingsGroup, query: string) {
+  const needle = query.trim().toLowerCase();
+  if (!needle) return true;
+  if (group.title.toLowerCase().includes(needle)) return true;
+  return group.rows.some((row) => isOpen(group, row) && matchesRow(row, needle));`;

export const SETTINGS_HELPER_PATCH = `@@ -0,0 +1,10 @@
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

export const SETTINGS_KEYWORDS_PATCH = `@@ -2,2 +2,7 @@
 export function matchesRow(row: SettingsRow, needle: string): boolean {
-  return row.label.toLowerCase().includes(needle) || row.description.toLowerCase().includes(needle);
+  const words = [row.label, row.description, ...(row.keywords ?? [])].join(" ").toLowerCase();
+  return words.includes(needle);
 }
+
+/** Keywords a row answers to besides its own words, such as "dark" for Appearance. */
+export const KEYWORDS: Record<string, string[]> = { appearance: ["dark", "theme"] };`;

export function slug(text: string): string {
  return (
    text
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 40) || "thread"
  );
}

export const now = Date.now();
export const minutes = (n: number) => now - n * 60_000;

export function update(sessionId: string, body: JsonObject): JsonObject {
  return { sessionId, update: body };
}

export const text = (t: string) => ({ type: "text", text: t });

/** An orchestration call's receipt key: the caller thread and its retry key, as the store holds it. */
export function receiptKey(params: JsonObject | undefined): string {
  return `${str(params?.caller) ?? ""}\u0000${str(params?.clientRequestId) ?? ""}`;
}

/** A recorded parent as `session/list` carries it in `_meta.rustcode.parent`. */
export const mockParent = (parent: { id?: string; title?: string; relationship?: string }): Json => ({
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
export function seedBranchThreads(): MockThread[] {
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
      history: turn("mock-branch-plain", "What should the new command be called?", "`rc data query`."),
    },
  ];
}

/**
 * `?scenario=delegated`: a parent with a child whose link is long pruned, and a
 * thread an older release started whose parent is gone. The child is nested
 * under its parent; the orphan has no parent to sit under, so it keeps a row of
 * its own (`state/topLevel.ts`).
 */
export function seedDelegatedThreads(): MockThread[] {
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
      history: turn("mock-plain", "What should the new command be called?", "`rc data query`."),
    },
  ];
}

/** The threads of `?scenario=delegates`: ids and titles, shared by the threads and the links. */
export const DELEGATES = {
  root: "mock-delegate-root",
  research: "mock-delegate-research",
  tests: "mock-delegate-tests",
  docs: "mock-delegate-docs",
  review: "mock-delegate-review",
  wait: "mock-delegate-wait",
  stop: "mock-delegate-stop",
} as const;

export const DELEGATE_TITLES: Record<string, string> = {
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
export function seedDelegateTreeThreads(): MockThread[] {
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
export function seedDelegateLinks(): JsonObject[] {
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
export const ALLOWED_NOTICE_LINE = "Cleaned up a delegate's worktree · allowed by Full access";

/**
 * `?notices=repeat`: a turn that ended on the same quiet notice six times, as a run of calls its
 * thread's access mode let through leaves it (one notice per call, each an app-side row the engine
 * never sent). The only engine line that draws that quiet row is a finished compaction, so each
 * notice replays as one; the transcript then draws a single row, with its count.
 */
export function seedRepeatedNotices(threads: MockThread[]): void {
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
export function seedToolRowsThread(): MockThread {
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

export function seedThreads(): MockThread[] {
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
            "![Temp screenshot](/tmp/rustcode/mock-agent-images/screenshot.png)\n\n**Settings, light theme.** The blocked one sits in the same paragraph as this sentence.\n![Blocked screenshot](/Users/you/Private/screenshot.png)\nThe text after it starts on its own line.",
          ),
          ...turn(2000),
        }),
      ],
    },
    // Quiet older threads, so the list runs past one screen and the compact density shows its
    // difference (Settings › Appearance › Sidebar rows).
    {
      id: "mock-quiet-1",
      cwd: "/Users/you/code/rustcode",
      title: "Bump the bun lockfile",
      updatedAt: minutes(1800),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-1", {
          sessionUpdate: "user_message_chunk",
          content: text("Bump the bun lockfile"),
        }),
        update("mock-quiet-1", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-2",
      cwd: "/Users/you/code/website",
      title: "Fix the footer links on mobile",
      updatedAt: minutes(2040),
      harness: "claude-code",
      model: "sonnet",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-2", {
          sessionUpdate: "user_message_chunk",
          content: text("Fix the footer links on mobile"),
        }),
        update("mock-quiet-2", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-3",
      cwd: "/Users/you/code/rustcode",
      title: "Explain the command guard rules",
      updatedAt: minutes(2400),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-3", {
          sessionUpdate: "user_message_chunk",
          content: text("Explain the command guard rules"),
        }),
        update("mock-quiet-3", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-4",
      cwd: "/Users/you/code/website",
      title: "Compress the hero images",
      updatedAt: minutes(2880),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-4", {
          sessionUpdate: "user_message_chunk",
          content: text("Compress the hero images"),
        }),
        update("mock-quiet-4", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    // A scheduled job's run (mockLoops.ts › CI watch): kept out of the sidebar, opened from its job.
    {
      id: "mock-loop-ci-3",
      cwd: "/Users/you/code/rustcode",
      title: "CI watch",
      updatedAt: minutes(150),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-loop-ci-3", {
          sessionUpdate: "user_message_chunk",
          content: text(
            "Check CI on main. Tell me only if something broke, with the failing job and its first error.",
          ),
        }),
        update("mock-loop-ci-3", {
          sessionUpdate: "agent_message_chunk",
          content: text(
            "`lint` failed on main at 14:02: `biome` found an unused import in `src/ui/Loops.tsx:9`.",
          ),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-5",
      cwd: "/Users/you/code/rustcode",
      title: "Trace the slow thread list load on a cold start",
      updatedAt: minutes(3120),
      harness: "claude-code",
      model: "opus",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-5", {
          sessionUpdate: "user_message_chunk",
          content: text("Trace the slow thread list load on a cold start"),
        }),
        update("mock-quiet-5", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-6",
      cwd: "/Users/you/code/rustcode",
      title: "Draft the release notes",
      updatedAt: minutes(4200),
      harness: "rustcode",
      model: "chatgpt/gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-6", {
          sessionUpdate: "user_message_chunk",
          content: text("Draft the release notes"),
        }),
        update("mock-quiet-6", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-7",
      cwd: "/Users/you/code/website",
      title: "Add a changelog page",
      updatedAt: minutes(5760),
      harness: "claude-code",
      model: "sonnet",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-7", {
          sessionUpdate: "user_message_chunk",
          content: text("Add a changelog page"),
        }),
        update("mock-quiet-7", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
    {
      id: "mock-quiet-8",
      cwd: "/Users/you/code/rustcode",
      title: "Rename the worktree helper",
      updatedAt: minutes(7200),
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
      checkpoints: [],
      history: [
        update("mock-quiet-8", {
          sessionUpdate: "user_message_chunk",
          content: text("Rename the worktree helper"),
        }),
        update("mock-quiet-8", {
          sessionUpdate: "agent_message_chunk",
          content: text("Done."),
          ...turn(9000),
        }),
      ],
    },
  ];
}
