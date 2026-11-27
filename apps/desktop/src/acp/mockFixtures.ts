// design-lint: allow-file (fake colours and issue numbers in canned data)
import { arr, type Json, type JsonObject, obj, str } from "./jsonrpc";
import { EFFORTS, HARNESS_MODELS, RUSTCODE_MODELS } from "./mockHarnesses";
import { type MockThread, minutes } from "./mockSeeds";
import type { HarnessSnapshot } from "./types";

// The mock engine's canned settings, skills, context, commands and pull requests. Split out of
// mock.ts.

/** The keys a project may override, as the engine's `PROJECT_KEYS` lists them. */
export const MOCK_PROJECT_KEYS = [
  "default-harness",
  "text-generator",
  "voice",
  "default-permission-mode",
  "resume-after-restart",
  "continue-after-limit",
  "merge-when-green",
  "auto-compact-mode",
  "auto-compact-threshold",
];

export const mockProjectKey = (path: string) => path.replace(/[\\/]+$/, "") || path;

/** `_rustcode/settings/get` as the engine answers it, resolved default → machine → project. */
export function mockSettingsAnswer(
  machine: JsonObject,
  projects: Record<string, JsonObject>,
  keys: string[],
  project: string | undefined,
): JsonObject {
  const overrides = project ? (projects[mockProjectKey(project)] ?? {}) : {};
  const all = { ...machine };
  for (const [key, value] of Object.entries(overrides)) if (key in all) all[key] = value;
  const values = keys.length
    ? Object.fromEntries(keys.filter((key) => key in all).map((key) => [key, all[key]!]))
    : all;
  const sources = Object.fromEntries(
    Object.keys(values).map((key) => [key, key in overrides ? "project" : "machine"]),
  );
  return {
    values,
    stored: Object.keys(machine),
    sources,
    ...(project
      ? {
          project: {
            path: mockProjectKey(project),
            overrides: { ...overrides },
            overridable: MOCK_PROJECT_KEYS,
          },
        }
      : {}),
  };
}

/** `_rustcode/settings/set`: the machine's values, or a project's overrides (`project`, `clear`). */
export function mockSettingsSet(
  machine: JsonObject,
  projects: Record<string, JsonObject>,
  params: JsonObject,
): JsonObject {
  const values = obj(params.values) ?? {};
  const clear = arr(params.clear).flatMap((key) => (typeof key === "string" ? [key] : []));
  const project = str(params.project);
  if (project) {
    const refused = Object.keys(values).find((key) => !MOCK_PROJECT_KEYS.includes(key));
    if (refused) throw Object.assign(new Error(`a project cannot override '${refused}'`), { code: -32602 });
    const key = mockProjectKey(project);
    const next: JsonObject = { ...(projects[key] ?? {}), ...values };
    for (const name of clear) delete next[name];
    if (Object.keys(next).length) projects[key] = next;
    else delete projects[key];
    return machine;
  }
  const next = { ...machine, ...values };
  for (const name of clear) delete next[name];
  return next;
}

/** Desktop control on a Mac that has not granted its permissions yet. */
export const MOCK_DESKTOP_CONTROL: JsonObject = {
  enabled: false,
  platform: "macos",
  supported: true,
  reason: null,
  accessibility: false,
  screen_recording: false,
  executable: "/Applications/RustCode.app/Contents/MacOS/rc",
};

/**
 * `_rustcode/settings/set` with desktop control's status kept, as the engine
 * reports it: asking for the permissions grants both in the mock.
 */
export function mockDesktopSettingsSet(
  machine: JsonObject,
  projects: Record<string, JsonObject>,
  params: JsonObject,
): JsonObject {
  const before = obj(machine["desktop-control"]);
  const next = mockSettingsSet(machine, projects, params);
  const asked = obj(next["desktop-control"]);
  if (!before || !asked || asked === before) return next;
  const granted = asked.request_permissions === true;
  const desktop = { ...before, enabled: asked.enabled === true };
  return {
    ...next,
    "desktop-control": granted ? { ...desktop, accessibility: true, screen_recording: true } : desktop,
  };
}

/** The local mock's skills; their text is served only by `_rustcode/skills/get`. */
export const MOCK_SKILLS: JsonObject[] = [
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

export function withoutSkillContent(skill: JsonObject): JsonObject {
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
   * machine ended, and `forward` a VPS reached through a user-run `ssh -L`
   * that lost its link while this computer slept, with the offer to let
   * RustCode manage that tunnel. The default is the paired, connected machine
   * the happy path shows.
   */
  machines?: "paired" | "connect" | "offline" | "revoked" | "forward";
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

export interface MockConnectRequest {
  id: string;
  deviceName: string;
  deviceKind: string;
  platform: string;
  remoteAddr: string;
  createdAt: number;
  expiresAt: number;
  state: "pending" | "approved";
}

/** What the mock's report commands answer, in the chunks they stream in. */
export const MOCK_REPORTS: Record<string, string[]> = {
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
export const MOCK_CONTEXT: Json[] = [
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
export const MOCK_CONTEXT_COMPACTED: Json[] = [
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
export function mockContext(
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

export const COMMANDS: Json[] = [
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

export function configOptions(thread: MockThread, harnesses: HarnessSnapshot): Json[] {
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

/** The pull requests the mock project has; `/view` opens one of these by number. */
export const MOCK_PULLS = [
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
];

/**
 * Each pull's check runs, matching its row's counts: #2781's one failure is an
 * advisory job (Merge stays open), #2795's is a real one (Merge is blocked).
 */
export const MOCK_CHECK_RUNS: Record<number, { name: string; status: string }[]> = {
  2781: [
    { name: "desktop / check", status: "passed" },
    { name: "lint", status: "passed" },
    { name: "label", status: "passed" },
    { name: "Desktop UI", status: "passed" },
    { name: "rust / test", status: "passed" },
    { name: "clippy", status: "passed" },
    { name: "Windows and macOS portability coverage", status: "failed" },
  ],
  2795: [
    { name: "desktop / check", status: "passed" },
    { name: "lint", status: "passed" },
    { name: "label", status: "passed" },
    { name: "rust / test", status: "passed" },
    { name: "Desktop UI", status: "failed" },
  ],
};
