// The mock's harnesses, their capabilities and models (acp/mock.ts).

import { BUILTIN_CAPABILITIES, type HarnessCapabilities, type HarnessSnapshot } from "./types";

export const RUSTCODE_MODELS = [
  { value: "chatgpt/gpt-6-luna", name: "chatgpt/gpt-6-luna", description: "gpt-6-luna" },
  { value: "chatgpt/gpt-6-astra", name: "chatgpt/gpt-6-astra", description: "gpt-6-astra" },
  { value: "claude-sonnet-5", name: "claude-sonnet-5", description: "claude-sonnet-5" },
];
export const EFFORTS = ["low", "medium", "high", "xhigh"];

export const CODEX_CAPS: HarnessCapabilities = {
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
export const ACP_CAPS: HarnessCapabilities = {
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
export const CLAUDE_CAPS: HarnessCapabilities = {
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

export function initialHarnesses(onboarded: boolean): HarnessSnapshot {
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
          {
            id: "personal-1",
            name: "Personal 1",
            config_dir: "~/.codex-personal-1",
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
      {
        id: "cursor",
        name: "Cursor",
        kind: "acp",
        builtin: false,
        known: true,
        enabled: false,
        command: ["agent", "acp"],
        default_command: ["agent", "acp"],
        installed: false,
        auth: "unknown",
        login_command: ["agent", "login"],
        install_hint: "curl https://cursor.com/install -fsS | bash",
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
export function withLast(snapshot: HarnessSnapshot): HarnessSnapshot {
  return { ...snapshot, harnesses: snapshot.harnesses.map((h) => ({ ...h, last: h.last ?? null })) };
}

export const HARNESS_MODELS: Record<
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
export const MOCK_CUSTOM_MODELS = [
  { id: "fast", name: "Fast", efforts: [] },
  { id: "deep", name: "Deep", efforts: [] },
];
