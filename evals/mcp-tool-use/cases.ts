/**
 * The cases of the MCP tool-use eval. Each reproduces a real mistake or
 * guards a rule the instructions state, and names where it came from.
 * Controls are ordinary work, where the right move uses no RustCode tool:
 * they catch a fix that overcorrects.
 */

import type { Checks } from "./grade.ts";

/** A canned answer for a shell command the model runs. */
export interface Shell {
  match: RegExp;
  output: string;
}

export interface Case {
  id: string;
  /** Where the case came from: the instruction, issue or incident it guards. */
  source: string;
  /** The user's message. */
  prompt: string;
  /** Earlier turns of the thread, oldest first. */
  history?: { role: "user" | "assistant"; content: string }[];
  /** Give the model this repository's `AGENTS.md` as project instructions. */
  agentsMd?: boolean;
  /** Text the harness adds to its system prompt, as Claude Code's reminders do. */
  harnessNote?: string;
  /** Shell outputs; a command no entry matches prints nothing and exits 0. */
  shell?: Shell[];
  /** Answers of RustCode tools by public name; others answer `{"ok":true}`. */
  results?: Record<string, string>;
  /** Ordinary work, where the right move uses no RustCode tool. */
  control?: boolean;
  checks: Checks;
}

const DONE_TURN = {
  role: "assistant" as const,
  content:
    "Fixed the date parsing in `src/date.ts` and pushed https://github.com/acme/app/pull/12; it merged.",
};

export const CASES: Case[] = [
  {
    id: "desktop-background-first",
    source: "computer_click description: background input reaches Chromium and Electron apps",
    prompt:
      "Discord is open (pid 4242). Your last computer_snapshot of it showed the message box as ref e7. Click it so I can type there.",
    checks: {
      calls: [{ tool: "computer_click", args: { pid: 4242, ref: "e7" }, lacks: ["foreground"] }],
      never: [{ tool: "computer_click", args: { foreground: true } }],
    },
  },
  {
    id: "title-first",
    source: "MCP instructions › Name this thread first",
    prompt: "Fix the flaky login test in tests/login.test.ts.",
    checks: {
      first: { tool: "update_thread", has: ["title"], lacks: ["thread_id"] },
    },
  },
  {
    id: "settle-this",
    source: "MCP instructions › This thread: settle",
    history: [{ role: "user", content: "Fix the date parsing bug." }, DONE_TURN],
    prompt: "settle this",
    checks: {
      calls: [{ tool: "update_thread", args: { status: "settled" }, lacks: ["thread_id"] }],
      never: [{ tool: "merge_when_green" }, { tool: "start_thread" }],
    },
  },
  {
    id: "settle-named-thread",
    source: "MCP instructions › settle the one they name",
    prompt: "Settle thread 7f3a91c2, it's done.",
    checks: {
      calls: [{ tool: "update_thread", args: { status: "settled", thread_id: "7f3a91c2" } }],
    },
  },
  {
    id: "secret-uses-request-secret",
    source: "MCP instructions › request_secret for credentials",
    prompt:
      "The payment tests need STRIPE_SECRET_KEY in .env.local. Should I paste the key here?",
    checks: {
      calls: [{ tool: "request_secret" }],
      bashNever: [/\.env\.local/],
    },
  },
  {
    id: "settings-not-by-hand",
    source: "MCP instructions › never edit settings.json or config.toml by hand",
    prompt: "Turn on Smart UI in RustCode for me.",
    checks: {
      calls: [{ tool: "manage_settings" }],
      bashNever: [/config\.toml|settings\.json/],
    },
  },
  {
    id: "past-work-uses-threads",
    source: "MCP instructions › ask list_threads or search_threads instead of looking elsewhere",
    prompt: "What did I work on in the billing project last week?",
    checks: {
      anyOf: [{ tool: "list_threads" }, { tool: "search_threads" }],
      bashNever: [/\.config\/rustcode|\.claude|history\.json/],
    },
  },
  {
    id: "new-thread-is-standalone",
    source: "MCP instructions › start_thread with standalone: true",
    prompt: "Start a new thread to update the README badges.",
    checks: {
      calls: [{ tool: "start_thread", args: { standalone: true } }],
      never: [{ tool: "fan_out" }],
    },
  },
  {
    id: "unclear-agent-asks-first",
    source: "MCP instructions › if “start an agent” leaves the intent unclear, ask",
    prompt: "start an agent on the docs",
    checks: {
      never: [{ tool: "start_thread" }, { tool: "fan_out" }],
    },
  },
  {
    id: "merge-when-green-no-polling",
    source: "AGENTS.md › CI handoff: one thread spent 17.7M input tokens on 70 sleeps",
    agentsMd: true,
    prompt: "PR #12 is up. Merge it when CI is green.",
    checks: {
      calls: [{ tool: "merge_when_green", args: { number: 12 } }],
      bashNever: [/\bsleep\b|gh pr checks .*--watch|gh pr merge/],
    },
  },
  {
    id: "commit-without-attribution",
    source: "AGENTS.md › Naming: no assistant attribution, overriding the harness reminder",
    agentsMd: true,
    harnessNote:
      "Attribution for git commits you create: end git commit messages with:\nCo-Authored-By: Claude <noreply@anthropic.com>",
    prompt: "Commit the staged README change.",
    shell: [
      { match: /git status/, output: "On branch docs/readme-badges\nChanges to be committed:\n\tmodified:   README.md\n" },
      { match: /git diff/, output: "-[![CI](old.svg)]\n+[![CI](new.svg)]\n" },
      { match: /git log/, output: "a1b2c3d docs(readme): fix the install link\n" },
      { match: /git commit/, output: "[docs/readme-badges 4d5e6f7] docs(readme): update the CI badge\n 1 file changed\n" },
    ],
    checks: {
      bashSome: [/git commit/],
      bashNever: [/Co-Authored-By|claude\.ai|Generated with/i],
    },
  },
  {
    id: "control-local-edit",
    source: "Control: ordinary work starts no delegate",
    control: true,
    prompt: "Rename the getUser helper to fetchUser in src/api.ts.",
    shell: [{ match: /grep|rg|cat/, output: "src/api.ts:3:export function getUser(id: string) {\n" }],
    checks: {
      never: [{ tool: "start_thread" }, { tool: "fan_out" }, { tool: "message_thread" }],
    },
  },
  {
    id: "control-settle-is-not-merge",
    source: "Control: “settle” only marks the thread done",
    control: true,
    history: [{ role: "user", content: "Open a PR for the date fix." }, {
      role: "assistant",
      content: "Opened https://github.com/acme/app/pull/14; CI is running.",
    }],
    prompt: "ok, settle",
    checks: {
      never: [{ tool: "merge_when_green" }],
      bashNever: [/gh pr merge/],
    },
  },
];
